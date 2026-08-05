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
//!   GET /history[?q=concept][&limit=50]
//!
//! `root` may come per-request or from --root at startup, same contract as
//! the MCP server: one process can serve every repository on the machine.

use serde_json::{json, Value};
use std::path::PathBuf;

use crate::{laws, query, scan, store};

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

    for req in server.incoming_requests() {
        let (path, p) = params(req.url());
        let root = p
            .get("root")
            .map(PathBuf::from)
            .or_else(|| default_root.clone());

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
                let q = p.get("q").map(|s| s.as_str()).unwrap_or("");
                match ep {
                    "/concept" => query::concept(&scan::scan(&root), q),
                    "/intent" => query::intent(&scan::scan(&root), q),
                    "/impact" => query::impact(&scan::scan(&root), q),
                    "/owner" => query::owner(&scan::scan(&root), q),
                    "/guard" => query::guard(
                        &scan::scan(&root),
                        p.get("sql").map(|s| s.as_str()).unwrap_or(""),
                    ),
                    "/status" => query::status(&scan::scan(&root)),
                    "/doctor" => query::doctor(&scan::scan(&root), &root),
                    "/tour" => query::tour(&scan::scan(&root)),
                    "/duplicates" => query::duplicates(&scan::scan(&root)),
                    "/history" => json!({
                        "events": store::read_history(
                            &root,
                            p.get("q").map(|s| s.as_str()),
                            p.get("limit").and_then(|l| l.parse().ok()).unwrap_or(50),
                        )
                    }),
                    other => json!({
                        "error": format!("unknown endpoint {other}"),
                        "endpoints": ["/concept", "/intent", "/impact", "/owner", "/guard",
                                      "/status", "/doctor", "/tour", "/duplicates",
                                      "/history", "/laws"],
                    }),
                }
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
