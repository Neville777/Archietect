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

use architect::{mcp, model, query, scan, store, watch};

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "architect", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
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

fn index_for(root: &PathBuf) -> model::Index {
    // Incremental: the scanner reuses per-file facts from architect.db where
    // (size, mtime, extractor version) are unchanged, and honours the
    // compiler rule — a changed concept set invalidates all cached usage.
    // Queries stay read-only; only `init` persists.
    scan::scan(root)
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let out = match cli.cmd {
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
        Cmd::Watch { root, subscribe } => {
            watch::run(root, subscribe)?;
            return Ok(());
        }
        Cmd::Doctor { root } => query::doctor(&index_for(&root), &root),
        Cmd::Tour { root } => query::tour(&index_for(&root)),
        Cmd::Duplicates { root } => query::duplicates(&index_for(&root)),
        Cmd::Owner { root, term } => query::owner(&index_for(&root), &term),
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
