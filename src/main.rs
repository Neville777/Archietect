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

mod mcp;
mod model;
mod query;
mod scan;
mod store;

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
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
