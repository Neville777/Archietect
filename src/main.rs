//! archietect — an architectural memory engine.
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
//! project markers (archietect.db first — like .git, once you've init'd, the
//! repo is self-identifying). Resolved ONCE, before dispatch, and passed to
//! every handler: one resolver, zero per-command drift. Nobody types
//! `git status --repo .`; nobody should type `archietect doctor --root .`.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

use archietect::{mcp, model, proposal, query, rest, root, scan, store, watch};

#[derive(Parser)]
#[command(name = "archietect", version, about)]
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
    /// Keep only these top-level keys of the JSON result (comma-separated),
    /// e.g. `status --only git`. Nothing else about the output changes. See
    /// src/shape.rs for why this exists — measured, not guessed.
    #[arg(long, global = true)]
    only: Option<String>,
    /// Drop the explanatory prose fields (`note`, `evidence_note`) from the
    /// result, recursively. Evidence, tiers, files and lines are all kept.
    #[arg(long, global = true)]
    compact: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scan the repository and persist the index (archietect.db)
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
    /// What does this file exactly, unambiguously import — and what
    /// imports it? Only exact relative-import resolutions are reported
    /// (see structural.rs's Import::relationship); an external package or
    /// an ambiguous match is correctly reported as nothing, not a guess.
    Imports {
        /// Repository-relative path, e.g. src/services/foo.ts
        file: String,
    },
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
        /// Also include events previously moved by `history-archive`. Off
        /// by default: most projects have no archive file, and merging two
        /// sources is extra work a caller that just wants the live tail
        /// shouldn't pay for.
        #[arg(long)]
        include_archived: bool,
        /// Return a narrative-quality summary of the window (grouped,
        /// phrased sentences — "3 new concepts appeared: ...") instead of
        /// the raw event list. Still fully deterministic, generated from
        /// the same events, never an LLM. See store::history_digest.
        #[arg(long)]
        digest: bool,
    },
    /// Cold-start fix: scan README.md for constraint/decision-shaped bullet
    /// points and propose them as `[[decision]]` entries, VERBATIM — no
    /// LLM, no rephrasing. Dry-run by default (prints candidates, writes
    /// nothing); --write appends new ones to archietect.toml. Human-invoked
    /// only, same shape as `history-archive` — never wired into REST/MCP,
    /// never run automatically. See src/seed.rs.
    Seed {
        #[arg(long)]
        write: bool,
    },
    /// Move events older than a cutoff out of the live archietect.db into a
    /// permanent, append-only archive file (.archietect/history-archive.db)
    /// — never deletes a record, only relocates it. A human-invoked
    /// maintenance action, the same shape as `git gc`; nothing in
    /// archietect calls this automatically. See store.rs's
    /// `archive_events_before` for why this exists and how it stays
    /// consistent with "history is append-only, never rewritten."
    HistoryArchive {
        /// Archive events older than this many days from now. Mutually
        /// exclusive with --before-ms.
        #[arg(long)]
        before_days: Option<u64>,
        /// Archive events with ts_ms strictly less than this raw
        /// millisecond timestamp. Mutually exclusive with --before-days.
        #[arg(long)]
        before_ms: Option<i64>,
    },
    /// CI gate: pipe a diff in, get an exit code out.
    /// `git diff main... | archietect ci`
    Ci {
        /// Also fail on name-collision warnings, not only storage violations
        #[arg(long)]
        strict: bool,
    },
    /// The law registry: every rule the engine obeys, with the wrong answer
    /// that taught it and the regression test that enforces it forever
    Laws,
    /// Daemon mode: watch the tree, keep archietect.db warm, emit unprompted
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
    /// AI-proposed extractor/decision/alias changes: submit a patch, test it
    /// against the existing laws + invariants suite in an isolated git
    /// worktree, and only then apply it to the working tree — uncommitted.
    /// AI proposes work, never evidence; a human always runs `accept`.
    #[command(subcommand)]
    Proposal(ProposalCmd),
    /// The cross-project pointer registry (~/.archietect/system.db) — an
    /// explicit, separate act from `init`. Stores only a project's root
    /// path, name, and timestamps; never any of its architectural facts,
    /// which stay exclusively in that project's own archietect.db.
    #[command(subcommand)]
    System(SystemCmd),
    /// Inspect the resolved domain permission state: what's enabled, where
    /// that decision came from (project config / global config / default),
    /// and the hardcoded denial list nothing can override. See
    /// SYSTEM_MEMORY.md's "Memory boundaries are the default" section.
    Permissions,
    /// Check ONE path against the permission boundary and give a reason —
    /// allowed or not. This is the call a pre-tool-use hook makes before
    /// letting an edit land: `check_resource` in permissions.rs, the same
    /// hardcoded-denial-first logic `resource_allowed`/`register` already
    /// use, just surfaced with the reason a blocked caller is owed. See
    /// SYSTEM_MEMORY.md's "Instruction files vs. memory vs. enforcement".
    PermissionsCheck {
        /// Path to check (absolute or relative to --root).
        #[arg(long)]
        path: PathBuf,
        /// Which domain this path is being read/written under. Defaults to
        /// "code" — the domain every existing command already treats as
        /// enabled by default, and the one a file-edit hook cares about.
        #[arg(long, default_value = "code")]
        domain: String,
    },
    /// The map of the bag: what this memory knows about this repository,
    /// what it does NOT know and why (unsupported languages, disabled or
    /// unconfirmed domains, evidence tiers no extractor can produce, declared
    /// concepts never observed in use), and where the permission boundary
    /// is. Read this before trusting any ABSENT. See src/register.rs.
    Register {
        /// Diff this call against the LAST tracked `register` call for this
        /// project (`.archietect/last_register_snapshot.json`) and include
        /// what changed under `since_last_session` — then overwrite the
        /// snapshot with this call, so the next one diffs against THIS one.
        /// A side effect deliberately kept out of `register::register`
        /// itself (pure composition) and opt-in here: `register` without
        /// this flag stays exactly as before, byte-for-byte.
        #[arg(long)]
        since_last: bool,
    },
    /// The first UNSTRUCTURED domain (SYSTEM_MEMORY.md's "Evidence has two
    /// vocabularies, not one") — filename/extension/size/modified-time only,
    /// content never read. Deliberately its own explicit subcommand rather
    /// than folded into `status`: this domain needs an explicit target
    /// directory and can trigger a one-time interactive y/N confirmation,
    /// neither of which belongs firing implicitly on every `status` call.
    #[command(subcommand)]
    Documents(DocumentsCmd),
    /// The SECOND unstructured domain — a sibling of `Documents` proving its
    /// discipline generalizes: filename/extension/size/modified-time only,
    /// content never read, same explicit-directory/confirmation-gated
    /// contract. See src/photos_domain.rs.
    #[command(subcommand)]
    Photos(PhotosCmd),
    /// LIVE container state — shells out to `docker compose ps`, unlike
    /// every other command in this binary. Deliberately its own explicit
    /// subcommand, never folded into `status`/`init`: a routine index build
    /// must never pay the cost or risk of a live process call (daemon down,
    /// `docker` missing, a hung engine). Same `permissions::domain_allowed`
    /// gate the declarative docker scan uses. See
    /// `docker_domain::scan_observed`.
    #[command(subcommand)]
    Docker(DockerCmd),
}

#[derive(Subcommand)]
enum DockerCmd {
    /// For each root-level compose file, observe each DECLARED service's
    /// real, current state right now (running / not running) via `docker
    /// compose ps --format json --all`. Silent (no resources) for a compose
    /// file the command can't be run against — missing `docker`, unreachable
    /// daemon, timeout — never a guessed or stale state.
    Observe,
}

#[derive(Subcommand)]
enum DocumentsCmd {
    /// Scan a directory for document files (.pdf/.docx/.txt/.md/.odt),
    /// non-recursive. First use of an unstructured domain requires a
    /// one-time interactive y/N confirmation (persisted so it's asked at
    /// most once) unless `[domains.documents]` is already set in
    /// archietect.toml/system.toml — fails closed with no real TTY.
    Scan {
        #[arg(long)]
        dir: PathBuf,
    },
}

#[derive(Subcommand)]
enum PhotosCmd {
    /// Scan a directory for photo files (.jpg/.jpeg/.png/.gif/.heic/.webp),
    /// non-recursive. Same confirmation-gated contract as `Documents`.
    Scan {
        #[arg(long)]
        dir: PathBuf,
    },
}

#[derive(Subcommand)]
enum SystemCmd {
    /// Register this project (the resolved --root) in the system-level
    /// pointer registry. Safe to re-run: updates last-seen, never
    /// duplicates the entry or resets when it was first registered.
    Register,
    /// List every project registered in the system-level pointer registry.
    List,
    /// Fan out a concept lookup over every registered project's OWN db,
    /// read-only, live — "which of my projects has X". Never caches results
    /// back into system.db; a re-run always re-reads each project's current
    /// state. A registered project with no archietect.db (moved, deleted,
    /// or never `init`'d) is reported honestly, not skipped silently.
    Query { term: String },
    /// "What do I have" across every registered project in one call — each
    /// project's own `archietect status` (counts, git, docker,
    /// same_project_as), fetched live and read-only from that project's own
    /// db, never cached into system.db. A registered project with no
    /// archietect.db is reported honestly, not skipped silently.
    Status,
}

#[derive(Subcommand)]
enum ProposalCmd {
    /// Register a new proposal (a patch) as pending — writes nothing outside
    /// .archietect/proposals/, does not touch the working tree
    Submit {
        #[arg(long, value_enum)]
        kind: proposal::Kind,
        #[arg(long)]
        title: String,
        #[arg(long, default_value = "")]
        description: String,
        /// Language name, for an extractor proposal
        #[arg(long)]
        lang: Option<String>,
        /// A real repo to preview an extractor against (informational only)
        #[arg(long)]
        preview_repo: Option<String>,
        /// Who/what authored this patch — "ai", "human", a tool name...
        #[arg(long, default_value = "human")]
        source: String,
        /// Path to a unified diff (git diff format)
        #[arg(long)]
        patch: PathBuf,
    },
    /// List all proposals and their status
    List,
    /// Show a proposal's metadata, patch, and last test result
    Inspect { id: u64 },
    /// Apply the patch in an isolated git worktree and run the existing
    /// regression suite against it — laws + invariants for an extractor,
    /// invariants::check for a decision/alias. Never touches the real
    /// working tree or archietect.db.
    Test { id: u64 },
    /// Apply a passed, unmodified-since-test proposal to the real working
    /// tree — uncommitted. Never runs `git commit`.
    Accept { id: u64 },
    /// Mark a proposal rejected (kept for audit trail unless --purge)
    Reject {
        id: u64,
        #[arg(long)]
        purge: bool,
    },
}

fn index_for(root: &PathBuf) -> (model::Index, archietect::structural::StructuralGraph) {
    // Incremental: reuses per-file facts from archietect.db where unchanged,
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
    println!("\n(archietect --json for machine output; subcommands are always JSON)");
}

/// `println!` panics on a write error, including the ordinary case of stdout
/// closing early (`archietect concept Foo | head`). Every other well-behaved
/// CLI (grep, cat, jq) exits quietly on SIGPIPE instead of printing a Rust
/// backtrace; restoring the default disposition here gets the same behavior.
#[cfg(unix)]
fn reset_sigpipe() {
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
}
#[cfg(not(unix))]
fn reset_sigpipe() {}

fn main() -> anyhow::Result<()> {
    reset_sigpipe();
    let cli = Cli::parse();
    // Output shaping (src/shape.rs): parsed once, applied at every print
    // site below. Both default to "change nothing".
    let only = archietect::shape::parse_only(cli.only.as_deref());
    let compact = cli.compact;
    // ONE resolver, before dispatch — every handler receives the same root.
    let root = root::resolve_from_cwd(cli.root)?;

    // bare `archietect` = the glance — the git-status of architecture
    let Some(cmd) = cli.cmd else {
        let (idx, graph) = index_for(&root);
        let out = query::glance(&idx, &graph, &root);
        if cli.json {
            println!("{}", serde_json::to_string_pretty(&archietect::shape::apply(out.clone(), only.as_deref(), compact))?);
        } else {
            print_glance(&out);
        }
        return Ok(());
    };

    let out = match cmd {
        Cmd::Init => {
            let (idx, graph) = scan::scan(&root);
            let path = store::save(&idx, &graph, &root)?;
            serde_json::json!({
                "indexed": path.display().to_string(),
                "files_scanned": idx.files_scanned,
                "concepts": idx.concepts.len(),
                "symbols": graph.symbols.len(),
                "routes": graph.routes.len(),
                "declaration_files": idx.declaration_files,
            })
        }
        Cmd::Status => { let (idx, g) = index_for(&root); query::status(&idx, &g) }
        Cmd::Concept { term } => { let (idx, g) = index_for(&root); query::concept(&idx, &g, &term) }
        Cmd::Intent { text } => { let (idx, _g) = index_for(&root); query::intent(&idx, &text.join(" ")) }
        Cmd::Plan { text } => { let (idx, g) = index_for(&root); query::plan(&idx, &g, &text.join(" ")) }
        Cmd::Impact { term } => { let (idx, g) = index_for(&root); query::impact(&idx, &g, &term) }
        Cmd::Imports { file } => { let (_idx, g) = index_for(&root); query::imports(&g, &file) }
        Cmd::Guard { sql } => { let (idx, _g) = index_for(&root); query::guard(&idx, &sql) }
        Cmd::Doctor => { let (idx, g) = index_for(&root); query::doctor(&idx, &g, &root) }
        Cmd::Tour => { let (idx, g) = index_for(&root); query::tour(&idx, &g) }
        Cmd::Duplicates => { let (idx, _g) = index_for(&root); query::duplicates(&idx) }
        Cmd::Owner { term } => { let (idx, g) = index_for(&root); query::owner(&idx, &g, &term) }
        Cmd::History { concept, limit, include_archived: _, digest } if digest => {
            let mut out = store::history_digest(&root, limit);
            if concept.is_some() {
                out["note"] = serde_json::json!("--digest summarizes the whole window; --concept is ignored with --digest. Drop --digest to filter by concept.");
            }
            out
        }
        Cmd::History { concept, limit, include_archived, digest: _ } => {
            let mut events = store::read_history(&root, concept.as_deref(), limit);
            if include_archived {
                events.extend(archietect::store::read_archived_history(&root, concept.as_deref(), limit));
                events.sort_by(|a, b| b["ts_ms"].as_i64().unwrap_or(0).cmp(&a["ts_ms"].as_i64().unwrap_or(0)));
                events.truncate(limit);
            }
            serde_json::json!({
                "root": root.display().to_string(),
                "concept": concept,
                "events": events,
                "note": "Append-only architectural timeline, newest first, written only by the daemon. Pass --include-archived to also see events moved by `history-archive`.",
            })
        }
        Cmd::Seed { write } => {
            let (idx, _g) = index_for(&root);
            archietect::seed::seed(&root, &idx, write)?
        }
        Cmd::HistoryArchive { before_days, before_ms } => {
            let cutoff = match (before_days, before_ms) {
                (Some(_), Some(_)) => anyhow::bail!("pass exactly one of --before-days or --before-ms, not both"),
                (None, None) => anyhow::bail!("pass one of --before-days or --before-ms"),
                (Some(days), None) => {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    now_ms - (days as i64) * 86_400_000
                }
                (None, Some(ms)) => ms,
            };
            let (archived_events, archived_to) = archietect::store::archive_events_before(&root, cutoff)?;
            serde_json::json!({
                "root": root.display().to_string(),
                "cutoff_ms": cutoff,
                "archived_events": archived_events,
                "archived_to": archived_to.display().to_string(),
                "note": "Moved, not deleted — the full record still exists in archived_to. Pass --include-archived to `archietect history` to read it back.",
            })
        }
        Cmd::Ci { strict } => {
            let mut diff = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut diff)?;
            let (idx, _g) = index_for(&root);
            let out = query::ci(&idx, &diff, strict);
            println!("{}", serde_json::to_string_pretty(&archietect::shape::apply(out.clone(), only.as_deref(), compact))?);

            // Record the outcome. query::ci() itself stays read-only — REST
            // or MCP could call the same query to CHECK a diff without
            // meaning to record a decision — so the write belongs at this
            // ONE call site, which is the actual commit-time decision point
            // (the pre-commit hook). Closes the concrete gap found
            // 2026-08-06: a real duplicate was prevented through this exact
            // path and `archietect history` had no way to say so.
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
        Cmd::Laws => archietect::laws::registry_json(),
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
        Cmd::Proposal(pcmd) => match pcmd {
            ProposalCmd::Submit { kind, title, description, lang, preview_repo, source, patch } => proposal::submit(
                &root,
                kind,
                &title,
                &description,
                lang.as_deref(),
                preview_repo.as_deref(),
                &source,
                &patch,
            )?,
            ProposalCmd::List => proposal::list(&root),
            ProposalCmd::Inspect { id } => proposal::inspect(&root, id)?,
            ProposalCmd::Test { id } => {
                let out = proposal::test(&root, id)?;
                let passed = out["result"]["passed"] == true;
                println!("{}", serde_json::to_string_pretty(&archietect::shape::apply(out.clone(), only.as_deref(), compact))?);
                if !passed {
                    std::process::exit(1);
                }
                return Ok(());
            }
            ProposalCmd::Accept { id } => proposal::accept(&root, id)?,
            ProposalCmd::Reject { id, purge } => proposal::reject(&root, id, purge)?,
        },
        Cmd::System(scmd) => {
            let db_path = archietect::system_db::default_db_path()?;
            match scmd {
                SystemCmd::Register => {
                    let p = archietect::system_db::register_project(&db_path, &root)?;
                    serde_json::json!({
                        "registered": p.root,
                        "name": p.name,
                        "first_registered_ms": p.first_registered_ms,
                        "last_seen_ms": p.last_seen_ms,
                        "system_db": db_path.display().to_string(),
                    })
                }
                SystemCmd::List => {
                    let projects = archietect::system_db::list_projects(&db_path)?;
                    serde_json::json!({
                        "projects": projects.iter().map(|p| serde_json::json!({
                            "root": p.root,
                            "name": p.name,
                            "first_registered_ms": p.first_registered_ms,
                            "last_seen_ms": p.last_seen_ms,
                        })).collect::<Vec<_>>(),
                        "system_db": db_path.display().to_string(),
                    })
                }
                SystemCmd::Query { term } => {
                    let results = archietect::system_db::query_registered_projects(&db_path, &term)?;
                    serde_json::json!({
                        "term": term,
                        "results": results.iter().map(|r| serde_json::json!({
                            "root": r.root,
                            "name": r.name,
                            "found": r.found,
                        })).collect::<Vec<_>>(),
                        "system_db": db_path.display().to_string(),
                        "note": "each project's own archietect.db is read live and read-only on every call; system.db itself stores only pointers and is never updated by this command.",
                    })
                }
                SystemCmd::Status => {
                    let results = archietect::system_db::status_registered_projects(&db_path)?;
                    serde_json::json!({
                        "projects": results.iter().map(|r| serde_json::json!({
                            "root": r.root,
                            "name": r.name,
                            "status": r.status,
                        })).collect::<Vec<_>>(),
                        "system_db": db_path.display().to_string(),
                        "note": "each project's own archietect.db is read live and read-only on every call; system.db itself stores only pointers and is never updated by this command.",
                    })
                }
            }
        }
        Cmd::Permissions => {
            let global_path = archietect::permissions::default_global_config_path()?;
            let cfg = archietect::permissions::load(&global_path, &root)?;
            archietect::permissions::report(&cfg)
        }
        Cmd::PermissionsCheck { path, domain } => {
            let global_path = archietect::permissions::default_global_config_path()?;
            let cfg = archietect::permissions::load(&global_path, &root)?;
            let full_path = if path.is_absolute() { path.clone() } else { root.join(&path) };
            let decision = archietect::permissions::check_resource(&cfg, &domain, &full_path);
            serde_json::json!({
                "path": full_path.display().to_string(),
                "domain": domain,
                "allowed": decision.allowed,
                "reason": decision.reason,
            })
        }
        Cmd::Register { since_last } => {
            let (idx, g) = index_for(&root);
            let mut out = archietect::register::register(&idx, &g, &root);
            if since_last {
                let delta = archietect::register::diff_since_last(&root, &out);
                out["since_last_session"] = delta;
            }
            out
        }
        Cmd::Documents(DocumentsCmd::Scan { dir }) => {
            let global_path = archietect::permissions::default_global_config_path()?;
            let cfg = archietect::permissions::load(&global_path, &root)?;
            let confirmations_path = archietect::permissions::default_confirmations_path()?;
            let asker = archietect::permissions::stdio_asker();
            let (enabled, resources) =
                archietect::documents_domain::scan_if_allowed(&cfg, &confirmations_path, &dir, asker.as_ref())?;
            serde_json::json!({
                "dir": dir.display().to_string(),
                "enabled": enabled,
                "resources": resources,
            })
        }
        Cmd::Photos(PhotosCmd::Scan { dir }) => {
            let global_path = archietect::permissions::default_global_config_path()?;
            let cfg = archietect::permissions::load(&global_path, &root)?;
            let confirmations_path = archietect::permissions::default_confirmations_path()?;
            let asker = archietect::permissions::stdio_asker();
            let (enabled, resources) =
                archietect::photos_domain::scan_if_allowed(&cfg, &confirmations_path, &dir, asker.as_ref())?;
            serde_json::json!({
                "dir": dir.display().to_string(),
                "enabled": enabled,
                "resources": resources,
            })
        }
        Cmd::Docker(DockerCmd::Observe) => {
            let global_path = archietect::permissions::default_global_config_path()?;
            let cfg = archietect::permissions::load(&global_path, &root)?;
            let resources = archietect::docker_domain::scan_observed(&cfg, &root);
            serde_json::json!({ "resources": resources })
        }
    };
    println!("{}", serde_json::to_string_pretty(&archietect::shape::apply(out.clone(), only.as_deref(), compact))?);
    Ok(())
}
