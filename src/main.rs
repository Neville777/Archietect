//! architect — an architectural memory engine.
//!
//! Answers, for any codebase: does this concept already exist, which
//! implementation is canonical, what is the evidence, and what is the
//! smallest correct change. Deterministic by design — no API key, no
//! network, no model. AI tools are CLIENTS of this engine, never components
//! of it.
//!
//! ## Root discovery (git-style)
//!
//! `--root` is an OPTIONAL override on every command. Without it, the root is
//! discovered by walking upward from the current directory looking for
//! project markers (architect.db first — like .git, once you've init'd, the
//! repo is self-identifying). Resolved ONCE, before dispatch, and passed to
//! every handler: one resolver, zero per-command drift. Nobody types
//! `git status --repo .`; nobody should type `architect doctor --root .`.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

use architect::{mcp, model, query, rest, root, scan, store, watch};

#[derive(Parser)]
#[command(name = "architect", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// Repository root (optional everywhere — discovered git-style by walking
    /// upward from the current directory when omitted)
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    /// Machine output for the bare glance (subcommands are always JSON —
    /// they are the scripting/agent surface; pipe them to jq)
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scan the repository and persist the index (architect.db)
    Init,
    /// Summary of what the index knows — and what it admits it cannot see
    Status,
    /// Does this concept exist? Which implementation is canonical?
    Concept { term: String },
    /// From a stated intent to the smallest correct change
    Intent { text: Vec<String> },
    /// One-call architectural plan: canonical locations, owners, decisions,
    /// impact — the composition an agent would need five calls for
    Plan { text: Vec<String> },
    /// What is affected if this concept changes?
    Impact { term: String },
    /// THE LAW: check text for CREATE TABLE that duplicates an existing concept
    Guard { sql: String },
    /// Repository summary for someone who just cloned it (the intern's view)
    Doctor,
    /// Onboarding tour: important concepts, ignorable ones, and the mistakes
    /// the ontology already knows people will make — zero generated prose
    Tour,
    /// Suspected duplicate concepts (name-token overlap — risk, not proof)
    Duplicates,
    /// Who owns a concept: the directory that declares it
    Owner { term: String },
    /// The architectural timeline: what changed, when, and what the engine
    /// said about it — Git knows files changed; this knows ARCHITECTURE did
    History {
        concept: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// CI gate: pipe a diff in, get an exit code out.
    /// `git diff main... | architect ci`
    Ci {
        /// Also fail on name-collision warnings, not only storage violations
        #[arg(long)]
        strict: bool,
    },
    /// The law registry: every rule the engine obeys, with the wrong answer
    /// that taught it and the regression test that enforces it forever
    Laws,
    /// Daemon mode: watch the tree, keep architect.db warm, emit unprompted
    /// findings as JSON lines. Observation and notification — never action.
    Watch {
        /// Only STREAM events touching this concept (all events are still
        /// persisted to the timeline regardless)
        #[arg(long)]
        subscribe: Option<String>,
    },
    /// REST API (127.0.0.1, read-only) — GUI and CI become clients of the
    /// same engine; no business logic lives in any transport
    Serve {
        #[arg(long, default_value_t = 7373)]
        port: u16,
    },
    /// Serve the index over MCP (stdio) — makes every AI coding tool a client
    Mcp,
}

fn index_for(root: &PathBuf) -> model::Index {
    // Incremental: reuses per-file facts from architect.db where unchanged,
    // honours the schema-invalidates-usage dependency rule. Queries stay
    // read-only; only `init` (and the daemon) persist.
    scan::scan(root)
}

/// Render the glance for a terminal. Pure presentation — every value comes
/// from the same JSON `--json` emits; nothing is computed here.
fn print_glance(g: &serde_json::Value) {
    let s = &g["status"];
    println!("Repository: {}", g["repository"].as_str().unwrap_or("?"));
    println!(
        "  {} concepts · {} decisions · {} duplicate-storage risks · {} ontology warnings",
        s["concepts"], s["declared_decisions"], s["duplicate_storage_risks"], s["ontology_warnings"]
    );
    println!("  index: {}", s["index"].as_str().unwrap_or("?"));
    if let Some(ob) = g["onboarding"].as_str() {
        println!("\n{ob}");
    }
    let recent = g["recent_changes"].as_array().cloned().unwrap_or_default();
    println!("\nRecent architectural changes");
    if recent.is_empty() {
        println!("  (none recorded — run the daemon and history accumulates)");
    }
    for e in recent {
        println!("  {} {}", e["kind"].as_str().unwrap_or("?"), e["concept"].as_str().unwrap_or(""));
    }
    let sug = g["suggestions"].as_array().cloned().unwrap_or_default();
    if !sug.is_empty() {
        println!("\nSuggestions");
        for x in sug {
            println!("  • {}", x.as_str().unwrap_or(""));
        }
    }
    println!("\n(architect --json for machine output; subcommands are always JSON)");
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // ONE resolver, before dispatch — every handler receives the same root.
    let root = root::resolve_from_cwd(cli.root)?;

    // bare `architect` = the glance — the git-status of architecture
    let Some(cmd) = cli.cmd else {
        let out = query::glance(&index_for(&root), &root);
        if cli.json {
            println!("{}", serde_json::to_string_pretty(&out)?);
        } else {
            print_glance(&out);
        }
        return Ok(());
    };

    let out = match cmd {
        Cmd::Init => {
            let idx = scan::scan(&root);
            let path = store::save(&idx, &root)?;
            serde_json::json!({
                "indexed": path.display().to_string(),
                "files_scanned": idx.files_scanned,
                "concepts": idx.concepts.len(),
                "declaration_files": idx.declaration_files,
            })
        }
        Cmd::Status => query::status(&index_for(&root)),
        Cmd::Concept { term } => query::concept(&index_for(&root), &term),
        Cmd::Intent { text } => query::intent(&index_for(&root), &text.join(" ")),
        Cmd::Plan { text } => query::plan(&index_for(&root), &text.join(" ")),
        Cmd::Impact { term } => query::impact(&index_for(&root), &term),
        Cmd::Guard { sql } => query::guard(&index_for(&root), &sql),
        Cmd::Doctor => query::doctor(&index_for(&root), &root),
        Cmd::Tour => query::tour(&index_for(&root)),
        Cmd::Duplicates => query::duplicates(&index_for(&root)),
        Cmd::Owner { term } => query::owner(&index_for(&root), &term),
        Cmd::History { concept, limit } => serde_json::json!({
            "root": root.display().to_string(),
            "concept": concept,
            "events": store::read_history(&root, concept.as_deref(), limit),
            "note": "Append-only architectural timeline, newest first, written only by the daemon.",
        }),
        Cmd::Ci { strict } => {
            let mut diff = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut diff)?;
            let out = query::ci(&index_for(&root), &diff, strict);
            println!("{}", serde_json::to_string_pretty(&out)?);

            // Record the outcome. query::ci() itself stays read-only — REST
            // or MCP could call the same query to CHECK a diff without
            // meaning to record a decision — so the write belongs at this
            // ONE call site, which is the actual commit-time decision point
            // (the pre-commit hook). Closes the concrete gap found
            // 2026-08-06: a real duplicate was prevented through this exact
            // path and `architect history` had no way to say so.
            let pass = out["pass"] == true;
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let concept = out["violations"]
                .as_array()
                .and_then(|v| v.first())
                .and_then(|v| v["findings"].as_array())
                .and_then(|f| f.first())
                .and_then(|f| f["canonical"].as_str())
                .unwrap_or("commit")
                .to_string();
            let kind = if pass { "ci_passed" } else { "ci_blocked" };
            let _ = store::append_events(
                &root,
                &[(ts, kind.to_string(), concept, out.to_string())],
            );

            if !pass {
                std::process::exit(1);
            }
            return Ok(());
        }
        Cmd::Laws => architect::laws::registry_json(),
        Cmd::Watch { subscribe } => {
            watch::run(root, subscribe)?;
            return Ok(());
        }
        Cmd::Serve { port } => {
            // the discovered root becomes the default; per-request ?root=
            // still overrides, so one server serves the whole machine
            rest::serve(Some(root), port)?;
            return Ok(());
        }
        Cmd::Mcp => {
            mcp::serve(Some(root))?;
            return Ok(());
        }
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
