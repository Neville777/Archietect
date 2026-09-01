//! The daemon — `architect watch --root DIR`. Levels 1–3 of autonomy, and
//! deliberately NOT levels 4–5.
//!
//! Level 1, automatic OBSERVATION: a filesystem watcher triggers the
//! incremental scan on change; architect.db stays warm, so every client —
//! CLI, MCP, any editor — reads the same continuously maintained facts with
//! no startup scan. rust-analyzer thinking: the model is rebuilt as you save,
//! not when you ask.
//!
//! Level 2, automatic REASONING: the incremental engine already recomputes
//! the graph and honours the schema→usage dependency edge on every rescan.
//!
//! Level 3, automatic NOTIFICATION: each rescan is DIFFED against the prior
//! index, and findings are emitted unprompted as JSON lines:
//!   duplicate_concept_risk — a new concept's name collides with an existing
//!                            canonical or a declared alias (the disease,
//!                            caught at the moment of infection);
//!   concept_lost_storage   — a concept lost all its declarations (removed
//!                            intentionally, or a refactor casualty?);
//!   stale_alias            — architect.toml points at a concept that no
//!                            longer exists.
//!
//! Levels 4–5 (proposal generation, auto-apply) are deliberately ABSENT from
//! this engine: they require an AI and a permission model, and this core is
//! deterministic. Architect preserves architectural truth; it does not decide
//! product direction. The guard rejects with evidence — it never rewrites.
//!
//! Concurrency: the daemon is the ONLY writer to architect.db (queries are
//! read-only by design), so SQLite's single-writer model is satisfied without
//! locks or coordination.

use crate::model::{names_concept, Index};
use crate::{scan, store};
use notify::{RecursiveMode, Watcher};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;
use walkdir::WalkDir;

const SKIP_DIRS: &[&str] = &[
    "node_modules", ".git", ".next", "target", "dist", "build", "__pycache__",
    ".venv", "venv", ".turbo", "coverage", ".cache", "vendor",
];

fn skip_dir(e: &walkdir::DirEntry) -> bool {
    e.file_type().is_dir()
        && e.file_name().to_str().map(|n| SKIP_DIRS.contains(&n)).unwrap_or(false)
}

/// Every directory the watcher should register on, respecting SKIP_DIRS
/// entirely — not just filtering events after the fact, but never
/// registering a watch inside an excluded subtree in the first place.
///
/// Found by dogfooding on TITAN: `watcher.watch(&root, RecursiveMode::
/// Recursive)` registers ONE recursive watch on the whole tree, which
/// means the OS watch backend (inotify on Linux) has to walk and register
/// every directory underneath — including target/, which on TITAN is 15GB
/// across 3,430 directories. `relevant()` filtered target/ EVENTS, but by
/// then the registration cost (and, on Linux, inotify's default
/// max_user_watches limit — often far below 3,430) was already paid. The
/// daemon pegged one CPU core at 100% for minutes registering watches it
/// was going to discard every event from anyway.
///
/// Trade-off, stated: each returned directory gets a NON-recursive watch,
/// so a brand-new subdirectory created after startup won't be picked up
/// until the daemon restarts. That is a real gap, but it is a vastly
/// smaller one than "the daemon cannot start on any repository with a
/// large build-artifact directory" — which is what the recursive call
/// produced in practice.
fn watchable_dirs(root: &Path) -> Vec<PathBuf> {
    WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !skip_dir(e))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_dir())
        .map(|e| e.into_path())
        .collect()
}

fn relevant(path: &Path) -> bool {
    // Never react to our own database — the daemon writing it must not wake
    // the daemon. PREFIX match, not exact: found by dogfooding on TITAN.
    // SQLite's rollback-journal file (architect.db-journal) is created and
    // deleted around EVERY write transaction — a file whose name does not
    // equal "architect.db" exactly, so the old exact check let both its
    // create and delete events through as "relevant". Each daemon write
    // (store::save, store::append_events) generated new events that woke
    // the daemon, which rescanned and wrote again — a fully self-sustaining
    // feedback loop, observed live: 1,300+ queued events every 300ms,
    // forever, with the daemon never actually going idle. Covers -journal,
    // -wal, -shm, and any future SQLite auxiliary/temp file with this
    // prefix, not just the ones known today.
    if path
        .file_name()
        .and_then(|f| f.to_str())
        .map(|f| f.starts_with("architect.db"))
        .unwrap_or(false)
    {
        return false;
    }
    if path.components().any(|c| {
        c.as_os_str().to_str().map(|s| SKIP_DIRS.contains(&s)).unwrap_or(false)
    }) {
        return false;
    }
    true
}

pub type Event = (i64, String, String, serde_json::Value); // ts, kind, concept, detail

/// LAW-013: a shared GENERIC architectural-role word is never, by itself,
/// evidence of a duplicate concept — same_word()/names_concept() still match
/// on it for direct lookup, but the collision-detection loops below (and
/// the CI guard, and the storage-family grouping in query::glance) must
/// skip it as a collision trigger. Found dogfooding the watch daemon: a
/// brand-new `executor_gaps` table was flagged as colliding with an
/// unrelated `BinanceExecutor` struct via the single shared token
/// "executor" — a role, not a domain concept.
pub const GENERIC_ROLE_TOKENS: &[&str] = &[
    "executor", "manager", "handler", "service", "controller", "factory", "builder",
    "adapter", "provider", "client", "worker", "engine", "repository", "store",
    "registry", "gateway", "middleware", "config", "result", "response", "request",
    "context", "state",
];

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn event(out: &mut Vec<Event>, kind: &str, concept: &str, detail: serde_json::Value) {
    out.push((now_ms(), kind.to_string(), concept.to_string(), detail));
}

/// Diff two indexes into level-3 findings. Deterministic; observations,
/// never actions — the daemon records and reports, it does not plan.
pub fn diff_findings(old: &Index, new: &Index) -> Vec<Event> {
    let mut out = Vec::new();

    // ── RENAMES first: a removed + an appeared concept sharing IDENTITY
    // (the same declared table, or strong field overlap) is one concept
    // changing its name, not a death and an unrelated birth. Git sees a
    // deleted string and an added string; this sees Story → Narrative.
    // Renamed pairs are then EXCLUDED from appeared/lost/duplicate findings —
    // reporting a rename as three separate alarms would bury the signal.
    let mut renamed_from: Vec<String> = Vec::new();
    let mut renamed_to: Vec<String> = Vec::new();
    for (rname, oc) in &old.concepts {
        if new.concepts.contains_key(rname) {
            continue;
        }
        for (aname, nc) in &new.concepts {
            if old.concepts.contains_key(aname) {
                continue;
            }
            let same_table = oc.table.is_some() && oc.table == nc.table;
            let shared = oc.fields.iter().filter(|f| nc.fields.contains(f)).count();
            let overlap = shared >= 3 && shared * 10 >= oc.fields.len().max(1) * 6;
            if same_table || overlap {
                event(&mut out, "concept_renamed", aname, json!({
                    "from": rname,
                    "to": aname,
                    "identity_evidence": if same_table {
                        format!("same declared table '{}'", oc.table.clone().unwrap_or_default())
                    } else {
                        format!("{shared} shared fields")
                    },
                    "note": "provenance carries over: this is one concept changing its name, not a new concept",
                }));
                renamed_from.push(rname.clone());
                renamed_to.push(aname.clone());
                break;
            }
        }
    }

    // NEW concepts — check each for collision with existing canonicals/aliases.
    for (name, c) in &new.concepts {
        if old.concepts.contains_key(name) || renamed_to.contains(name) {
            continue;
        }
        event(&mut out, "concept_appeared", name, json!({
            "declared_in": c.declared_in,
        }));
        // duplicate risk: any token of the new name resolving to an EXISTING
        // concept or declared alias is the moment of infection.
        for tok in crate::model::name_tokens(name) {
            if GENERIC_ROLE_TOKENS.contains(&tok.to_lowercase().as_str()) {
                continue;
            }
            let hit = old
                .concepts
                .keys()
                .find(|other| names_concept(other, &tok))
                .cloned()
                .or_else(|| {
                    new.aliases
                        .iter()
                        .find(|(k, _)| crate::model::same_word(k, &tok))
                        .map(|(_, target)| target.clone())
                });
            if let Some(existing) = hit {
                if existing != *name {
                    // Full drift report: the finding carries its EVIDENCE
                    // (the canonical's declarations, its relations, and the
                    // governing decision when one is declared) — no AI, just
                    // the facts a reader needs to act without a second query.
                    let canon = old.concepts.get(&existing).or_else(|| new.concepts.get(&existing));
                    let decision = new.decisions.iter().find(|d| {
                        d.links.iter().any(|l| crate::model::same_word(l, &tok)
                            || l.eq_ignore_ascii_case(&existing))
                    });
                    event(&mut out, "duplicate_concept_risk", name, json!({
                        "collides_with": existing,
                        "via_token": tok,
                        "evidence": {
                            "declared_in": canon.map(|c| c.declared_in.clone()),
                            "relations": canon.map(|c| c.relations.clone()),
                            "governing_decision": decision.map(|d| json!({
                                "id": d.id, "because": d.because,
                                "rejected": d.rejected,
                            })),
                        },
                        "suggested_action": format!(
                            "Extend '{existing}' rather than introducing '{name}'."
                        ),
                    }));
                    break;
                }
            }
        }
    }
    // REMOVED concepts — storage vanished; intentional or a refactor casualty?
    for name in old.concepts.keys() {
        if !new.concepts.contains_key(name) && !renamed_from.contains(name) {
            event(&mut out, "concept_lost_storage", name, json!({
                "advice": "all declarations for this concept are gone — if unintentional, a refactor just deleted storage something may still depend on",
            }));
        }
    }
    // ONTOLOGY changes — the declared layer has its own event vocabulary.
    for (k, target) in &new.aliases {
        if !old.aliases.contains_key(k) {
            event(&mut out, "alias_introduced", k, json!({ "target": target }));
        }
    }
    for (k, target) in &old.aliases {
        if !new.aliases.contains_key(k) {
            event(&mut out, "alias_removed", k, json!({ "was_target": target }));
        }
    }
    for d in &new.decisions {
        if !old.decisions.iter().any(|o| o.id == d.id) {
            event(&mut out, "decision_added", &d.id, json!({ "decision": d.decision }));
        }
    }
    for d in &old.decisions {
        if !new.decisions.iter().any(|n| n.id == d.id) {
            event(&mut out, "decision_removed", &d.id, json!({
                "was": d.decision,
                "advice": "a recorded architectural decision was deleted — rationale removed is rationale lost; supersede rather than delete",
            }));
        }
    }

    // STALE aliases — the declared ontology pointing at nothing.
    for (k, target) in &new.aliases {
        let resolves = new
            .concepts
            .keys()
            .any(|n| n == target || names_concept(n, target.trim_end_matches('s')));
        if !resolves {
            event(&mut out, "stale_alias", k, json!({
                "target": target,
                "advice": "architect.toml declares this alias but the target concept no longer exists — the ontology file is stale or the concept was removed",
            }));
        }
    }
    out
}

/// Print an event as a JSON line, honouring an optional subscription filter
/// (a concept token — `--subscribe forecast` sees only forecast's timeline).
/// ALL events are persisted regardless; the filter shapes the stream, never
/// the record.
fn print_event(ev: &Event, subscribe: Option<&str>) {
    if let Some(pat) = subscribe {
        if !(names_concept(&ev.2, pat) || ev.2.eq_ignore_ascii_case(pat)) {
            return;
        }
    }
    println!("{}", json!({ "ts_ms": ev.0, "kind": ev.1, "concept": ev.2, "detail": ev.3 }));
}

pub fn run(root: PathBuf, subscribe: Option<String>) -> anyhow::Result<()> {
    let sub = subscribe.as_deref();
    // See `crate::exe_mtime`'s doc comment — the daemon is the longest-lived
    // process in this whole system (started at login, runs for weeks), so
    // it's the most likely of all of them to end up silently serving a
    // rebuilt-out-from-under-it binary.
    let started_mtime = crate::exe_mtime();
    // initial build — the daemon starts knowing.
    let (mut current, mut current_graph) = scan::scan(&root);
    store::save(&current, &current_graph, &root)?;
    println!("{}", json!({
        "ts_ms": now_ms(), "kind": "watching", "concept": "",
        "detail": {
            "root": root.display().to_string(),
            "files": current.files_scanned,
            "concepts": current.concepts.len(),
            "subscribed": subscribe,
        }
    }));

    let (tx, rx) = mpsc::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if ev.paths.iter().any(|p| relevant(p)) {
                let _ = tx.send(());
            }
        }
    })?;
    // NON-recursive, per included directory — never registers inside
    // SKIP_DIRS at all (target/, node_modules/, .git/, ...), instead of
    // registering everywhere and filtering events afterward. See
    // watchable_dirs() for why: a single recursive call from root pegged a
    // CPU core for minutes registering watches on TITAN's 15GB target/
    // before discarding every event it produced.
    let dirs = watchable_dirs(&root);
    for dir in &dirs {
        // Best-effort: a directory that vanished between the walk and here
        // (a build tool cleaning up) should not abort the whole daemon.
        let _ = watcher.watch(dir, RecursiveMode::NonRecursive);
    }
    eprintln!("architect watch: registered {} directories (skipped target/node_modules/.git/...)", dirs.len());

    loop {
        // block until something changes…
        if rx.recv().is_err() {
            break;
        }
        // …then debounce: absorb the burst (editors write several events per
        // save) and rescan once when the tree goes quiet.
        while rx.recv_timeout(Duration::from_millis(300)).is_ok() {}

        if let (Some(started), Some(now)) = (started_mtime, crate::exe_mtime()) {
            if now != started {
                println!("{}", json!({
                    "ts_ms": now_ms(), "kind": "stale_binary_warning", "concept": "",
                    "detail": "This daemon has been running since before the architect binary on disk was last rebuilt — it is rescanning with OLD extractor code. Restart it (systemctl --user restart architectd@<escaped-path>, or re-run `architect watch`) to pick up the current build.",
                }));
            }
        }

        let (next, next_graph) = scan::scan_with_prior(&root, Some(current.clone()), Some(current_graph.clone()));
        let mut events = diff_findings(&current, &next);
        // ARCHITECTURE VERSION: a monotonic number that advances only when
        // the concept set changes — migration numbering for architectural
        // knowledge, with the +/- delta recorded like a changelog entry.
        let added: Vec<&String> =
            next.concepts.keys().filter(|n| !current.concepts.contains_key(*n)).collect();
        let removed: Vec<&String> =
            current.concepts.keys().filter(|n| !next.concepts.contains_key(*n)).collect();
        if !added.is_empty() || !removed.is_empty() {
            if let Ok(v) = store::bump_arch_version(&root) {
                events.push((now_ms(), "architecture_version".into(), format!("v{v}"), json!({
                    "version": v,
                    "added": added,
                    "removed": removed,
                })));
            }
        }
        store::save(&next, &next_graph, &root)?;
        // persist the timeline (append-only), THEN stream it — history that
        // exists only in a terminal scrollback is not history.
        let rows: Vec<(i64, String, String, String)> = events
            .iter()
            .map(|(ts, k, c, d)| (*ts, k.clone(), c.clone(), d.to_string()))
            .collect();
        let _ = store::append_events(&root, &rows);
        for ev in &events {
            print_event(ev, sub);
        }
        current = next;
        current_graph = next_graph;
    }
    Ok(())
}
