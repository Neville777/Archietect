//! MCP server — the killer interface. `architect mcp [--root DIR]`
//!
//! Claude Code, Cursor, Codex, Gemini CLI all speak MCP natively, so this one
//! subcommand makes every AI coding tool on the machine a CLIENT of the
//! architectural memory: the model consults facts before writing code instead
//! of guessing architecture from a context window.
//!
//! Transport: newline-delimited JSON-RPC 2.0 over stdio (the MCP stdio
//! transport). Implemented directly — ~150 lines — rather than pulling an SDK:
//! the protocol surface used here (initialize / tools/list / tools/call) is
//! small enough to own, and a dependency-free binary is the distribution
//! story.
//!
//! Every tool takes an optional `root` argument, so ONE registered server
//! serves every repository on the machine; `--root` merely sets the default.

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;

use crate::{query, root, scan};

fn tool_defs() -> Value {
    let root_prop = json!({
        "type": "string",
        "description": "Repository root to answer about. Optional if the server was started with --root."
    });
    json!([
        {
            "name": "concept",
            "description": "ARCHITECTURAL MEMORY — call BEFORE designing or building anything. Answers: does this concept already exist in the repository, which implementation is canonical, and what is the evidence (tiered DECLARED / USED / NAMED — never invented). Verdicts: ACTIVE (extend it, do not rebuild), DECLARED_ONLY (declared but unused — confirm before extending), UNKNOWN (name resemblance only — needs human confirmation), ABSENT (building is justified).",
            "inputSchema": { "type": "object", "properties": {
                "term": { "type": "string", "description": "The concept in plain language, e.g. 'episode', 'notification', 'payment'." },
                "root": root_prop
            }, "required": ["term"] }
        },
        {
            "name": "intent",
            "description": "From a stated goal ('add session tracking') to the smallest correct change: which concepts already exist (EXTEND, with their canonical implementations), which are genuinely new (CREATE), and which need human confirmation. Call FIRST for any feature request.",
            "inputSchema": { "type": "object", "properties": {
                "text": { "type": "string", "description": "The goal in plain language." },
                "root": root_prop
            }, "required": ["text"] }
        },
        {
            "name": "impact",
            "description": "What is affected if this concept changes: files that observably use it (USED tier) and models that declare relations to it (DECLARED tier). Call before modifying any existing model or table.",
            "inputSchema": { "type": "object", "properties": {
                "term": { "type": "string" },
                "root": root_prop
            }, "required": ["term"] }
        },
        {
            "name": "guard",
            "description": "THE LAW. Check a patch or SQL snippet for CREATE TABLE statements that would duplicate an existing concept. Returns allowed:false with the canonical implementation named when a proposed table collides. Run on any patch that creates storage, BEFORE applying it.",
            "inputSchema": { "type": "object", "properties": {
                "sql": { "type": "string", "description": "The patch or SQL text to check." },
                "root": root_prop
            }, "required": ["sql"] }
        },
        {
            "name": "plan",
            "description": "ONE-CALL architectural plan for an intent ('add fraud scoring'): canonical concepts to extend with their locations, owners, governing decisions, impact severity and affected files — the composition of concept/owner/impact/decisions an agent would otherwise need five calls for. Call FIRST for any feature request; then guard the final patch.",
            "inputSchema": { "type": "object", "properties": {
                "text": { "type": "string", "description": "The goal in plain language." },
                "root": root_prop
            }, "required": ["text"] }
        },
        {
            "name": "owner",
            "description": "Who owns a concept: the directory holding its declarations (maintaining the contract is ownership; calling it is only interest), with ranked directories by declaration+usage weight.",
            "inputSchema": { "type": "object", "properties": {
                "term": { "type": "string" },
                "root": root_prop
            }, "required": ["term"] }
        },
        {
            "name": "duplicates",
            "description": "Suspected duplicate concepts in the repository: live pairs sharing a name token. Evidence of RISK, not proof — use to check whether territory is already claimed before proposing new concepts.",
            "inputSchema": { "type": "object", "properties": { "root": root_prop } }
        },
        {
            "name": "status",
            "description": "What the architectural index knows about this repository: declaration files found, concepts declared, concepts observably in use, concepts declared but never observed in use, and structural_coverage (which languages/frameworks in THIS repo Architect can actually see) — with an honest note about what the scan cannot see.",
            "inputSchema": { "type": "object", "properties": { "root": root_prop } }
        },
        {
            "name": "doctor",
            "description": "Repository summary for someone who just cloned it: domains, top concepts, recent architectural changes, decisions to read, and structural coverage. The onboarding view.",
            "inputSchema": { "type": "object", "properties": { "root": root_prop } }
        },
        {
            "name": "tour",
            "description": "Onboarding tour: important concepts, ignorable ones, and the mistakes the ontology already knows people will make (every declared alias and every rejected decision is a 'don't build X' waiting to happen) — zero generated prose.",
            "inputSchema": { "type": "object", "properties": { "root": root_prop } }
        },
        {
            "name": "history",
            "description": "The architectural timeline: what changed, when, and what the engine said about it — Git knows files changed; this knows ARCHITECTURE changed. Append-only, written only by the daemon or `architect ci`.",
            "inputSchema": { "type": "object", "properties": {
                "concept": { "type": "string", "description": "Optional — filter to events touching this concept." },
                "limit": { "type": "number", "description": "Max events to return (default 50)." },
                "root": root_prop
            } }
        },
        {
            "name": "ci",
            "description": "CI gate: check a diff/patch text for CREATE TABLE statements that duplicate an existing concept. Read-only — unlike `architect ci` on the command line, this tool call does NOT record the outcome to history (recording happens only at the actual commit-time call site).",
            "inputSchema": { "type": "object", "properties": {
                "diff": { "type": "string", "description": "The diff or patch text to check." },
                "strict": { "type": "boolean", "description": "Also fail on name-collision warnings, not only storage violations." },
                "root": root_prop
            }, "required": ["diff"] }
        }
    ])
}

pub fn serve(default_root: Option<PathBuf>) -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    // Warm cache across the whole MCP session, same fix and same reasoning
    // as rest.rs: an agent composing one architectural answer easily makes
    // five tool calls (concept, owner, impact, decisions via plan) in a row.
    // Without this, each one was a full cold scan — five times the cost for
    // one question. stdin is read one line at a time, sequentially, so a
    // plain HashMap needs no lock here either.
    let mut cache: std::collections::HashMap<PathBuf, (crate::model::Index, crate::structural::StructuralGraph)> = std::collections::HashMap::new();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");

        // Notifications (no id) get no response.
        if id.is_none() {
            continue;
        }
        let id = id.unwrap();

        let result: Result<Value, (i64, String)> = match method {
            "initialize" => Ok(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "architect", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Architectural memory for this machine's repositories. Call `concept` before designing anything, `intent` for feature requests, `impact` before modifying models, `guard` on any patch that creates tables. Answers are deterministic facts with tiered evidence (DECLARED/USED/NAMED) — reason on top of them; do not override them with intuition."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_defs() })),
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or("");
                let args = &msg["params"]["arguments"];
                let root = root::resolve(
                    args.get("root").and_then(|r| r.as_str()).map(PathBuf::from)
                        .or_else(|| default_root.clone()),
                    &std::env::current_dir().unwrap_or_default(),
                ).ok();
                match root {
                    None => Err((-32602i64, "no repository root: pass `root` or start the server with --root".to_string())),
                    Some(root) if !root.exists() => {
                        Err((-32602i64, format!("root does not exist: {}", root.display())))
                    }
                    Some(root) => {
                        let prior = cache.remove(&root);
                        let (schema_prior, graph_prior) = match prior {
                            Some((s, g)) => (Some(s), Some(g)),
                            None => (None, None),
                        };
                        let (idx, graph) = scan::scan_with_prior(&root, schema_prior, graph_prior);
                        let out = match name {
                            "concept" => query::concept(&idx, &graph, args["term"].as_str().unwrap_or("")),
                            "intent" => query::intent(&idx, args["text"].as_str().unwrap_or("")),
                            "impact" => query::impact(&idx, &graph, args["term"].as_str().unwrap_or("")),
                            "guard" => query::guard(&idx, args["sql"].as_str().unwrap_or("")),
                            "plan" => query::plan(&idx, &graph, args["text"].as_str().unwrap_or("")),
                            "owner" => query::owner(&idx, args["term"].as_str().unwrap_or("")),
                            "duplicates" => query::duplicates(&idx),
                            "status" => query::status(&idx, &graph),
                            "doctor" => query::doctor(&idx, &graph, &root),
                            "tour" => query::tour(&idx, &graph),
                            "history" => json!({
                                "events": crate::store::read_history(
                                    &root,
                                    args.get("concept").and_then(|c| c.as_str()),
                                    args.get("limit").and_then(|l| l.as_u64()).unwrap_or(50) as usize,
                                ),
                                "note": "Append-only architectural timeline, newest first, written only by the daemon or `architect ci`.",
                            }),
                            "ci" => query::ci(&idx, args["diff"].as_str().unwrap_or(""), args.get("strict").and_then(|s| s.as_bool()).unwrap_or(false)),
                            other => json!({ "error": format!("unknown tool {other}") }),
                        };
                        cache.insert(root, (idx, graph));
                        Ok(json!({
                            "content": [ { "type": "text", "text": serde_json::to_string_pretty(&out)? } ],
                            "isError": false
                        }))
                    }
                }
            }
            other => Err((-32601i64, format!("method not found: {other}"))),
        };

        let response = match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, message)) => json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": code, "message": message }
            }),
        };
        writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
        stdout.flush()?;
    }
    Ok(())
}
