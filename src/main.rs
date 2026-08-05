//! architect — an architectural memory engine.
//!
//! Answers, for any codebase: does this concept already exist, which
//! implementation is canonical, what is the evidence, and what is the
//! smallest correct change. Deterministic by design — no API key, no
//! network, no model. AI tools are CLIENTS of this engine, never components
//! of it: an LLM reasons on top of architectural facts; it does not get to
//! replace them with intuition, because intuition is what produces duplicate
//! architecture in the first place.
//!
//!   architect init    --root DIR          build architect.db
//!   architect status  --root DIR          what the index knows
//!   architect concept --root DIR TERM     does TERM exist? what is canonical?
//!   architect intent  --root DIR "TEXT"   smallest correct change for an intent
//!   architect impact  --root DIR TERM     what is affected if TERM changes

use architect::{mcp, model, query, rest, scan, store, watch};

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "architect", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// Machine output for the bare glance (subcommands are always JSON —
    /// they are the scripting/agent surface; pipe them to jq)
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scan the repository and persist the index (architect.db)
    Init {
        #[arg(long)]
        root: PathBuf,
    },
    /// Summary of what the index knows — and what it admits it cannot see
    Status {
        #[arg(long)]
        root: PathBuf,
    },
    /// Does this concept exist? Which implementation is canonical?
    Concept {
        #[arg(long)]
        root: PathBuf,
        term: String,
    },
    /// From a stated intent to the smallest correct change
    Intent {
        #[arg(long)]
        root: PathBuf,
        text: Vec<String>,
    },
    /// What is affected if this concept changes?
    Impact {
        #[arg(long)]
        root: PathBuf,
        term: String,
    },
    /// THE LAW: check text for CREATE TABLE that duplicates an existing concept
    Guard {
        #[arg(long)]
        root: PathBuf,
        sql: String,
    },
    /// Serve the index over MCP (stdio) — makes every AI coding tool a client
    Mcp {
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// The law registry: every rule the engine obeys, with the wrong answer
    /// that taught it and the regression test that enforces it forever
    Laws,
    /// CI gate: pipe a diff in, get an exit code out.
    /// `git diff main... | architect ci --root .`
    Ci {
        #[arg(long)]
        root: PathBuf,
        /// Also fail on name-collision warnings, not only storage violations
        #[arg(long)]
        strict: bool,
    },
    /// REST API (127.0.0.1, read-only) — the GUI and CI become clients of the
    /// same engine; no business logic lives in any transport
    Serve {
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long, default_value_t = 7373)]
        port: u16,
    },
    /// Daemon mode: watch the tree, keep architect.db warm, and emit
    /// unprompted findings (duplicate-concept risk, lost storage, stale
    /// aliases) as JSON lines. Observation and notification — never action.
    Watch {
        #[arg(long)]
        root: PathBuf,
        /// Only STREAM events touching this concept (all events are still
        /// persisted to the timeline regardless)
        #[arg(long)]
        subscribe: Option<String>,
    },
    /// Repository summary for someone who just cloned it (the intern's view)
    Doctor {
        #[arg(long)]
        root: PathBuf,
    },
    /// Onboarding tour: important concepts, ignorable ones, and the mistakes
    /// the ontology already knows people will make — zero generated prose
    Tour {
        #[arg(long)]
        root: PathBuf,
    },
    /// Suspected duplicate concepts (name-token overlap — risk, not proof)
    Duplicates {
        #[arg(long)]
        root: PathBuf,
    },
    /// Who owns a concept: the directory that declares it
    Owner {
        #[arg(long)]
        root: PathBuf,
        term: String,
    },
    /// One-call architectural plan for an intent: canonical locations, owners,
    /// decisions, impact — composition of the five queries an agent needs
    Plan {
        #[arg(long)]
        root: PathBuf,
        text: Vec<String>,
    },
    /// The architectural timeline: what changed, when, and what the engine
    /// said about it — Git knows files changed; this knows ARCHITECTURE did
    History {
        #[arg(long)]
        root: PathBuf,
        /// Filter to events touching one concept
        concept: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
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
    let recent = g["recent_changes"].as_array().cloned().unwrap_or_default();
    println!("
Recent architectural changes");
    if recent.is_empty() {
        println!("  (none recorded — run the daemon and history accumulates)");
    }
    for e in recent {
        println!("  {} {}", e["kind"].as_str().unwrap_or("?"), e["concept"].as_str().unwrap_or(""));
    }
    let sug = g["suggestions"].as_array().cloned().unwrap_or_default();
    if !sug.is_empty() {
        println!("
Suggestions");
        for x in sug {
            println!("  • {}", x.as_str().unwrap_or(""));
        }
    }
    println!("
(architect --json for machine output; subcommands are always JSON)");
}

fn index_for(root: &PathBuf) -> model::Index {
    // Incremental: the scanner reuses per-file facts from architect.db where
    // (size, mtime, extractor version) are unchanged, and honours the
    // compiler rule — a changed concept set invalidates all cached usage.
    // Queries stay read-only; only `init` persists.
    scan::scan(root)
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // bare `architect` = the glance, on the current directory — the git-status
    // of architecture, the thing you run without thinking before starting work
    let Some(cmd) = cli.cmd else {
        let root = std::env::current_dir()?;
        let out = query::glance(&index_for(&root), &root);
        if cli.json {
            println!("{}", serde_json::to_string_pretty(&out)?);
        } else {
            // terminal-native by default — the glance is read by a human
            // between `cd` and the first edit, like git status
            print_glance(&out);
        }
        return Ok(());
    };
    let out = match cmd {
        Cmd::Init { root } => {
            let idx = scan::scan(&root);
            let path = store::save(&idx, &root)?;
            serde_json::json!({
                "indexed": path.display().to_string(),
                "files_scanned": idx.files_scanned,
                "concepts": idx.concepts.len(),
                "declaration_files": idx.declaration_files,
            })
        }
        Cmd::Status { root } => query::status(&index_for(&root)),
        Cmd::Concept { root, term } => query::concept(&index_for(&root), &term),
        Cmd::Intent { root, text } => query::intent(&index_for(&root), &text.join(" ")),
        Cmd::Impact { root, term } => query::impact(&index_for(&root), &term),
        Cmd::Guard { root, sql } => query::guard(&index_for(&root), &sql),
        Cmd::Mcp { root } => {
            mcp::serve(root)?;
            return Ok(());
        }
        Cmd::Laws => architect::laws::registry_json(),
        Cmd::Ci { root, strict } => {
            let mut diff = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut diff)?;
            let out = query::ci(&index_for(&root), &diff, strict);
            println!("{}", serde_json::to_string_pretty(&out)?);
            if out["pass"] == false {
                std::process::exit(1);
            }
            return Ok(());
        }
        Cmd::Serve { root, port } => {
            rest::serve(root, port)?;
            return Ok(());
        }
        Cmd::Watch { root, subscribe } => {
            watch::run(root, subscribe)?;
            return Ok(());
        }
        Cmd::Doctor { root } => query::doctor(&index_for(&root), &root),
        Cmd::Tour { root } => query::tour(&index_for(&root)),
        Cmd::Duplicates { root } => query::duplicates(&index_for(&root)),
        Cmd::Owner { root, term } => query::owner(&index_for(&root), &term),
        Cmd::Plan { root, text } => query::plan(&index_for(&root), &text.join(" ")),
        Cmd::History { root, concept, limit } => {
            serde_json::json!({
                "root": root.display().to_string(),
                "concept": concept,
                "events": store::read_history(&root, concept.as_deref(), limit),
                "note": "Append-only architectural timeline, newest first, written only by the daemon. Git knows which files changed; this knows which CONCEPTS changed, when, and what the engine observed about it at the time.",
            })
        }
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
