//! REST API — `archietect serve [--root DIR] [--port 7373]`.
//!
//! A thin transport over the same engine the CLI and MCP expose: no business
//! logic lives here, ever. The GUI (when it exists), CI checks, dashboards,
//! and anything HTTP-shaped become clients of the same deterministic answers
//! — GitHub Desktop is a client of git, never a fork of it.
//!
//! Read-only by design, with one deliberate exception: every endpoint is a
//! query and nothing here EVER writes archietect.db (the daemon remains the
//! single writer for that) — except `/proposal/*`, which writes only under
//! .archietect/proposals/ or, for `accept`, the working tree itself
//! (uncommitted, never `git commit`). See src/proposal.rs's own module doc
//! for the trust boundary that makes that safe. Every other endpoint here
//! can serve any number of concurrent readers without coordination.
//!
//! Binding to 127.0.0.1 keeps remote attackers out, but not a malicious page
//! open in the same browser as the operator — a plain `<img>` tag can fire a
//! cross-origin GET with no confirmation. So the five write endpoints
//! (`proposal/{submit,test,accept,reject}`, `system/register`) additionally
//! require `&token=...`, printed once to stderr when `serve` starts; nothing
//! else on the machine can guess it. Responses carry no
//! `Access-Control-Allow-Origin` header — the embedded GUI at `/` is
//! same-origin and needs none.
//!
//!   GET /concept?q=invoice[&root=/path]      GET /doctor
//!   GET /intent?q=add+invoicing              GET /tour
//!   GET /impact?q=payments                   GET /duplicates
//!   GET /imports?file=src/foo.ts (exact relative-import edges only)
//!   GET /owner?q=invoice                     GET /status
//!   GET /guard?sql=CREATE+TABLE+...          GET /laws
//!   GET /plan?q=add+invoicing                GET /ci?diff=...[&strict=true]
//!   GET /history[?q=concept][&limit=50]      GET /permissions[&root=/path]
//!   GET /system/list                         GET /system/query?q=Widget
//!   GET /system/status                       GET /register[&root=/path]
//!   GET /permissions/check?path=...&domain=code[&root=/path]
//!   POST /system/register[&root=/path]&token=...  (writes ~/.archietect/system.db)
//!   GET /documents/scan?dir=/path[&root=/path]     (see module doc below)
//!   GET /photos/scan?dir=/path[&root=/path]        (same contract as /documents/scan)
//!
//! `root` may come per-request or from --root at startup, same contract as
//! the MCP server: one process can serve every repository on the machine.
//! `/system/list`, `/system/query`, and `/system/status` are the exception —
//! like `/laws`, they answer from `~/.archietect/system.db` directly and
//! need no root at all (a project root is meaningless for "list/summarize
//! every project I know about").
//!
//! ## `/documents/scan` never prompts
//!
//! `permissions::domain_allowed_with_confirmation`'s interactive y/N prompt
//! is designed around a real terminal. Neither REST nor MCP has one, and
//! blocking a server's single request-handling thread on stdin that will
//! never receive input would hang the whole process for every other client.
//! So this endpoint always passes `NonInteractiveAsker`, which fails closed
//! (`"enabled": false`) rather than prompting — the documents domain is only
//! ever reachable over REST/MCP for a project that already has
//! `[domains.documents]` explicitly set in its `archietect.toml`/
//! `system.toml` (an explicit config entry is honored without ever
//! consulting the asker at all — see `permissions.rs`'s own doc on
//! `domain_allowed_with_confirmation`). The CLI (`archietect documents
//! scan`) remains the only way to answer the confirmation prompt itself.
//!
//! ## The warm cache
//!
//! REST is a long-running SERVER, not a one-shot CLI invocation — but until
//! this fix it behaved like the CLI called in a loop: every request called
//! `scan::scan(&root)` fresh, with no memory of the previous request. Found
//! by dogfooding: on TITAN (1,483 files) with no persisted archietect.db,
//! that meant every single HTTP request paid the full 11+ SECOND cold-scan
//! cost, forever — a "server" that was never actually warm.
//!
//! The fix mirrors what the daemon already does, scoped to this process:
//! keep the last built `Index` per root IN MEMORY and pass it as `prior` to
//! `scan_with_prior`. The first request for a root still pays scan cost
//! (once); every request after that is incremental — only files whose
//! (size, mtime) changed get re-parsed, the same guarantee `archietect
//! watch` gives. This is a request-loop-local cache, not a second daemon:
//! it holds no lock, needs no thread-safety, because `serve` is a single
//! blocking loop over `incoming_requests()` — one request handled at a
//! time, by construction.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::{
    docker_domain, documents_domain, laws, model::Index, permissions, photos_domain, query, root, scan, store,
    system_db, structural::StructuralGraph,
};

/// Minimal percent-decoding for query values ('+' and %XX). Deliberately
/// tiny: this API serves identifiers and short text, not arbitrary payloads.
fn decode(s: &str) -> String {
    let s = s.replace('+', " ");
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn params(url: &str) -> (String, std::collections::HashMap<String, String>) {
    let (path, qs) = url.split_once('?').unwrap_or((url, ""));
    let mut map = std::collections::HashMap::new();
    for pair in qs.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(k.to_string(), decode(v));
    }
    (path.to_string(), map)
}

/// A fresh, unguessable-from-outside-the-process token, printed once at
/// startup and required on every `/proposal/{submit,test,accept,reject}`
/// request. Binding to 127.0.0.1 stops a remote attacker, but not a
/// malicious page open in the operator's own browser: without this, that
/// page could fire `<img src="http://127.0.0.1:PORT/proposal/accept?id=1">`
/// and get a write applied to the working tree with zero interaction — GET
/// requests need no CORS approval to be *sent*, only to have their response
/// *read*. `RandomState` pulls its seed from the OS RNG the same way
/// `HashMap`'s DoS-resistant hashing does; that's all the unpredictability
/// this needs; it isn't protecting against an attacker who can already read
/// this process's stdout/environment.
fn generate_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut bytes = [0u8; 16];
    for (i, chunk) in bytes.chunks_mut(8).enumerate() {
        let mut h = RandomState::new().build_hasher();
        h.write_usize(i);
        h.write_u64(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0));
        chunk.copy_from_slice(&h.finish().to_le_bytes()[..chunk.len()]);
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

const MUTATING_ENDPOINTS: &[&str] = &[
    "/proposal/submit",
    "/proposal/test",
    "/proposal/accept",
    "/proposal/reject",
    // Writes a pointer (root path, name, timestamps) into
    // ~/.archietect/system.db — same class of risk as the four proposal
    // endpoints above (a mutating action reachable by a bare cross-origin
    // request), so it gets the same token gate. See src/system_db.rs's own
    // module doc for exactly what this write does and does not store.
    "/system/register",
];

pub fn serve(default_root: Option<PathBuf>, port: u16) -> anyhow::Result<()> {
    let server = tiny_http::Server::http(("127.0.0.1", port))
        .map_err(|e| anyhow::anyhow!("bind 127.0.0.1:{port}: {e}"))?;
    let token = generate_token();
    eprintln!("archietect REST listening on http://127.0.0.1:{port} (read-only except /proposal/* and /system/register)");
    eprintln!("mutating requests (proposal submit/test/accept/reject, system/register) require &token={token}");

    // The warm cache. Single-threaded loop, one request at a time — plain
    // HashMap, no lock needed.
    let mut cache: HashMap<PathBuf, (Index, StructuralGraph)> = HashMap::new();
    // See `crate::exe_mtime`'s doc comment — same staleness detection as the
    // MCP server, for the same reason: this is a long-running process that
    // can outlive many rebuilds of its own binary.
    let started_mtime = crate::exe_mtime();

    for req in server.incoming_requests() {
        let (path, p) = params(req.url());

        if MUTATING_ENDPOINTS.contains(&path.as_str())
            && p.get("token").map(|t| t.as_str()) != Some(token.as_str())
        {
            let body = json!({
                "error": "missing or incorrect token — pass &token=<value printed when `archietect serve` started>",
            });
            let response = tiny_http::Response::from_string(
                serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()),
            )
            .with_status_code(401)
            .with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .unwrap(),
            );
            let _ = req.respond(response);
            continue;
        }

        let root = root::resolve(
            p.get("root").map(PathBuf::from).or_else(|| default_root.clone()),
            &std::env::current_dir().unwrap_or_default(),
        ).ok();

        // GUI v0 — the embedded read-only dashboard, itself a client of the
        // JSON endpoints below. No logic lives in it.
        if path == "/" {
            let response = tiny_http::Response::from_string(include_str!("../ui/index.html"))
                .with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
                        .unwrap(),
                );
            let _ = req.respond(response);
            continue;
        }

        let mut body: Value = match (path.as_str(), root) {
            ("/laws", _) => laws::registry_json(),
            // No root needed — same reasoning as /laws: this answers from
            // ~/.archietect/system.db directly, not from any one project.
            ("/system/list", _) => match system_db::default_db_path().and_then(|db| system_db::list_projects(&db).map(|p| (db, p))) {
                Ok((db_path, projects)) => json!({
                    "projects": projects.iter().map(|p| json!({
                        "root": p.root,
                        "name": p.name,
                        "first_registered_ms": p.first_registered_ms,
                        "last_seen_ms": p.last_seen_ms,
                    })).collect::<Vec<_>>(),
                    "system_db": db_path.display().to_string(),
                }),
                Err(e) => json!({ "error": e.to_string() }),
            },
            ("/system/query", _) => {
                let term = p.get("q").map(|s| s.as_str()).unwrap_or("");
                match system_db::default_db_path().and_then(|db| system_db::query_registered_projects(&db, term).map(|r| (db, r))) {
                    Ok((db_path, results)) => json!({
                        "term": term,
                        "results": results.iter().map(|r| json!({
                            "root": r.root,
                            "name": r.name,
                            "found": r.found,
                        })).collect::<Vec<_>>(),
                        "system_db": db_path.display().to_string(),
                        "note": "each project's own archietect.db is read live and read-only on every call; system.db itself stores only pointers and is never updated by this command.",
                    }),
                    Err(e) => json!({ "error": e.to_string() }),
                }
            }
            // No root needed — same reasoning as /system/list and /laws.
            ("/system/status", _) => match system_db::default_db_path().and_then(|db| system_db::status_registered_projects(&db).map(|r| (db, r))) {
                Ok((db_path, results)) => json!({
                    "projects": results.iter().map(|r| json!({
                        "root": r.root,
                        "name": r.name,
                        "status": r.status,
                    })).collect::<Vec<_>>(),
                    "system_db": db_path.display().to_string(),
                    "note": "each project's own archietect.db is read live and read-only on every call; system.db itself stores only pointers and is never updated by this command.",
                }),
                Err(e) => json!({ "error": e.to_string() }),
            },
            (_, None) => json!({ "error": "no repository root: pass ?root=/path or start with --root" }),
            (_, Some(root)) if !root.exists() => {
                json!({ "error": format!("root does not exist: {}", root.display()) })
            }
            (ep, Some(root)) => {
                // ONE scan per request, incremental against THIS process's
                // last result for this root — not a fresh cold scan every
                // time. Refreshed and re-stored before dispatch, so every
                // endpoint below reads the same warm index.
                let prior = cache.remove(&root);
                let (schema_prior, graph_prior) = match prior {
                    Some((s, g)) => (Some(s), Some(g)),
                    None => (None, None),
                };
                let (idx, graph) = scan::scan_with_prior(&root, schema_prior, graph_prior);
                let result = {
                    let q = p.get("q").map(|s| s.as_str()).unwrap_or("");
                    match ep {
                        "/concept" => query::concept(&idx, &graph, q),
                        "/intent" => query::intent(&idx, q),
                        "/impact" => query::impact(&idx, &graph, q),
                        "/imports" => query::imports(&graph, p.get("file").map(|s| s.as_str()).unwrap_or("")),
                        "/owner" => query::owner(&idx, &graph, q),
                        "/guard" => query::guard(&idx, p.get("sql").map(|s| s.as_str()).unwrap_or("")),
                        "/plan" => query::plan(&idx, &graph, q),
                        "/status" => query::status(&idx, &graph),
                        "/doctor" => query::doctor(&idx, &graph, &root),
                        "/tour" => query::tour(&idx, &graph),
                        "/duplicates" => query::duplicates(&idx),
                        "/history" => json!({
                            "events": store::read_history(
                                &root,
                                p.get("q").map(|s| s.as_str()),
                                p.get("limit").and_then(|l| l.parse().ok()).unwrap_or(50),
                            )
                        }),
                        "/ci" => query::ci(
                            &idx,
                            p.get("diff").map(|s| s.as_str()).unwrap_or(""),
                            p.get("strict").map(|s| s == "true").unwrap_or(false),
                        ),
                        // AI-extension protocol. The one exception to this
                        // module's read-only design (see the header comment)
                        // — `test`/`accept`/`reject` do write, but only ever
                        // under .archietect/proposals/ or, for `accept`, the
                        // working tree itself, uncommitted; never archietect.db.
                        // `patch` here is a query param like everything else
                        // in this file ("identifiers and short text, not
                        // arbitrary payloads" — see `decode()` above): fine
                        // for a small patch, but a large diff should go
                        // through the CLI or the MCP tool instead.
                        "/proposal/submit" => {
                            let kind_str = p.get("kind").map(|s| s.as_str()).unwrap_or("");
                            match serde_json::from_value::<crate::proposal::Kind>(json!(kind_str)) {
                                Err(_) => json!({ "error": format!("unknown proposal kind '{kind_str}' — expected extractor, decision, or alias") }),
                                Ok(kind) => {
                                    let tmp = std::env::temp_dir().join(format!("archietect-rest-proposal-{}.diff", std::process::id()));
                                    match std::fs::write(&tmp, p.get("patch").map(|s| s.as_str()).unwrap_or("")) {
                                        Err(e) => json!({ "error": format!("failed to stage patch: {e}") }),
                                        Ok(()) => {
                                            let out = crate::proposal::submit(
                                                &root, kind,
                                                p.get("title").map(|s| s.as_str()).unwrap_or(""),
                                                p.get("description").map(|s| s.as_str()).unwrap_or(""),
                                                p.get("lang").map(|s| s.as_str()),
                                                p.get("preview_repo").map(|s| s.as_str()),
                                                "ai",
                                                &tmp,
                                            );
                                            let _ = std::fs::remove_file(&tmp);
                                            match out { Ok(v) => v, Err(e) => json!({ "error": e.to_string() }) }
                                        }
                                    }
                                }
                            }
                        }
                        "/proposal/list" => crate::proposal::list(&root),
                        "/proposal/inspect" => match crate::proposal::inspect(&root, p.get("id").and_then(|i| i.parse().ok()).unwrap_or(0)) {
                            Ok(v) => v, Err(e) => json!({ "error": e.to_string() }),
                        },
                        "/proposal/test" => match crate::proposal::test(&root, p.get("id").and_then(|i| i.parse().ok()).unwrap_or(0)) {
                            Ok(v) => v, Err(e) => json!({ "error": e.to_string() }),
                        },
                        "/proposal/accept" => match crate::proposal::accept(&root, p.get("id").and_then(|i| i.parse().ok()).unwrap_or(0)) {
                            Ok(v) => v, Err(e) => json!({ "error": e.to_string() }),
                        },
                        "/proposal/reject" => match crate::proposal::reject(
                            &root,
                            p.get("id").and_then(|i| i.parse().ok()).unwrap_or(0),
                            p.get("purge").map(|s| s == "true").unwrap_or(false),
                        ) {
                            Ok(v) => v, Err(e) => json!({ "error": e.to_string() }),
                        },
                        // Token-gated (see MUTATING_ENDPOINTS) — writes a
                        // pointer for THIS request's resolved root into
                        // ~/.archietect/system.db.
                        "/system/register" => match system_db::default_db_path().and_then(|db| system_db::register_project(&db, &root).map(|proj| (db, proj))) {
                            Ok((db_path, proj)) => json!({
                                "registered": proj.root,
                                "name": proj.name,
                                "first_registered_ms": proj.first_registered_ms,
                                "last_seen_ms": proj.last_seen_ms,
                                "system_db": db_path.display().to_string(),
                            }),
                            Err(e) => json!({ "error": e.to_string() }),
                        },
                        "/permissions" => match permissions::default_global_config_path().and_then(|g| permissions::load(&g, &root)) {
                            Ok(cfg) => permissions::report(&cfg),
                            Err(e) => json!({ "error": e.to_string() }),
                        },
                        // Read-only, no token: answers one path/domain pair
                        // with a reason (`?path=...&domain=code`, domain
                        // defaults to "code"). Same check_resource() a
                        // pre-tool-use hook calls locally over the CLI,
                        // exposed here so a remote MCP/REST client can ask
                        // the same boundary question, not just a local
                        // process.
                        "/permissions/check" => match permissions::default_global_config_path().and_then(|g| permissions::load(&g, &root)) {
                            Ok(cfg) => {
                                let path_str = p.get("path").map(|s| s.as_str()).unwrap_or("");
                                let domain = p.get("domain").map(|s| s.as_str()).unwrap_or("code");
                                let candidate = std::path::PathBuf::from(path_str);
                                let full_path = if candidate.is_absolute() { candidate } else { root.join(&candidate) };
                                let decision = permissions::check_resource(&cfg, domain, &full_path);
                                json!({
                                    "path": full_path.display().to_string(),
                                    "domain": domain,
                                    "allowed": decision.allowed,
                                    "reason": decision.reason,
                                })
                            }
                            Err(e) => json!({ "error": e.to_string() }),
                        },
                        // Read-only, no token: the map of the bag — see
                        // src/register.rs. Composes over the same warm idx.
                        "/register" => crate::register::register(&idx, &graph, &root),
                        // See this module's doc: always NonInteractiveAsker —
                        // never blocks waiting for a y/N answer that can
                        // never arrive over a network transport.
                        "/documents/scan" => match p.get("dir") {
                            None => json!({ "error": "missing required ?dir=<path> parameter" }),
                            Some(dir_str) => {
                                let dir = PathBuf::from(dir_str);
                                let result: anyhow::Result<Value> = (|| {
                                    let global_path = permissions::default_global_config_path()?;
                                    let cfg = permissions::load(&global_path, &root)?;
                                    let confirmations_path = permissions::default_confirmations_path()?;
                                    let (enabled, resources) = documents_domain::scan_if_allowed(
                                        &cfg,
                                        &confirmations_path,
                                        &dir,
                                        &permissions::NonInteractiveAsker,
                                    )?;
                                    Ok(json!({
                                        "dir": dir.display().to_string(),
                                        "enabled": enabled,
                                        "resources": resources,
                                    }))
                                })();
                                match result {
                                    Ok(v) => v,
                                    Err(e) => json!({ "error": e.to_string() }),
                                }
                            }
                        },
                        // Same NonInteractiveAsker contract as /documents/scan
                        // above — see this module's doc.
                        "/photos/scan" => match p.get("dir") {
                            None => json!({ "error": "missing required ?dir=<path> parameter" }),
                            Some(dir_str) => {
                                let dir = PathBuf::from(dir_str);
                                let result: anyhow::Result<Value> = (|| {
                                    let global_path = permissions::default_global_config_path()?;
                                    let cfg = permissions::load(&global_path, &root)?;
                                    let confirmations_path = permissions::default_confirmations_path()?;
                                    let (enabled, resources) = photos_domain::scan_if_allowed(
                                        &cfg,
                                        &confirmations_path,
                                        &dir,
                                        &permissions::NonInteractiveAsker,
                                    )?;
                                    Ok(json!({
                                        "dir": dir.display().to_string(),
                                        "enabled": enabled,
                                        "resources": resources,
                                    }))
                                })();
                                match result {
                                    Ok(v) => v,
                                    Err(e) => json!({ "error": e.to_string() }),
                                }
                            }
                        },
                        // LIVE — shells out to `docker compose ps`, unlike
                        // every other endpoint here. Same
                        // `permissions::domain_allowed` gate the declarative
                        // docker scan uses. See `docker_domain::scan_observed`.
                        "/docker/observe" => match permissions::default_global_config_path().and_then(|g| permissions::load(&g, &root)) {
                            Ok(cfg) => {
                                let resources = docker_domain::scan_observed(&cfg, &root);
                                json!({ "resources": resources })
                            }
                            Err(e) => json!({ "error": e.to_string() }),
                        },
                        other => json!({
                            "error": format!("unknown endpoint {other}"),
                            "endpoints": ["/concept", "/intent", "/impact", "/imports", "/owner", "/guard", "/plan",
                                          "/status", "/doctor", "/tour", "/duplicates",
                                          "/history", "/ci", "/laws", "/permissions", "/permissions/check", "/register",
                                          "/system/list", "/system/query", "/system/status", "/system/register",
                                          "/documents/scan", "/photos/scan", "/docker/observe",
                                          "/proposal/submit", "/proposal/list", "/proposal/inspect",
                                          "/proposal/test", "/proposal/accept", "/proposal/reject"],
                        }),
                    }
                };
                cache.insert(root, (idx, graph));
                result
            }
        };

        if let (Some(started), Some(now)) = (started_mtime, crate::exe_mtime()) {
            if now != started {
                if let Value::Object(ref mut map) = body {
                    map.insert("_stale_binary_warning".to_string(), json!(
                        "This REST server process has been running since before the archietect binary on disk was last rebuilt — it is answering from OLD code in memory. Restart the `archietect serve` process to pick up the current build."
                    ));
                }
            }
        }

        // Output shaping (src/shape.rs): `?only=a,b` and `?compact=true`.
        // Applied here, at the one serialization point, so no endpoint's
        // output changes unless the caller asks.
        let body = crate::shape::apply(
            body,
            crate::shape::parse_only(p.get("only").map(|s| s.as_str())).as_deref(),
            p.get("compact").map(|s| s == "true" || s == "1").unwrap_or(false),
        );
        let data = serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into());
        // No Access-Control-Allow-Origin header: the embedded GUI at "/" is
        // same-origin and needs none; a page on any OTHER origin has no
        // business reading these responses, and without this header the
        // browser won't let it, regardless of what request it manages to send.
        let response = tiny_http::Response::from_string(data).with_header(
            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
        );
        let _ = req.respond(response);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::time::Duration;

    /// Kills the spawned `archietect serve` child even if an assertion
    /// panics mid-test — otherwise a failed test leaks a real bound TCP
    /// listener for the rest of the test run.
    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn bin_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/archietect")
    }

    fn tmp_dir(label: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("archietect-rest-test-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// Spawns a REAL `archietect serve` subprocess (the actual compiled
    /// binary, not rest.rs's internals called in-process) with its own
    /// isolated HOME, so it never touches the real machine's
    /// ~/.archietect/system.db, and returns a guard plus the real random
    /// token it printed on startup — parsed off its own stderr, the same
    /// way an operator running this command themselves would read it.
    fn spawn_server(project_root: &Path, home: &Path, port: u16) -> (ChildGuard, String) {
        let mut child = Command::new(bin_path())
            .args(["serve", "--root", project_root.to_str().unwrap(), "--port", &port.to_string()])
            .env("HOME", home)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn archietect serve — is target/release/archietect built?");

        let stderr = child.stderr.take().unwrap();
        let mut reader = std::io::BufReader::new(stderr);
        let mut token = None;
        use std::io::BufRead;
        // Bounded read: a broken binary that never prints its token must
        // fail this test loudly, not hang it waiting for a line that never
        // comes.
        for _ in 0..10 {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            if let Some(t) = line.trim().strip_prefix("mutating requests (proposal submit/test/accept/reject, system/register) require &token=") {
                token = Some(t.to_string());
                break;
            }
        }
        let token = token.expect("server never printed its token on stderr");

        let mut connected = false;
        for _ in 0..50 {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                connected = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(connected, "server on port {port} never accepted a connection");

        (ChildGuard(child), token)
    }

    /// Minimal raw HTTP/1.1 GET client — this project has no HTTP client
    /// dependency (see Cargo.toml), and a GET this simple doesn't warrant
    /// pulling one in for tests. A 5-second read timeout turns a server that
    /// hangs (e.g. a regression that makes /documents/scan block on a
    /// confirmation prompt it can never receive) into a loud test failure
    /// instead of hanging this whole test suite forever.
    fn http_get(port: u16, path_and_query: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let req = format!("GET {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).unwrap();
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).expect("reading response (or it hung/timed out)");
        let resp = String::from_utf8_lossy(&resp);
        let status = resp
            .lines()
            .next()
            .unwrap_or("")
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body = resp.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (status, body)
    }

    // Fixed, widely-spaced high ports — distinct per test so they can run
    // concurrently (the default for `cargo test`) without colliding with
    // each other; arbitrary enough to be unlikely to collide with a real
    // dev server already running on this machine.
    #[test]
    fn system_register_requires_token_then_actually_registers() {
        let home = tmp_dir("home-register");
        let project = tmp_dir("project-register");
        let (_guard, token) = spawn_server(&project, &home, 17402);

        let (status, body) = http_get(17402, "/system/register");
        assert_eq!(status, 401, "registering with no token must be rejected, got body: {body}");

        let (status, body) = http_get(17402, &format!("/system/register?token={token}"));
        assert_eq!(status, 200, "registering with the correct token must succeed, got: {body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        let canonical = project.canonicalize().unwrap().display().to_string();
        assert_eq!(v["registered"].as_str().unwrap(), canonical);

        let (status, body) = http_get(17402, "/system/list");
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        let projects = v["projects"].as_array().unwrap();
        assert!(
            projects.iter().any(|p| p["root"] == canonical),
            "expected the just-registered project in /system/list, got: {body}"
        );
    }

    #[test]
    fn system_list_and_query_need_no_token() {
        let home = tmp_dir("home-list");
        let project = tmp_dir("project-list");
        let (_guard, _token) = spawn_server(&project, &home, 17403);

        let (status, _) = http_get(17403, "/system/list");
        assert_eq!(status, 200, "GET /system/list (read-only) must not require a token");

        let (status, body) = http_get(17403, "/system/query?q=NothingRegisteredYet");
        assert_eq!(status, 200, "GET /system/query (read-only) must not require a token");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["results"].as_array().unwrap().len(), 0, "nothing registered yet, got: {body}");
    }

    #[test]
    fn system_status_needs_no_token_and_reports_real_project_counts() {
        let home = tmp_dir("home-status");
        let project = tmp_dir("project-status");
        std::fs::write(
            project.join("schema.prisma"),
            "model Widget {\n  id Int @id @default(autoincrement())\n  name String\n}\n",
        )
        .unwrap();
        // A real archietect.db on disk, exactly what `archietect init` would
        // produce — `serve` itself never persists to disk (it only keeps an
        // in-memory warm cache), and /system/status reads live from each
        // project's OWN db file, so one must actually exist here first.
        let (idx, graph) = scan::scan(&project);
        store::save(&idx, &graph, &project).unwrap();

        let (_guard, token) = spawn_server(&project, &home, 17407);

        // Register via the token-gated endpoint (real end-to-end, not a
        // direct system_db:: call) so this project actually appears in
        // system.db before querying its status.
        let (status, _) = http_get(17407, &format!("/system/register?token={token}"));
        assert_eq!(status, 200);

        let (status, body) = http_get(17407, "/system/status");
        assert_eq!(status, 200, "GET /system/status (read-only) must not require a token, got: {body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        let projects = v["projects"].as_array().unwrap();
        let canonical = project.canonicalize().unwrap().display().to_string();
        let entry = projects.iter().find(|p| p["root"] == canonical).expect("registered project must appear");
        assert_eq!(
            entry["status"]["concepts_declared"].as_u64(),
            Some(1),
            "must report this project's real concept count from its own archietect.db, got: {body}"
        );
    }

    #[test]
    fn permissions_endpoint_reports_real_domain_state() {
        let home = tmp_dir("home-permissions");
        let project = tmp_dir("project-permissions");
        let (_guard, _token) = spawn_server(&project, &home, 17404);

        let (status, body) = http_get(17404, "/permissions");
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        let domains = v["domains"].as_array().unwrap();
        let code = domains.iter().find(|d| d["domain"] == "code").unwrap();
        assert_eq!(code["allowed"], true);
        assert_eq!(code["source"], "default-enabled");
        let docker = domains.iter().find(|d| d["domain"] == "docker").unwrap();
        assert_eq!(docker["allowed"], false);
    }

    #[test]
    fn register_endpoint_needs_no_token_and_reports_real_unknowns() {
        // Labels must be unique across this test module: tmp_dir() wipes
        // and recreates its directory, and tests run in parallel.
        let home = tmp_dir("home-the-register");
        let project = tmp_dir("project-the-register");
        std::fs::write(
            project.join("schema.prisma"),
            "model Widget {\n  id Int @id @default(autoincrement())\n  name String\n}\n",
        )
        .unwrap();
        std::fs::write(project.join("handler.lua"), b"function f() end\n").unwrap();
        let (_guard, _token) = spawn_server(&project, &home, 17408);

        let (status, body) = http_get(17408, "/register");
        assert_eq!(status, 200, "GET /register (read-only) must not require a token, got: {body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        let kinds: Vec<&str> = v["not_known"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
        assert!(kinds.contains(&"unsupported_language"), "the .lua file must surface, got: {body}");
        assert!(kinds.contains(&"usage_unobserved"), "the declared-only Widget must surface, got: {body}");
        assert!(kinds.contains(&"domain_disabled"), "docker is default-disabled, got: {body}");
        // shaping applies here too: one slice, no prose
        let (status, body) = http_get(17408, "/register?only=known&compact=true");
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v.get("not_known").is_none() && v["known"]["files_scanned"].as_u64() == Some(1), "got: {body}");
    }

    #[test]
    fn documents_scan_never_hangs_and_reports_disabled_without_config() {
        let home = tmp_dir("home-documents");
        let project = tmp_dir("project-documents");
        std::fs::write(project.join("note.md"), b"should never be read").unwrap();
        let (_guard, _token) = spawn_server(&project, &home, 17405);

        let start = std::time::Instant::now();
        let (status, body) = http_get(17405, &format!("/documents/scan?dir={}", project.display()));
        let elapsed = start.elapsed();

        assert_eq!(status, 200, "got: {body}");
        assert!(
            elapsed < Duration::from_secs(2),
            "documents/scan took {elapsed:?} — should return near-instantly, never block on a confirmation prompt"
        );
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            v["enabled"], false,
            "no explicit [domains.documents] config here, so REST must report disabled rather than prompting or guessing consent — got: {body}"
        );
        assert_eq!(v["resources"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn existing_proposal_endpoints_still_token_gated_as_before() {
        // Regression guard for the original REST security fix (this
        // session, earlier): confirms adding new mutating/read endpoints
        // did not loosen the pre-existing proposal token gate.
        let home = tmp_dir("home-proposal-regression");
        let project = tmp_dir("project-proposal-regression");
        let (_guard, _token) = spawn_server(&project, &home, 17406);

        let (status, _) = http_get(17406, "/proposal/submit?kind=decision");
        assert_eq!(status, 401, "proposal/submit with no token must still be rejected");
    }
}
