//! MCP server — the killer interface. `archietect mcp [--root DIR]`
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

use crate::{documents_domain, permissions, query, root, scan, system_db};

fn tool_defs() -> Value {
    let mut tools = tool_defs_inner();
    // Every tool accepts the two output-shaping arguments (src/shape.rs):
    // `only` selects top-level keys of the result, `compact` drops
    // explanatory prose. Advertised generically here rather than repeated in
    // each literal below, so a new tool can't forget them.
    if let Some(arr) = tools.as_array_mut() {
        for t in arr {
            if let Some(props) = t.pointer_mut("/inputSchema/properties").and_then(|p| p.as_object_mut()) {
                props.insert("only".to_string(), json!({
                    "type": "array", "items": { "type": "string" },
                    "description": "Return only these top-level keys of the result (e.g. [\"git\"] on `status`). Saves tokens when you need one slice of a large answer."
                }));
                props.insert("compact".to_string(), json!({
                    "type": "boolean",
                    "description": "Drop explanatory prose fields (`note`, `evidence_note`) from the result. Evidence, tiers, files and lines are all kept."
                }));
            }
        }
    }
    tools
}

fn tool_defs_inner() -> Value {
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
            "name": "imports",
            "description": "What does this exact file import, and what imports it — but ONLY exact, unambiguous relative-import resolutions (e.g. './services/foo' resolving to a real scanned file). An external package import or an import that matched more than one scanned file is correctly reported as nothing, never a guess. Not folded into `status` — a full import graph is too large to return on every call; ask about one file at a time.",
            "inputSchema": { "type": "object", "properties": {
                "file": { "type": "string", "description": "Repository-relative path, e.g. src/services/foo.ts" },
                "root": root_prop
            }, "required": ["file"] }
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
            "description": "What the architectural index knows about this repository: declaration files found, concepts declared, concepts observably in use, concepts declared but never observed in use, and structural_coverage (which languages/frameworks in THIS repo Archietect can actually see) — with an honest note about what the scan cannot see.",
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
            "description": "The architectural timeline: what changed, when, and what the engine said about it — Git knows files changed; this knows ARCHITECTURE changed. Append-only, written only by the daemon or `archietect ci`.",
            "inputSchema": { "type": "object", "properties": {
                "concept": { "type": "string", "description": "Optional — filter to events touching this concept." },
                "limit": { "type": "number", "description": "Max events to return (default 50)." },
                "root": root_prop
            } }
        },
        {
            "name": "ci",
            "description": "CI gate: check a diff/patch text for CREATE TABLE statements that duplicate an existing concept. Read-only — unlike `archietect ci` on the command line, this tool call does NOT record the outcome to history (recording happens only at the actual commit-time call site).",
            "inputSchema": { "type": "object", "properties": {
                "diff": { "type": "string", "description": "The diff or patch text to check." },
                "strict": { "type": "boolean", "description": "Also fail on name-collision warnings, not only storage violations." },
                "root": root_prop
            }, "required": ["diff"] }
        },
        {
            "name": "proposal_submit",
            "description": "AI-EXTENSION PROTOCOL, step 1 of 3. Register a new proposal (a unified diff) as pending — writes only under .archietect/proposals/, never touches the real working tree. An extractor proposal may only touch src/structural.rs, tests/fixtures/**, validation/**; a decision/alias proposal may only touch archietect.toml. Call `proposal_test` next.",
            "inputSchema": { "type": "object", "properties": {
                "kind": { "type": "string", "enum": ["extractor", "decision", "alias"] },
                "title": { "type": "string" },
                "description": { "type": "string" },
                "lang": { "type": "string", "description": "Language name, for an extractor proposal." },
                "preview_repo": { "type": "string", "description": "A real repo (path) to preview an extractor against — informational only." },
                "patch": { "type": "string", "description": "The unified diff (git diff format) text." },
                "root": root_prop
            }, "required": ["kind", "title", "patch"] }
        },
        {
            "name": "proposal_list",
            "description": "AI-EXTENSION PROTOCOL. List all proposals and their status (pending/passed/failed/accepted/rejected).",
            "inputSchema": { "type": "object", "properties": { "root": root_prop } }
        },
        {
            "name": "proposal_inspect",
            "description": "AI-EXTENSION PROTOCOL. Show one proposal's metadata, patch text, and last test result.",
            "inputSchema": { "type": "object", "properties": {
                "id": { "type": "number" },
                "root": root_prop
            }, "required": ["id"] }
        },
        {
            "name": "proposal_test",
            "description": "AI-EXTENSION PROTOCOL, step 2 of 3. Apply the patch in an ISOLATED git worktree and run the real regression suite against it (laws + invariants for an extractor; invariants::check for a decision/alias). Never touches the real working tree or archietect.db. This is the only thing that can turn a proposal 'passed' — nothing here writes a fact.",
            "inputSchema": { "type": "object", "properties": {
                "id": { "type": "number" },
                "root": root_prop
            }, "required": ["id"] }
        },
        {
            "name": "proposal_accept",
            "description": "AI-EXTENSION PROTOCOL, step 3 of 3 — HUMAN-GATED. Applies a proposal to the REAL working tree, UNCOMMITTED, but only if: status is 'passed', the patch is byte-identical to what was tested, and the repository HEAD has not moved since. Never runs `git commit`. Prefer running this from the CLI yourself rather than calling it as a tool — accepting your own AI's proposal without a human actually looking at the diff first defeats the point of the gate.",
            "inputSchema": { "type": "object", "properties": {
                "id": { "type": "number" },
                "root": root_prop
            }, "required": ["id"] }
        },
        {
            "name": "proposal_reject",
            "description": "AI-EXTENSION PROTOCOL. Mark a proposal rejected (kept for audit trail unless purge is set).",
            "inputSchema": { "type": "object", "properties": {
                "id": { "type": "number" },
                "purge": { "type": "boolean", "description": "Delete the proposal's files instead of just marking it rejected." },
                "root": root_prop
            }, "required": ["id"] }
        },
        {
            "name": "register",
            "description": "THE MAP OF THE BAG — call this before trusting any ABSENT. What this memory knows about the repository (counts, enabled domains), what it does NOT know and WHY (`not_known`: unsupported languages with the exact files, disabled or unconfirmed domains, evidence tiers no extractor can produce — e.g. whether a declared docker service is actually running — and declared concepts never observed in use), each with how to establish the fact without archietect, plus the permission boundary including whether a human actually confirmed each unstructured domain. Distinguishes 'X does not exist' from 'X cannot be established here'.",
            "inputSchema": { "type": "object", "properties": { "root": root_prop } }
        },
        {
            "name": "permissions",
            "description": "Inspect the resolved domain permission state for this repository: which domains (code, git, docker, systemd, photos, messages, documents, browser) are enabled and WHERE that decision came from (project config / global config / default), plus the hardcoded denial list (.ssh, .aws, credential files, browser profiles, ...) nothing can ever override.",
            "inputSchema": { "type": "object", "properties": { "root": root_prop } }
        },
        {
            "name": "permissions_check",
            "description": "Check ONE path against the permission boundary and get a reason, allowed or not — hardcoded denials (.ssh, .aws, credential/secret filenames, browser profiles) are checked first and are never overridable by any config. This is the call a pre-tool-use hook makes before letting a Read/Edit/Write land.",
            "inputSchema": { "type": "object", "properties": {
                "path": { "type": "string", "description": "Path to check (absolute, or relative to root)." },
                "domain": { "type": "string", "description": "Domain this path is being accessed under. Defaults to \"code\"." },
                "root": root_prop
            }, "required": ["path"] }
        },
        {
            "name": "system_list",
            "description": "SYSTEM MEMORY. List every project registered in the machine-wide pointer registry (~/.archietect/system.db) — root path, display name, and when it was first/last registered. Stores pointers only, never any project's actual architectural facts.",
            "inputSchema": { "type": "object", "properties": { "root": root_prop } }
        },
        {
            "name": "system_query",
            "description": "SYSTEM MEMORY. \"Which of my registered projects has X?\" — fans out a concept lookup live, read-only, over every registered project's OWN archietect.db (never cached into system.db). A registered project with no archietect.db yet (moved, deleted, or never `init`'d) is reported honestly rather than skipped.",
            "inputSchema": { "type": "object", "properties": {
                "term": { "type": "string", "description": "The concept in plain language, checked against every registered project." },
                "root": root_prop
            }, "required": ["term"] }
        },
        {
            "name": "system_status",
            "description": "SYSTEM MEMORY. \"What do I have?\" — fans out a full status summary (counts, git, docker, same_project_as) live, read-only, over every registered project's OWN archietect.db (never cached into system.db). A registered project with no archietect.db yet (moved, deleted, or never `init`'d) is reported honestly rather than skipped.",
            "inputSchema": { "type": "object", "properties": { "root": root_prop } }
        },
        {
            "name": "system_register",
            "description": "SYSTEM MEMORY. Register this repository (the resolved root) in the machine-wide pointer registry (~/.archietect/system.db). Safe to re-run: updates last-seen, never duplicates the entry or resets when it was first registered. Writes only a root path, name, and timestamps — never any architectural fact.",
            "inputSchema": { "type": "object", "properties": { "root": root_prop } }
        },
        {
            "name": "documents_scan",
            "description": "FIRST UNSTRUCTURED DOMAIN. Scan one explicit directory for document files (.pdf/.docx/.txt/.md/.odt), non-recursive — filename/extension/size/modified-time only, content never read. Requires the 'documents' domain to already be explicitly enabled via [domains.documents] in archietect.toml or ~/.archietect/system.toml for this repository: over MCP this tool can never prompt for the one-time confirmation the CLI (`archietect documents scan`) can, so an unconfigured repository always reports enabled:false here rather than hanging or guessing consent.",
            "inputSchema": { "type": "object", "properties": {
                "dir": { "type": "string", "description": "Absolute path to the directory to scan." },
                "root": root_prop
            }, "required": ["dir"] }
        }
    ])
}

pub fn serve(default_root: Option<PathBuf>) -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    // Snapshot once at startup — see `crate::exe_mtime`'s doc comment. This
    // is a stdio server that can live for hours across many rebuilds of the
    // very binary it's running; checked on every tool call because the
    // failure mode this guards is "already stale mid-session," not just
    // "started stale."
    let started_mtime = crate::exe_mtime();
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
                "serverInfo": { "name": "archietect", "version": env!("CARGO_PKG_VERSION") },
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
                        let mut out = match name {
                            "concept" => query::concept(&idx, &graph, args["term"].as_str().unwrap_or("")),
                            "intent" => query::intent(&idx, args["text"].as_str().unwrap_or("")),
                            "impact" => query::impact(&idx, &graph, args["term"].as_str().unwrap_or("")),
                            "imports" => query::imports(&graph, args["file"].as_str().unwrap_or("")),
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
                                "note": "Append-only architectural timeline, newest first, written only by the daemon or `archietect ci`.",
                            }),
                            "ci" => query::ci(&idx, args["diff"].as_str().unwrap_or(""), args.get("strict").and_then(|s| s.as_bool()).unwrap_or(false)),
                            "proposal_submit" => {
                                let kind_str = args["kind"].as_str().unwrap_or("");
                                match serde_json::from_value::<crate::proposal::Kind>(json!(kind_str)) {
                                    Err(_) => json!({ "error": format!("unknown proposal kind '{kind_str}' — expected extractor, decision, or alias") }),
                                    Ok(kind) => {
                                        let patch_text = args["patch"].as_str().unwrap_or("");
                                        let tmp = std::env::temp_dir().join(format!("archietect-mcp-proposal-{}.diff", std::process::id()));
                                        match std::fs::write(&tmp, patch_text) {
                                            Err(e) => json!({ "error": format!("failed to stage patch: {e}") }),
                                            Ok(()) => {
                                                let out = crate::proposal::submit(
                                                    &root, kind,
                                                    args["title"].as_str().unwrap_or(""),
                                                    args.get("description").and_then(|d| d.as_str()).unwrap_or(""),
                                                    args.get("lang").and_then(|l| l.as_str()),
                                                    args.get("preview_repo").and_then(|p| p.as_str()),
                                                    "ai",
                                                    &tmp,
                                                );
                                                let _ = std::fs::remove_file(&tmp);
                                                match out {
                                                    Ok(v) => v,
                                                    Err(e) => json!({ "error": e.to_string() }),
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            "proposal_list" => crate::proposal::list(&root),
                            "proposal_inspect" => match crate::proposal::inspect(&root, args["id"].as_u64().unwrap_or(0)) {
                                Ok(v) => v,
                                Err(e) => json!({ "error": e.to_string() }),
                            },
                            "proposal_test" => match crate::proposal::test(&root, args["id"].as_u64().unwrap_or(0)) {
                                Ok(v) => v,
                                Err(e) => json!({ "error": e.to_string() }),
                            },
                            "proposal_accept" => match crate::proposal::accept(&root, args["id"].as_u64().unwrap_or(0)) {
                                Ok(v) => v,
                                Err(e) => json!({ "error": e.to_string() }),
                            },
                            "proposal_reject" => match crate::proposal::reject(&root, args["id"].as_u64().unwrap_or(0), args.get("purge").and_then(|p| p.as_bool()).unwrap_or(false)) {
                                Ok(v) => v,
                                Err(e) => json!({ "error": e.to_string() }),
                            },
                            "permissions" => match permissions::default_global_config_path().and_then(|g| permissions::load(&g, &root)) {
                                Ok(cfg) => permissions::report(&cfg),
                                Err(e) => json!({ "error": e.to_string() }),
                            },
                            "permissions_check" => match permissions::default_global_config_path().and_then(|g| permissions::load(&g, &root)) {
                                Ok(cfg) => {
                                    let path_str = args["path"].as_str().unwrap_or("");
                                    let domain = args["domain"].as_str().unwrap_or("code");
                                    let candidate = PathBuf::from(path_str);
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
                            "register" => crate::register::register(&idx, &graph, &root),
                            // root is required here purely because every MCP
                            // tool call in this server goes through the same
                            // Some(root) dispatch gate above — system_list
                            // itself never reads or needs that root, it
                            // answers from ~/.archietect/system.db alone
                            // (see REST's /system/list, which — unlike this
                            // MCP tool — genuinely needs no root at all,
                            // since rest.rs's dispatch has a root-independent
                            // path this server does not).
                            "system_list" => match system_db::default_db_path().and_then(|db| system_db::list_projects(&db).map(|p| (db, p))) {
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
                            "system_query" => {
                                let term = args["term"].as_str().unwrap_or("");
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
                            "system_status" => {
                                match system_db::default_db_path().and_then(|db| system_db::status_registered_projects(&db).map(|r| (db, r))) {
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
                                }
                            }
                            // MCP's trust boundary is "whoever spawned this
                            // process" (see rest.rs's module doc, which
                            // documents the REST token gate by contrast) —
                            // no token needed here, unlike REST's
                            // /system/register.
                            "system_register" => match system_db::default_db_path().and_then(|db| system_db::register_project(&db, &root).map(|proj| (db, proj))) {
                                Ok((db_path, proj)) => json!({
                                    "registered": proj.root,
                                    "name": proj.name,
                                    "first_registered_ms": proj.first_registered_ms,
                                    "last_seen_ms": proj.last_seen_ms,
                                    "system_db": db_path.display().to_string(),
                                }),
                                Err(e) => json!({ "error": e.to_string() }),
                            },
                            // Always NonInteractiveAsker — see tool_defs()'s
                            // description and rest.rs's matching endpoint doc:
                            // MCP has no real stdin to prompt against, so this
                            // must never block waiting for a y/N that can
                            // never come. Only ever returns real data for a
                            // repository that already has [domains.documents]
                            // explicitly configured.
                            "documents_scan" => {
                                let dir_str = args["dir"].as_str().unwrap_or("");
                                if dir_str.is_empty() {
                                    json!({ "error": "missing required 'dir' argument" })
                                } else {
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
                            }
                            other => json!({ "error": format!("unknown tool {other}") }),
                        };
                        cache.insert(root, (idx, graph));
                        if let (Some(started), Some(now)) = (started_mtime, crate::exe_mtime()) {
                            if now != started {
                                if let Value::Object(ref mut map) = out {
                                    map.insert("_stale_binary_warning".to_string(), json!(
                                        "This MCP server process has been running since before the archietect binary on disk was last rebuilt — it is answering from OLD code in memory. Restart this session (or otherwise force your MCP client to respawn the 'archietect' server) to pick up the current build."
                                    ));
                                }
                            }
                        }
                        // Output shaping (src/shape.rs). `only` may arrive as a
                        // JSON array of strings or a comma-separated string.
                        let only: Option<Vec<String>> = match &args["only"] {
                            Value::Array(items) => {
                                let keys: Vec<String> = items.iter().filter_map(|x| x.as_str().map(String::from)).collect();
                                if keys.is_empty() { None } else { Some(keys) }
                            }
                            Value::String(s) => crate::shape::parse_only(Some(s)),
                            _ => None,
                        };
                        let compact = args["compact"].as_bool().unwrap_or(false);
                        let out = crate::shape::apply(out, only.as_deref(), compact);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    /// Kills the spawned `archietect mcp` child even if an assertion panics
    /// mid-test.
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
        let p = std::env::temp_dir().join(format!("archietect-mcp-test-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// Spawns a REAL `archietect mcp` subprocess (the actual compiled
    /// binary, exercised over its real stdio JSON-RPC transport — not
    /// mcp.rs's dispatch called in-process) with its own isolated HOME, so
    /// it never touches the real machine's ~/.archietect/system.db. Returns
    /// a guard, the child's stdin, and a channel yielding each stdout line
    /// as it arrives: decoupling reads from a fixed timeout means a
    /// hung/broken server fails a test loudly instead of blocking it
    /// forever.
    fn spawn_mcp(project_root: &Path, home: &Path) -> (ChildGuard, std::process::ChildStdin, mpsc::Receiver<String>) {
        let mut child = Command::new(bin_path())
            .args(["mcp", "--root", project_root.to_str().unwrap()])
            .env("HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn archietect mcp — is target/release/archietect built?");

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().flatten() {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        (ChildGuard(child), stdin, rx)
    }

    fn call(
        stdin: &mut std::process::ChildStdin,
        rx: &mpsc::Receiver<String>,
        id: u64,
        method: &str,
        params: Value,
    ) -> Value {
        let req = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(stdin, "{req}").unwrap();
        stdin.flush().unwrap();
        let line = rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("no MCP response to '{method}' within 5s — possible hang"));
        serde_json::from_str(&line).unwrap()
    }

    /// A `tools/call` response wraps its actual JSON payload as a STRING
    /// inside `result.content[0].text` (see this file's own `tools/call`
    /// handler above) — unwrap that one extra layer.
    fn tool_result(resp: &Value) -> Value {
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("tool response missing content[0].text: {resp}"));
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn new_tools_are_registered_and_return_real_data() {
        let home = tmp_dir("home");
        let project = tmp_dir("project");
        let (_guard, mut stdin, rx) = spawn_mcp(&project, &home);

        let _ = call(&mut stdin, &rx, 1, "initialize", json!({}));

        let list_resp = call(&mut stdin, &rx, 2, "tools/list", json!({}));
        let names: Vec<&str> = list_resp["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        for expected in ["permissions", "register", "system_list", "system_query", "system_status", "system_register", "documents_scan"] {
            assert!(names.contains(&expected), "expected tool '{expected}' in tools/list, got: {names:?}");
        }

        let register_resp = call(&mut stdin, &rx, 3, "tools/call", json!({ "name": "system_register", "arguments": {} }));
        let registered = tool_result(&register_resp);
        let canonical = project.canonicalize().unwrap().display().to_string();
        assert_eq!(registered["registered"].as_str().unwrap(), canonical);

        let list_resp = call(&mut stdin, &rx, 4, "tools/call", json!({ "name": "system_list", "arguments": {} }));
        let listed = tool_result(&list_resp);
        let projects = listed["projects"].as_array().unwrap();
        assert!(
            projects.iter().any(|p| p["root"] == canonical),
            "expected the just-registered project in system_list, got: {listed}"
        );

        let query_resp = call(
            &mut stdin, &rx, 5, "tools/call",
            json!({ "name": "system_query", "arguments": { "term": "Anything" } }),
        );
        let queried = tool_result(&query_resp);
        assert_eq!(queried["results"].as_array().unwrap().len(), 1, "expected exactly the one registered project, got: {queried}");

        let status_resp = call(&mut stdin, &rx, 7, "tools/call", json!({ "name": "system_status", "arguments": {} }));
        let statuses = tool_result(&status_resp);
        let status_projects = statuses["projects"].as_array().unwrap();
        assert_eq!(status_projects.len(), 1, "expected exactly the one registered project, got: {statuses}");
        assert!(
            status_projects[0]["status"].is_null(),
            "this project was registered but never `init`'d, so its status must honestly report null, not fabricate counts — got: {statuses}"
        );

        // The map of the bag, with shaping: one slice, no prose. This
        // project has no schema and no unclassified files, so the only
        // unknowns are the domain-level ones — docker disabled by default.
        let reg_resp = call(
            &mut stdin, &rx, 8, "tools/call",
            json!({ "name": "register", "arguments": { "only": ["not_known", "known"], "compact": true } }),
        );
        let reg = tool_result(&reg_resp);
        assert!(reg.get("boundary").is_none(), "`only` must drop unselected keys, got: {reg}");
        assert!(reg.get("note").is_none(), "`compact` must drop prose, got: {reg}");
        assert!(
            reg["not_known"].as_array().unwrap().iter().any(|e| e["kind"] == "domain_disabled" && e["domain"] == "docker"),
            "docker is default-disabled and must be stated as not looked at, got: {reg}"
        );
        assert_eq!(reg["known"]["domains_enabled"], json!(["code", "git"]), "got: {reg}");

        let perms_resp = call(&mut stdin, &rx, 6, "tools/call", json!({ "name": "permissions", "arguments": {} }));
        let perms = tool_result(&perms_resp);
        assert!(
            perms["domains"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["domain"] == "code" && d["allowed"] == true),
            "expected code to show as allowed in permissions, got: {perms}"
        );
    }

    #[test]
    fn documents_scan_tool_never_hangs_and_reports_disabled_without_config() {
        let home = tmp_dir("home-docs");
        let project = tmp_dir("project-docs");
        std::fs::write(project.join("note.md"), b"should never be read").unwrap();
        let (_guard, mut stdin, rx) = spawn_mcp(&project, &home);
        let _ = call(&mut stdin, &rx, 1, "initialize", json!({}));

        let start = std::time::Instant::now();
        let resp = call(
            &mut stdin, &rx, 2, "tools/call",
            json!({ "name": "documents_scan", "arguments": { "dir": project.to_str().unwrap() } }),
        );
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_secs(2),
            "documents_scan took {elapsed:?} — must never block waiting for a confirmation prompt over MCP"
        );
        let out = tool_result(&resp);
        assert_eq!(
            out["enabled"], false,
            "no explicit config for 'documents' in this project, so MCP must report disabled — got: {out}"
        );
    }
}
