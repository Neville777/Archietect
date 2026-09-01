//! REST API — `architect serve [--root DIR] [--port 7373]`.
//!
//! A thin transport over the same engine the CLI and MCP expose: no business
//! logic lives here, ever. The GUI (when it exists), CI checks, dashboards,
//! and anything HTTP-shaped become clients of the same deterministic answers
//! — GitHub Desktop is a client of git, never a fork of it.
//!
//! Read-only by design: every endpoint is a query; nothing here writes
//! architect.db. The daemon remains the single writer, so REST can serve any
//! number of concurrent readers without coordination.
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
//! by dogfooding: on TITAN (1,483 files) with no persisted architect.db,
//! that meant every single HTTP request paid the full 11+ SECOND cold-scan
//! cost, forever — a "server" that was never actually warm.
//!
//! The fix mirrors what the daemon already does, scoped to this process:
//! keep the last built `Index` per root IN MEMORY and pass it as `prior` to
//! `scan_with_prior`. The first request for a root still pays scan cost
//! (once); every request after that is incremental — only files whose
//! (size, mtime) changed get re-parsed, the same guarantee `architect
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

pub fn serve(default_root: Option<PathBuf>, port: u16) -> anyhow::Result<()> {
    let server = tiny_http::Server::http(("127.0.0.1", port))
        .map_err(|e| anyhow::anyhow!("bind 127.0.0.1:{port}: {e}"))?;
    eprintln!("architect REST listening on http://127.0.0.1:{port} (read-only)");

    // The warm cache. Single-threaded loop, one request at a time — plain
    // HashMap, no lock needed.
    let mut cache: HashMap<PathBuf, (Index, StructuralGraph)> = HashMap::new();

    for req in server.incoming_requests() {
        let (path, p) = params(req.url());
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

        let body: Value = match (path.as_str(), root) {
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
                        other => json!({
                            "error": format!("unknown endpoint {other}"),
                            "endpoints": ["/concept", "/intent", "/impact", "/owner", "/guard", "/plan",
                                          "/status", "/doctor", "/tour", "/duplicates",
                                          "/history", "/ci", "/laws"],
                        }),
                    }
                };
                cache.insert(root, (idx, graph));
                result
            }
        };

        let data = serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into());
        let response = tiny_http::Response::from_string(data)
            .with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .unwrap(),
            )
            // future GUI is a browser client of this API — same-machine only
            // (bound to 127.0.0.1), so permissive CORS is safe here
            .with_header(
                tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..])
                    .unwrap(),
            );
        let _ = req.respond(response);
    }
    Ok(())
}
