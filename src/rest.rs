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
//! cross-origin GET with no confirmation. So the four write endpoints
//! (`submit`/`test`/`accept`/`reject`) additionally require `&token=...`,
//! printed once to stderr when `serve` starts; nothing else on the machine
//! can guess it. Responses carry no `Access-Control-Allow-Origin` header —
//! the embedded GUI at `/` is same-origin and needs none.
//!
//!   GET /concept?q=invoice[&root=/path]      GET /doctor
//!   GET /intent?q=add+invoicing              GET /tour
//!   GET /impact?q=payments                   GET /duplicates
//!   GET /owner?q=invoice                     GET /status
//!   GET /guard?sql=CREATE+TABLE+...          GET /laws
//!   GET /plan?q=add+invoicing                GET /ci?diff=...[&strict=true]
//!   GET /history[?q=concept][&limit=50]
//!
//! `root` may come per-request or from --root at startup, same contract as
//! the MCP server: one process can serve every repository on the machine.
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

use crate::{laws, model::Index, query, root, scan, store, structural::StructuralGraph};

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
];

pub fn serve(default_root: Option<PathBuf>, port: u16) -> anyhow::Result<()> {
    let server = tiny_http::Server::http(("127.0.0.1", port))
        .map_err(|e| anyhow::anyhow!("bind 127.0.0.1:{port}: {e}"))?;
    let token = generate_token();
    eprintln!("archietect REST listening on http://127.0.0.1:{port} (read-only except /proposal/*)");
    eprintln!("proposal-mutating requests (submit/test/accept/reject) require &token={token}");

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
                        "/owner" => query::owner(&idx, q),
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
                        other => json!({
                            "error": format!("unknown endpoint {other}"),
                            "endpoints": ["/concept", "/intent", "/impact", "/owner", "/guard", "/plan",
                                          "/status", "/doctor", "/tour", "/duplicates",
                                          "/history", "/ci", "/laws",
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
