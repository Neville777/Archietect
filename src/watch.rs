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

const SKIP_DIRS: &[&str] = &[
    "node_modules", ".git", ".next", "target", "dist", "build", "__pycache__",
    ".venv", "venv", ".turbo", "coverage", ".cache", "vendor",
];

fn relevant(path: &Path) -> bool {
    // never react to our own database (the daemon writing it must not wake
    // the daemon), nor to anything inside a skipped tree
    if path.file_name().and_then(|f| f.to_str()) == Some("architect.db") {
        return false;
    }
    if path.components().any(|c| {
        c.as_os_str().to_str().map(|s| SKIP_DIRS.contains(&s)).unwrap_or(false)
    }) {
        return false;
    }
    true
}

type Event = (i64, String, String, serde_json::Value); // ts, kind, concept, detail

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
fn diff_findings(old: &Index, new: &Index) -> Vec<Event> {
    let mut out = Vec::new();
    // NEW concepts — check each for collision with existing canonicals/aliases.
    for (name, c) in &new.concepts {
        if old.concepts.contains_key(name) {
            continue;
        }
        event(&mut out, "concept_appeared", name, json!({
            "declared_in": c.declared_in,
        }));
        // duplicate risk: any token of the new name resolving to an EXISTING
        // concept or declared alias is the moment of infection.
        for tok in crate::model::name_tokens(name) {
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
        if !new.concepts.contains_key(name) {
            event(&mut out, "concept_lost_storage", name, json!({
                "advice": "all declarations for this concept are gone — if unintentional, a refactor just deleted storage something may still depend on",
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
    // initial build — the daemon starts knowing.
    let mut current = scan::scan(&root);
    store::save(&current, &root)?;
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
    watcher.watch(&root, RecursiveMode::Recursive)?;

    loop {
        // block until something changes…
        if rx.recv().is_err() {
            break;
        }
        // …then debounce: absorb the burst (editors write several events per
        // save) and rescan once when the tree goes quiet.
        while rx.recv_timeout(Duration::from_millis(300)).is_ok() {}

        let next = scan::scan_with_prior(&root, Some(current.clone()));
        let events = diff_findings(&current, &next);
        store::save(&next, &root)?;
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
    }
    Ok(())
}
