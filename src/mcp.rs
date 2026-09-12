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
use std::path::{Path, PathBuf};

use crate::{docker_domain, documents_domain, permissions, photos_domain, query, root, scan, system_db};

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

/// "asking about: concept" said WHICH tool ran, not what it was actually
/// asked — a category, not information; called out directly as meaning
/// nothing on its own. This pulls the one argument that actually identifies
/// what a call was ABOUT, per tool (field names copied from this file's own
/// `tool_defs_inner` below, not guessed), and formats "tool(argument)" —
/// e.g. "concept(PaymentService)" instead of "concept". Tools with no
/// identifying argument (doctor, tour, verdicts, ...) fall back to the bare
/// name; there's nothing more specific to say about those. Long free-text
/// arguments (`guard`'s sql, `ci`'s diff) are truncated — this is a status
/// display, not a place to reproduce an entire patch.
fn describe_call(name: &str, args: &Value) -> String {
    fn field(args: &Value, key: &str) -> Option<String> {
        args.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string())
    }
    fn truncate(s: &str, max: usize) -> String {
        if s.chars().count() <= max {
            s.to_string()
        } else {
            format!("{}…", s.chars().take(max).collect::<String>())
        }
    }
    let arg = match name {
        "concept" | "impact" | "owner" | "system_query" => field(args, "term"),
        "intent" | "plan" => field(args, "text"),
        "imports" => field(args, "file"),
        "guard" => field(args, "sql").map(|s| truncate(&s, 60)),
        "claim" => field(args, "statement").map(|s| truncate(&s, 80)),
        "ci" => field(args, "diff").map(|s| truncate(&s, 60)),
        "history" => field(args, "concept"),
        "proposal_submit" => field(args, "title"),
        "proposal_inspect" | "proposal_test" | "proposal_accept" | "proposal_reject" => {
            args.get("id").and_then(|v| v.as_i64()).map(|n| n.to_string())
        }
        "permissions_check" => field(args, "path"),
        "documents_scan" | "photos_scan" => field(args, "dir"),
        _ => None,
    };
    match arg {
        Some(a) => format!("{name}({a})"),
        None => name.to_string(),
    }
}

/// Shared, mutex-protected AI-activity state — read/written by both the
/// main stdin-processing loop and the background flush thread below (see
/// `spawn_activity_flusher`). A plain per-call HashMap sufficed as long as
/// the only thing that could trigger a flush was another tool call arriving
/// late enough — which is exactly the bug this replaces: a session's LAST
/// call before going quiet never flushed at all, because nothing ever
/// arrived afterward to notice. Reported live: `duplicates` then
/// `impact(akt_wallet_state)` back to back — `duplicates` happened to cross
/// the old interval and flushed (itself plus whatever came before), but
/// `impact` started a fresh, empty accumulator that no third call ever
/// came along to flush. It would have sat there forever.
#[derive(Default)]
struct McpActivity {
    client_info: Option<(String, String)>,
    recorded_connection_for: std::collections::HashSet<PathBuf>,
    tools_since_heartbeat: std::collections::HashMap<PathBuf, std::collections::BTreeSet<String>>,
    last_heartbeat_for: std::collections::HashMap<PathBuf, i64>,
}

/// At most one history write per root per this many ms — bounds writes
/// during a rapid burst of tool calls without meaningfully delaying the
/// "an AI is using this" signal a human is actually watching for in the
/// GUI (which itself polls every 15s — a few seconds of write-throttling
/// underneath that is invisible; the old 60s value was not).
const HEARTBEAT_INTERVAL_MS: i64 = 3_000;

/// The one place either the main loop or the background flusher actually
/// appends to history, so the two can't diverge on what "due" means.
/// Writes `root`'s pending activity as `mcp_client_connected` (first
/// contact) or `mcp_client_active` (thereafter), then clears it. Registers
/// the project into system.db on first contact — see
/// `mcp_client_connected`'s own doc for why that lives here rather than
/// requiring a separate manual `system_register` call.
fn flush_if_due(activity: &mut McpActivity, root: &Path, now: i64) {
    let Some(tools) = activity.tools_since_heartbeat.get(root) else { return };
    if tools.is_empty() {
        return;
    }
    let due = activity
        .last_heartbeat_for
        .get(root)
        .map(|last| now - last >= HEARTBEAT_INTERVAL_MS)
        .unwrap_or(true);
    if !due {
        return;
    }
    let Some((client_name, client_version)) = activity.client_info.clone() else { return };
    let tools: Vec<String> = tools.iter().cloned().collect();
    let first_contact = activity.recorded_connection_for.insert(root.to_path_buf());
    let kind = if first_contact { "mcp_client_connected" } else { "mcp_client_active" };
    let _ = crate::store::append_events(root, &[(
        now,
        kind.to_string(),
        client_name,
        json!({ "version": client_version, "tools": tools }).to_string(),
    )]);
    activity.last_heartbeat_for.insert(root.to_path_buf(), now);
    if let Some(t) = activity.tools_since_heartbeat.get_mut(root) {
        t.clear();
    }
    if first_contact {
        if let Ok(db_path) = system_db::default_db_path() {
            let _ = system_db::register_project(&db_path, root);
        }
    }
}

/// The fix for "the last tool call in a session never shows up": a new
/// call arriving used to be the ONLY thing that ever rechecked whether a
/// flush was due. This thread rechecks every root with pending activity on
/// a timer instead, so a flush happens within HEARTBEAT_INTERVAL_MS of the
/// last call regardless of whether anything else ever arrives after it.
/// 500ms tick: fine-grained enough that the GUI's own 15s poll never
/// visibly waits on this thread's own latency, coarse enough to cost
/// nothing noticeable over a session that may run for hours.
fn spawn_activity_flusher(activity: std::sync::Arc<std::sync::Mutex<McpActivity>>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let now = crate::humanize::now_ms();
        let mut guard = activity.lock().unwrap();
        let pending: Vec<PathBuf> = guard
            .tools_since_heartbeat
            .iter()
            .filter(|(_, tools)| !tools.is_empty())
            .map(|(root, _)| root.clone())
            .collect();
        for root in pending {
            flush_if_due(&mut guard, &root, now);
        }
    });
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
            "description": "CALL THIS before refactoring, renaming, or deleting any core symbol, function, or model. Traces the AST import graph and call edges to depth 3. Returns every downstream file, background service, and test that will break. You MUST include all listed dependents in your change plan — a change that touches the concept without updating its dependents is incomplete.",
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
            "description": "CALL THIS when planning a new domain feature or adding a new concept. Scans the codebase for concepts that share name tokens or schemas. If a pair is returned, investigate before building — the territory may already be claimed under a different name.",
            "inputSchema": { "type": "object", "properties": { "root": root_prop } }
        },
        {
            "name": "duplicate_logic",
            "description": "Suspected duplicate BUSINESS LOGIC: two top-level functions in DIFFERENT files (often different languages), sharing no name worth acting on and no import/call edge, that independently encode the same rule — evidenced by sharing several literal string values (status names, error messages) inside their bodies. Different question from `duplicates` (concept-NAME overlap): this finds behavior reimplemented under a completely unrelated name, e.g. a server's validation logic quietly drifting from a client's copy of the same rule. Evidence of risk, not proof.",
            "inputSchema": { "type": "object", "properties": { "root": root_prop } }
        },
        {
            "name": "verdicts",
            "description": "Every declared concept bucketed by verdict — ACTIVE (declared and observably used) vs DECLARED_ONLY (declared, never observed in use) — with counts, instead of querying one concept name at a time. UNKNOWN and ABSENT are deliberately not listable here: those describe a search TERM's outcome, not a property a declared concept holds on its own.",
            "inputSchema": { "type": "object", "properties": { "root": root_prop } }
        },
        {
            "name": "verify_edit",
            "description": "Pre-write AST validation gate — validates proposed file content IN MEMORY before it touches disk. Pass the FULL proposed content of the file (after your edit has been applied in memory). Returns valid:true/false with exact error messages. CALL THIS before any fs_write or file-patching operation on a source file. Catches in <5ms what cargo build finds in 75s: Rust syntax errors (unterminated literals, mismatched braces via syn) and duplicate top-level symbol declarations in any supported language (the exact failure a blind str_replace produces when the old version is not removed before the new one is inserted). Exit: valid:true = safe to write; valid:false with errors = fix before writing.",
            "inputSchema": { "type": "object", "required": ["file", "content"], "properties": {
                "file": { "type": "string", "description": "Repository-relative path of the file being edited, e.g. src/structural.rs. Used to select the right extractor." },
                "content": { "type": "string", "description": "The FULL proposed file content after your edit has been applied in memory. Not a diff — the complete new file text." }
            } }
        },
        {
            "name": "claim",
            "description": "CALL THIS to verify architectural assumptions before executing changes. Tests whether a concept is genuinely absent (--type absence), actively adopted (--type usage-threshold), or strictly bounded to a directory (--type isolation). If it returns REFUTED, adjust your implementation plan to respect the violation list — do NOT proceed as if the claim were true.",
            "inputSchema": { "type": "object", "properties": {
                "statement": { "type": "string", "description": "Free-form claim, e.g. 'RefundService does not exist' or 'User is used in more than 5 files'." },
                "type": { "type": "string", "enum": ["absence", "usage-threshold", "isolation"], "description": "Structured claim type. Takes precedence over statement." },
                "target": { "type": "string", "description": "Concept name for structured claims." },
                "min": { "type": "number", "description": "Minimum usage count for usage-threshold claims." },
                "within": { "type": "string", "description": "Directory prefix for isolation claims, e.g. 'ghost-engine'." },
                "root": root_prop
            } }
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
            "description": "The architectural timeline: what changed, when, and what the engine said about it — Git knows files changed; this knows ARCHITECTURE changed. Append-only, written by the daemon, `archietect ci`, or an MCP client (mcp_client_connected on first tool call, mcp_client_active as a roughly-60s heartbeat thereafter — together, which AI used this project, when, and whether it is still active). Pass digest=true for a narrative-quality summary of the window (grouped, phrased sentences — still fully deterministic, generated from the same events, never an LLM) instead of the raw event list.",
            "inputSchema": { "type": "object", "properties": {
                "concept": { "type": "string", "description": "Optional — filter to events touching this concept. Ignored if digest=true." },
                "limit": { "type": "number", "description": "Max events to return, or events considered for the digest (default 50)." },
                "digest": { "type": "boolean", "description": "Return a narrative summary instead of the raw event list." },
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
            "description": "THE MAP OF THE BAG — call this before trusting any ABSENT. What this memory knows about the repository (counts, enabled domains), what it does NOT know and WHY (`not_known`: unsupported languages with the exact files, disabled or unconfirmed domains, evidence tiers no extractor can produce — e.g. whether a git remote is reachable or ahead/behind — and declared concepts never observed in use), each with how to establish the fact without archietect, plus the permission boundary including whether a human actually confirmed each unstructured domain. Distinguishes 'X does not exist' from 'X cannot be established here'. Pass since_last=true to also get `since_last_session`: what changed since the last time THIS was called with since_last=true for this project (concept/domain/not_known deltas) — then this call's snapshot becomes the new baseline for the next one.",
            "inputSchema": { "type": "object", "properties": { "root": root_prop, "since_last": { "type": "boolean", "description": "Diff against and then overwrite this project's tracked register snapshot." } } }
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
        },
        {
            "name": "photos_scan",
            "description": "SECOND UNSTRUCTURED DOMAIN. Scan one explicit directory for photo files (.jpg/.jpeg/.png/.gif/.heic/.webp), non-recursive — filename/extension/size/modified-time only, content never read. Same confirmation-gated contract as documents_scan: requires 'photos' to already be explicitly enabled via [domains.photos], since this tool can never prompt for the one-time confirmation over MCP.",
            "inputSchema": { "type": "object", "properties": {
                "dir": { "type": "string", "description": "Absolute path to the directory to scan." },
                "root": root_prop
            }, "required": ["dir"] }
        },
        {
            "name": "messages_scan",
            "description": "THIRD UNSTRUCTURED DOMAIN. No `dir` argument — unlike documents_scan/photos_scan, this checks a small set of well-known local message-store locations (macOS iMessage, Signal/WhatsApp/Slack/Discord) under this machine's home directory. Existence and top-level metadata only: a single-file store (iMessage's chat.db) reports size/mtime, a directory-based store reports only the directory's OWN mtime, never its contents — nothing is ever opened or queried. Same confirmation-gated contract as documents_scan/photos_scan: requires 'messages' to already be explicitly enabled via [domains.messages], since this tool can never prompt for the one-time confirmation over MCP.",
            "inputSchema": { "type": "object", "properties": { "root": root_prop } }
        },
        {
            "name": "docker_observe",
            "description": "LIVE container state — the one tool in this server that shells out (to `docker compose ps --format json --all`), unlike every other tool here which only reads what's already indexed. For each root-level compose file, reports each DECLARED service's real, current state right now: running, or observed NOT running. Requires 'docker' to already be enabled via [domains.docker], same gate the declarative docker scan uses. Silent (no resources) for a compose file the command can't be run against — missing `docker`, unreachable daemon, timeout — never a guessed or stale state.",
            "inputSchema": { "type": "object", "properties": { "root": root_prop } }
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
    // Track archietect.db mtime per root so the cache is invalidated
    // when the index changes on disk (archietect init ran in another
    // terminal, a migration updated the DB, etc.). Checked on every
    // tool call — hot-reload in 55ms via load_cached, never stale.
    let mut db_mtimes: std::collections::HashMap<PathBuf, std::time::SystemTime> = std::collections::HashMap::new();
    // Track archietect.db mtime per root — invalidate cache when the index
    // changes on disk (e.g. `archietect init` ran in another terminal).
    // Same 55ms load_cached path as the CLI fast-path fix; the session never
    // goes stale silently. Checked on every tool call, not just at startup.
    let mut db_mtimes: std::collections::HashMap<PathBuf, std::time::SystemTime> = std::collections::HashMap::new();
    // Captured from `initialize`'s `clientInfo` (name/version) — every real
    // MCP client sends this per the protocol spec, and until now archietect
    // just ignored it. Recorded once per (session, root actually touched)
    // into THAT project's own architectural history the first time a tool
    // call resolves a concrete root — the same event log `archietect ci`
    // already writes to beside the watch daemon, not a new mechanism. This
    // is the only way to answer "is an AI actually using this, and which
    // one" with evidence instead of a guess.
    // `mcp_client_connected` alone answers "has an AI ever used this
    // project" — it does NOT answer "is one using it RIGHT NOW", since it
    // fires exactly once per (session, root) and a session can run for
    // hours. Found live: a user demanding to SEE an AI actively working in
    // the GUI, not go find a single old log line and guess whether the
    // session behind it is even still open. See `McpActivity`'s own doc for
    // the rest of this design, including the bug (a call with nothing after
    // it never flushed) that made a background thread necessary.
    let activity = std::sync::Arc::new(std::sync::Mutex::new(McpActivity::default()));
    spawn_activity_flusher(activity.clone());

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
            "initialize" => {
                if let Some(ci) = msg["params"].get("clientInfo") {
                    activity.lock().unwrap().client_info = Some((
                        ci.get("name").and_then(|n| n.as_str()).unwrap_or("unknown").to_string(),
                        ci.get("version").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    ));
                }
                Ok(json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "archietect", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": "Architectural memory for this machine's repositories. Call `concept` before designing anything, `intent` for feature requests, `impact` before modifying models, `guard` on any patch that creates tables. Answers are deterministic facts with tiered evidence (DECLARED/USED/NAMED) — reason on top of them; do not override them with intuition."
                }))
            }
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
                        {
                            let mut guard = activity.lock().unwrap();
                            if guard.client_info.is_some() && !name.is_empty() {
                                let desc = describe_call(name, args);
                                guard.tools_since_heartbeat.entry(root.clone()).or_default().insert(desc);
                            }
                            let now = crate::humanize::now_ms();
                            flush_if_due(&mut guard, &root, now);
                        }
                        // Hot-reload: if archietect.db changed on disk since
                        // we last loaded it (archietect init, a migration, etc.)
                        // evict the cache entry so the next block reloads it.
                        // This is the fix for stale MCP answers: the in-process
                        // cache now tracks DB mtime per root and re-runs
                        // load_cached (55ms) the instant the file is newer.
                        let db_path = root.join("archietect.db");
                        let current_db_mtime = std::fs::metadata(&db_path)
                            .and_then(|m| m.modified())
                            .ok();
                        if let Some(current) = current_db_mtime {
                            let known = db_mtimes.get(&root).copied();
                            if known.map(|k| k != current).unwrap_or(false) {
                                // DB changed — evict stale cache entry
                                cache.remove(&root);
                            }
                            db_mtimes.insert(root.clone(), current);
                        }
                        let prior = cache.remove(&root);
                        // Fast path: if the in-process cache already has a
                        // warm index for this root, use it directly without
                        // scanning the filesystem at all. The cache is
                        // populated on the first call (cold path below) and
                        // stays valid until the process is restarted.
                        // Hot-reload (above) ensures stale entries are evicted
                        // when archietect.db changes on disk.
                        let (idx, graph) = match prior {
                            Some((s, g)) => (s, g),
                            None => {
                                // Cold path: try the persisted SQLite cache
                                // first (avoids a full scan even on the very
                                // first tool call of a session when the CLI
                                // has already init'd the project), then fall
                                // back to a full scan if neither exists.
                                match crate::store::load_cached(&root) {
                                    Some((s, g)) => (s, g),
                                    None => {
                                        let (s, g) = scan::scan_with_prior(&root, None, None);
                                        (s, g)
                                    }
                                }
                            }
                        };
                        let mut out = match name {
                            "concept" => query::concept(&idx, &graph, args["term"].as_str().unwrap_or("")),
                            "intent" => query::intent(&idx, &graph, args["text"].as_str().unwrap_or("")),
                            "impact" => query::impact(&idx, &graph, args["term"].as_str().unwrap_or("")),
                            "imports" => query::imports(&graph, args["file"].as_str().unwrap_or("")),
                            "guard" => query::guard(&idx, &graph, args["sql"].as_str().unwrap_or("")),
                            "plan" => query::plan(&idx, &graph, args["text"].as_str().unwrap_or("")),
                            "owner" => query::owner(&idx, &graph, args["term"].as_str().unwrap_or("")),
                            "claim" => {
                                let claim_type = args.get("type").and_then(|v| v.as_str());
                                if let Some(ct) = claim_type {
                                    query::claim_structured(
                                        &idx, &graph, ct,
                                        args.get("target").and_then(|v| v.as_str()),
                                        args.get("min").and_then(|v| v.as_u64()).map(|n| n as usize),
                                        args.get("within").and_then(|v| v.as_str()),
                                    )
                                } else {
                                    query::claim(&idx, &graph, args["statement"].as_str().unwrap_or(""))
                                }
                            },
                            "duplicates" => query::duplicates(&idx),
                            "duplicate_logic" => query::duplicate_logic(&graph),
                            "verdicts" => query::verdicts(&idx),
                            "verify_edit" => {
                                let file = args["file"].as_str().unwrap_or("");
                                let proposed = args["content"].as_str().unwrap_or("");
                                let verdict = crate::structural::verify_edit(file, proposed);
                                json!({
                                    "file": file,
                                    "valid": verdict.valid,
                                    "errors": verdict.errors,
                                    "warnings": verdict.warnings,
                                })
                            }
                            "status" => query::status(&idx, &graph),
                            "doctor" => query::doctor(&idx, &graph, &root),
                            "tour" => query::tour(&idx, &graph),
                            "history" if args.get("digest").and_then(|d| d.as_bool()).unwrap_or(false) => {
                                crate::store::history_digest(&root, args.get("limit").and_then(|l| l.as_u64()).unwrap_or(50) as usize)
                            }
                            "history" => json!({
                                "events": crate::store::read_history(
                                    &root,
                                    args.get("concept").and_then(|c| c.as_str()),
                                    args.get("limit").and_then(|l| l.as_u64()).unwrap_or(50) as usize,
                                ),
                                "note": "Append-only architectural timeline, newest first, written by the daemon, `archietect ci`, or an MCP client (mcp_client_connected on first tool call, mcp_client_active as a roughly-60s heartbeat thereafter).",
                            }),
                            "ci" => query::ci(&idx, &graph, args["diff"].as_str().unwrap_or(""), args.get("strict").and_then(|s| s.as_bool()).unwrap_or(false)),
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
                            "register" => {
                                let mut out = crate::register::register(&idx, &graph, &root);
                                if args.get("since_last").and_then(|s| s.as_bool()).unwrap_or(false) {
                                    let delta = crate::register::diff_since_last(&root, &out);
                                    out["since_last_session"] = delta;
                                }
                                out
                            }
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
                            // Same NonInteractiveAsker contract as
                            // documents_scan above — see tool_defs()'s
                            // description.
                            "photos_scan" => {
                                let dir_str = args["dir"].as_str().unwrap_or("");
                                if dir_str.is_empty() {
                                    json!({ "error": "missing required 'dir' argument" })
                                } else {
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
                            }
                            // No `dir` argument — see tool_defs()'s
                            // description. Same NonInteractiveAsker contract
                            // as documents_scan/photos_scan above.
                            "messages_scan" => {
                                let result: anyhow::Result<Value> = (|| {
                                    let global_path = permissions::default_global_config_path()?;
                                    let cfg = permissions::load(&global_path, &root)?;
                                    let confirmations_path = permissions::default_confirmations_path()?;
                                    let home = crate::messages_domain::default_home()?;
                                    let (enabled, resources) = crate::messages_domain::scan_if_allowed(
                                        &cfg,
                                        &confirmations_path,
                                        &home,
                                        &permissions::NonInteractiveAsker,
                                    )?;
                                    Ok(json!({ "enabled": enabled, "resources": resources }))
                                })();
                                match result {
                                    Ok(v) => v,
                                    Err(e) => json!({ "error": e.to_string() }),
                                }
                            }
                            // LIVE — shells out to `docker compose ps`, see
                            // tool_defs()'s description. Same
                            // `permissions::domain_allowed` gate the
                            // declarative docker scan uses.
                            "docker_observe" => {
                                match permissions::default_global_config_path().and_then(|g| permissions::load(&g, &root)) {
                                    Ok(cfg) => {
                                        let resources = docker_domain::scan_observed(&cfg, &root);
                                        json!({ "resources": resources })
                                    }
                                    Err(e) => json!({ "error": e.to_string() }),
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

    /// Real regression test for the exact bug reported live this session:
    /// `duplicates` then `impact(x)` back to back — `duplicates` happened to
    /// land on a heartbeat boundary and flushed (itself included), but
    /// `impact` started a fresh accumulator that nothing ever arrived to
    /// flush, because the old design only ever rechecked "is a flush due"
    /// when the NEXT call showed up. This makes exactly ONE call and then
    /// sends NOTHING further — the real subprocess, real stdio, no second
    /// call standing in for a background timer. If `spawn_activity_flusher`
    /// doesn't work, this call's info sits in memory forever and the
    /// assertion below finds nothing.
    #[test]
    fn last_call_in_a_session_flushes_even_with_no_call_after_it() {
        let home = tmp_dir("home-lone-call");
        let project = tmp_dir("project-lone-call");
        let (_guard, mut stdin, rx) = spawn_mcp(&project, &home);
        let _ = call(&mut stdin, &rx, 1, "initialize", json!({
            "clientInfo": { "name": "lone-call-test", "version": "9.9.9" }
        }));
        let _ = call(&mut stdin, &rx, 2, "tools/call", json!({ "name": "doctor", "arguments": {} }));

        // Nothing sent after this — real wall-clock wait, comfortably past
        // HEARTBEAT_INTERVAL_MS (3s) plus the flusher's own 500ms tick.
        std::thread::sleep(Duration::from_millis(4_500));

        let events = crate::store::read_history(&project, None, 10);
        let found = events.iter().any(|e| {
            e["kind"] == "mcp_client_connected"
                && e["concept"] == "lone-call-test"
                && e["detail"]["tools"].as_array().map(|t| t.iter().any(|x| x == "doctor")).unwrap_or(false)
        });
        assert!(
            found,
            "the lone 'doctor' call never flushed to history with no follow-up call — got events: {events:#?}"
        );
    }
}
