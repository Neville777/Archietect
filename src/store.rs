//! Persistence — one SQLite file in the repo root (`archietect.db`).
//!
//! `init` writes it; queries load it when fresh instead of rescanning. The
//! staleness check is deliberately blunt: any source file newer than the
//! index invalidates it. A stale index that answers anyway gives confident
//! wrong answers — the exact failure this tool exists to prevent — so the
//! bias is always toward rescanning.

use crate::model::Index;
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

/// Append architectural events (daemon-only — the daemon is the single
/// writer; queries never write). The timeline is APPEND-ONLY: history that
/// can be rewritten is not history.
pub fn append_events(root: &Path, events: &[(i64, String, String, String)]) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    let conn = Connection::open(root.join("archietect.db"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS events (           id INTEGER PRIMARY KEY AUTOINCREMENT,            ts_ms INTEGER NOT NULL,            kind TEXT NOT NULL,            concept TEXT NOT NULL,            detail TEXT NOT NULL)",
    )?;
    let mut stmt =
        conn.prepare("INSERT INTO events (ts_ms, kind, concept, detail) VALUES (?1, ?2, ?3, ?4)")?;
    for (ts, kind, concept, detail) in events {
        stmt.execute(rusqlite::params![ts, kind, concept, detail])?;
    }
    Ok(())
}

/// Read the architectural timeline, newest first, optionally filtered to
/// events touching one concept. This is what Git cannot answer: Git knows
/// `users.rs` changed; this knows the Authentication ARCHITECTURE changed,
/// when, and what the engine said about it at the time.
pub fn read_history(root: &Path, concept: Option<&str>, limit: usize) -> Vec<serde_json::Value> {
    let db = root.join("archietect.db");
    // Existence check BEFORE open: SQLite CREATES a file on open, so an
    // unchecked open turns this read into a write — the glance was leaving
    // 0-byte archietect.db droppings wherever it ran, and each dropping then
    // became a STRONG root marker that corrupted git-style discovery from
    // subdirectories. A read that writes poisons more than the principle.
    if !db.exists() {
        return Vec::new();
    }
    let Ok(conn) = Connection::open_with_flags(
        &db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return Vec::new();
    };
    let Ok(mut stmt) =
        conn.prepare("SELECT ts_ms, kind, concept, detail FROM events ORDER BY id DESC LIMIT ?1")
    else {
        return Vec::new();
    };
    let rows = stmt
        .query_map([limit.max(1) as i64 * 4], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map(|it| it.filter_map(|x| x.ok()).collect::<Vec<_>>())
        .unwrap_or_default();
    rows.into_iter()
        .filter(|(_, _, c, _)| {
            concept
                .map(|want| crate::model::names_concept(c, want) || c.eq_ignore_ascii_case(want))
                .unwrap_or(true)
        })
        .take(limit)
        .map(|(ts, kind, concept, detail)| {
            serde_json::json!({
                "ts_ms": ts,
                "kind": kind,
                "concept": concept,
                "detail": serde_json::from_str::<serde_json::Value>(&detail).unwrap_or(serde_json::Value::String(detail)),
            })
        })
        .collect()
}

/// Move events older than `cutoff_ms` out of the live `events` table into a
/// separate, permanent archive file — never delete-and-forget. This exists
/// because the events table is genuinely unbounded (no row is ever removed
/// by normal operation, confirmed by grep: nothing in this codebase issues
/// DELETE/VACUUM/TRUNCATE against it), found by measuring real growth
/// (this repo's own db: 25 real ci_passed events from one session, ~385
/// bytes each) and asking what happens after years of daemon activity.
///
/// The fix respects the README's own founding principle — "History is
/// append-only... history that can be rewritten is not history" — by never
/// deleting a record, only relocating it: `<root>/.archietect/
/// history-archive.db` is itself append-only (rows are only ever inserted
/// into it, across as many archive calls as a human ever runs, never
/// overwritten), and `read_archived_history` below can still read
/// everything moved there. This is a human-invoked maintenance action, the
/// same shape as `git gc` — nothing in this codebase calls this
/// automatically; it exists only because a human asked `archietect
/// history-archive` to run.
///
/// Event kinds NEVER moved by `archive_events_before`, regardless of age —
/// the declared-ontology audit trail (watch.rs's own "ONTOLOGY changes —
/// the declared layer has its own event vocabulary"). These record when a
/// human-declared decision or alias was added/removed/went stale; unlike
/// `concept_appeared`/`ci_passed`/etc. (routine, high-volume, fine to
/// relocate), losing quick visibility into "when was this decision
/// recorded" behind an extra `--include-archived` flag defeats the point of
/// a governance record. The decision/alias TEXT itself always lives in
/// archietect.toml, never in this table — this only protects the audit
/// trail of when it changed.
const PROTECTED_KINDS: &[&str] = &["decision_added", "decision_removed", "alias_introduced", "alias_removed", "stale_alias"];

/// Atomic via SQLite's own `ATTACH DATABASE`: the copy into the archive and
/// the delete from the live table happen in ONE transaction on one
/// connection, so a crash mid-operation leaves either the pre-archive state
/// (nothing moved) or the fully-completed post-archive state — never a
/// window where a row exists in neither, or in both counted twice.
pub fn archive_events_before(root: &Path, cutoff_ms: i64) -> Result<(usize, std::path::PathBuf)> {
    let live_db = root.join("archietect.db");
    anyhow::ensure!(live_db.exists(), "no archietect.db at {} — nothing to archive", root.display());

    let archive_dir = root.join(".archietect");
    std::fs::create_dir_all(&archive_dir)
        .with_context(|| format!("creating {}", archive_dir.display()))?;
    let archive_db = archive_dir.join("history-archive.db");

    // Path is escaped for SQL string-literal embedding (a lone `'` in a
    // path is legal on most filesystems, however unlikely) — ATTACH
    // DATABASE has no bind-parameter form for the filename, unlike every
    // other query in this codebase, so this is the one place a path is
    // interpolated into SQL text at all.
    let archive_db_sql = archive_db.display().to_string().replace('\'', "''");
    let conn = Connection::open(&live_db)?;
    conn.execute_batch(&format!(
        "ATTACH DATABASE '{archive_db_sql}' AS archive;
         CREATE TABLE IF NOT EXISTS archive.events (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             ts_ms INTEGER NOT NULL,
             kind TEXT NOT NULL,
             concept TEXT NOT NULL,
             detail TEXT NOT NULL
         );
         BEGIN;"
    ))?;

    // PROTECTED_KINDS values are a fixed, hardcoded compile-time constant —
    // never external input — so interpolating them into the IN (...) list
    // carries no injection risk; rusqlite has no bind-parameter form for a
    // variable-length list.
    let protected_sql = PROTECTED_KINDS.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(",");
    let where_clause = format!("ts_ms < ?1 AND kind NOT IN ({protected_sql})");
    let inserted = conn.execute(
        &format!("INSERT INTO archive.events (ts_ms, kind, concept, detail) SELECT ts_ms, kind, concept, detail FROM events WHERE {where_clause}"),
        [cutoff_ms],
    )?;
    conn.execute(&format!("DELETE FROM events WHERE {where_clause}"), [cutoff_ms])?;
    conn.execute_batch("COMMIT; DETACH DATABASE archive;")?;

    Ok((inserted, archive_db))
}

/// Read events previously moved by `archive_events_before`, same shape as
/// `read_history`. Returns empty if no archive file exists yet — archiving
/// is opt-in, so most projects never have one, and that's not an error.
pub fn read_archived_history(root: &Path, concept: Option<&str>, limit: usize) -> Vec<serde_json::Value> {
    let archive_db = root.join(".archietect").join("history-archive.db");
    if !archive_db.exists() {
        return Vec::new();
    }
    let Ok(conn) = Connection::open_with_flags(&archive_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return Vec::new();
    };
    let Ok(mut stmt) =
        conn.prepare("SELECT ts_ms, kind, concept, detail FROM events ORDER BY id DESC LIMIT ?1")
    else {
        return Vec::new();
    };
    let rows = stmt
        .query_map([limit.max(1) as i64 * 4], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
        })
        .map(|it| it.filter_map(|x| x.ok()).collect::<Vec<_>>())
        .unwrap_or_default();
    rows.into_iter()
        .filter(|(_, _, c, _)| {
            concept
                .map(|want| crate::model::names_concept(c, want) || c.eq_ignore_ascii_case(want))
                .unwrap_or(true)
        })
        .take(limit)
        .map(|(ts, kind, concept, detail)| {
            serde_json::json!({
                "ts_ms": ts,
                "kind": kind,
                "concept": concept,
                "detail": serde_json::from_str::<serde_json::Value>(&detail).unwrap_or(serde_json::Value::String(detail)),
                "archived": true,
            })
        })
        .collect()
}

/// Caps a name list for narration the same way query.rs/register.rs cap
/// evidence lists (LIST_CAP=20, `.take(15)` fields, ...) — a digest that
/// dumps every name defeats the point of narrating instead of listing.
fn join_capped(names: &[&str], cap: usize) -> String {
    if names.len() <= cap {
        names.join(", ")
    } else {
        format!("{} (+{} more)", names[..cap].join(", "), names.len() - cap)
    }
}

fn plural_s(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// A narrative-quality summary of the timeline, not a fact dump — deliberate
/// parallel to a "session consolidation" pass: still generated entirely from
/// deterministic event records (never an LLM), just grouped and phrased into
/// sentences instead of returned as a raw event list. `read_history`
/// already answers "what happened, in order"; this answers "what's the
/// story," for a limit large enough to actually see the shape of a period,
/// not just the last few rows.
pub fn history_digest(root: &Path, limit: usize) -> serde_json::Value {
    let events = read_history(root, None, limit);
    if events.is_empty() {
        return serde_json::json!({
            "available": false,
            "reason": "no history events recorded yet — the watch daemon or a CI run must produce at least one",
        });
    }

    use std::collections::BTreeMap;
    let mut by_kind: BTreeMap<String, Vec<&serde_json::Value>> = BTreeMap::new();
    for e in &events {
        by_kind.entry(e["kind"].as_str().unwrap_or("").to_string()).or_default().push(e);
    }
    let concept_names = |kind: &str| -> Vec<&str> {
        by_kind.get(kind).map(|v| v.iter().filter_map(|e| e["concept"].as_str()).collect()).unwrap_or_default()
    };

    let mut narrative: Vec<String> = Vec::new();

    let ci_passed = by_kind.get("ci_passed").map(|v| v.len()).unwrap_or(0);
    let ci_blocked = by_kind.get("ci_blocked").map(|v| v.len()).unwrap_or(0);
    if ci_passed + ci_blocked > 0 {
        narrative.push(format!(
            "CI ran {} time{}: {} passed, {} blocked.",
            ci_passed + ci_blocked, plural_s(ci_passed + ci_blocked), ci_passed, ci_blocked
        ));
    }

    let appeared = concept_names("concept_appeared");
    if !appeared.is_empty() {
        narrative.push(format!("{} new concept{} appeared: {}.", appeared.len(), plural_s(appeared.len()), join_capped(&appeared, 10)));
    }

    if let Some(v) = by_kind.get("concept_renamed") {
        let renames: Vec<String> = v.iter().map(|e| format!("{} → {}", e["detail"]["from"].as_str().unwrap_or("?"), e["detail"]["to"].as_str().unwrap_or("?"))).collect();
        let refs: Vec<&str> = renames.iter().map(|s| s.as_str()).collect();
        narrative.push(format!("{} concept{} renamed: {}.", v.len(), plural_s(v.len()), join_capped(&refs, 10)));
    }

    let dup_risk = concept_names("duplicate_concept_risk");
    if !dup_risk.is_empty() {
        narrative.push(format!(
            "{} duplicate-concept risk{} flagged: {}.",
            dup_risk.len(), plural_s(dup_risk.len()), join_capped(&dup_risk, 10)
        ));
    }

    let lost = concept_names("concept_lost_storage");
    if !lost.is_empty() {
        narrative.push(format!(
            "{} concept{} lost all storage declarations: {}.",
            lost.len(), plural_s(lost.len()), join_capped(&lost, 10)
        ));
    }

    if let Some(v) = by_kind.get("alias_introduced") {
        let names: Vec<String> = v.iter().map(|e| format!("{} → {}", e["concept"].as_str().unwrap_or("?"), e["detail"]["target"].as_str().unwrap_or("?"))).collect();
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        narrative.push(format!("{} alias{} introduced: {}.", v.len(), plural_s(v.len()), join_capped(&refs, 10)));
    }
    if let Some(v) = by_kind.get("alias_removed") {
        let names = concept_names("alias_removed");
        narrative.push(format!("{} alias{} removed: {}.", v.len(), plural_s(v.len()), join_capped(&names, 10)));
    }
    if let Some(v) = by_kind.get("decision_added") {
        let names = concept_names("decision_added");
        narrative.push(format!("{} decision{} recorded: {}.", v.len(), plural_s(v.len()), join_capped(&names, 10)));
    }
    if let Some(v) = by_kind.get("decision_removed") {
        let names = concept_names("decision_removed");
        narrative.push(format!("{} decision{} deleted (rationale lost, not just the record): {}.", v.len(), plural_s(v.len()), join_capped(&names, 10)));
    }
    let stale = concept_names("stale_alias");
    if !stale.is_empty() {
        narrative.push(format!("{} alias{} point at a concept that no longer exists: {}.", stale.len(), plural_s(stale.len()), join_capped(&stale, 10)));
    }
    // Newest-first: the first architecture_version event in this window is
    // the latest version reached, not the first bump — narrate that one.
    if let Some(v) = by_kind.get("architecture_version") {
        if let Some(latest) = v.first() {
            narrative.push(format!("Architecture version reached {}.", latest["concept"].as_str().unwrap_or("?")));
        }
    }

    let oldest_ms = events.last().and_then(|e| e["ts_ms"].as_i64());
    let newest_ms = events.first().and_then(|e| e["ts_ms"].as_i64());

    serde_json::json!({
        "available": true,
        "event_count": events.len(),
        "oldest_ms": oldest_ms,
        "oldest_label": oldest_ms.and_then(crate::humanize::age_label),
        "newest_ms": newest_ms,
        "newest_label": newest_ms.and_then(crate::humanize::age_label),
        "narrative": narrative,
        "by_kind_count": by_kind.iter().map(|(k, v)| (k.clone(), v.len())).collect::<BTreeMap<_, _>>(),
    })
}

/// Bump and return the ARCHITECTURE version — a monotonic counter that
/// advances only when the concept SET changes (a new concept, a removed one,
/// a rename). Like a migration number, but for architectural knowledge:
/// "v12 → v13: + WebsiteStats, - CheckoutSession". Daemon-only write.
pub fn bump_arch_version(root: &Path) -> Result<i64> {
    let conn = Connection::open(root.join("archietect.db"))?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT)")?;
    let cur: i64 = conn
        .query_row("SELECT v FROM meta WHERE k='arch_version'", [], |r| r.get::<_, String>(0))
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let next = cur + 1;
    conn.execute(
        "INSERT OR REPLACE INTO meta (k, v) VALUES ('arch_version', ?1)",
        [next.to_string()],
    )?;
    Ok(next)
}

/// Episodic replay — "what did concept X look like at architecture version
/// N" — scoped deliberately small after weighing the honest tradeoff: a
/// full Index snapshot per version (like `save()`'s live blob) would
/// reproduce the exact unbounded-growth risk `archive_events_before` was
/// built to fix for the events table, except worse (a whole project's
/// concept graph per row, not ~385 bytes). Instead this stores one COMPACT
/// row per concept per version — verdict, table, field names, first_seen —
/// never the full graph (relations, structural symbols, source snippets).
/// Growth is bounded by how often the concept SET actually changes, which
/// is `bump_arch_version`'s own trigger: rare compared to routine scans,
/// by construction, not by hope.
///
/// Real, disclosed limitation: this only ever has data for a project where
/// `archietect watch` (the daemon) has actually run and observed a
/// concept-set change — nothing else calls this. A project that has never
/// run the daemon has zero snapshots, forever, and `concept_at_version`
/// says so honestly rather than guessing.
pub fn snapshot_concepts_at_version(root: &Path, version: i64, ts_ms: i64, idx: &Index) -> Result<()> {
    let conn = Connection::open(root.join("archietect.db"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS concept_snapshots (
             version INTEGER NOT NULL,
             ts_ms INTEGER NOT NULL,
             concept TEXT NOT NULL,
             verdict TEXT NOT NULL,
             table_name TEXT,
             fields TEXT NOT NULL,
             first_seen_ms INTEGER NOT NULL,
             PRIMARY KEY (version, concept)
         )",
    )?;
    let mut stmt = conn.prepare(
        "INSERT OR REPLACE INTO concept_snapshots (version, ts_ms, concept, verdict, table_name, fields, first_seen_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for (name, c) in &idx.concepts {
        let verdict = if c.usage.is_empty() { "DECLARED_ONLY" } else { "ACTIVE" };
        let fields = serde_json::to_string(&c.fields).unwrap_or_else(|_| "[]".to_string());
        stmt.execute(rusqlite::params![version, ts_ms, name, verdict, c.table, fields, c.first_seen_ms])?;
    }
    Ok(())
}

/// The lookup side of episodic replay: the concept's recorded state at the
/// GIVEN version if a snapshot exists for it there, else the most recent
/// snapshot at or before that version (a concept's fields rarely change
/// between one arch-version bump and the next, but if this specific
/// version's snapshot doesn't have the concept — it hadn't appeared yet, or
/// this version bump was about a DIFFERENT concept — falling back to the
/// nearest prior recorded state is still more honest than nothing). Returns
/// None if no snapshot at or before that version mentions this concept at
/// all — including the common case of a project that never ran the daemon.
pub fn concept_at_version(root: &Path, concept: &str, version: i64) -> Option<serde_json::Value> {
    let db = root.join("archietect.db");
    if !db.exists() {
        return None;
    }
    let conn = Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    conn.query_row(
        "SELECT version, ts_ms, verdict, table_name, fields, first_seen_ms
             FROM concept_snapshots
             WHERE concept = ?1 AND version <= ?2
             ORDER BY version DESC LIMIT 1",
        rusqlite::params![concept, version],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
            ))
        },
    )
    .ok()
    .map(|(snap_version, ts_ms, verdict, table, fields, first_seen_ms)| {
        serde_json::json!({
            "concept": concept,
            "requested_version": version,
            "snapshot_version": snap_version,
            "exact_version_match": snap_version == version,
            "ts_ms": ts_ms,
            "ts_label": crate::humanize::age_label(ts_ms),
            "verdict": verdict,
            "table": table,
            "fields": serde_json::from_str::<serde_json::Value>(&fields).unwrap_or(serde_json::Value::Array(vec![])),
            "first_seen_ms": first_seen_ms,
        })
    })
}

pub fn save(idx: &Index, graph: &crate::structural::StructuralGraph, root: &Path) -> Result<std::path::PathBuf> {
    let db_path = root.join("archietect.db");
    let conn = Connection::open(&db_path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT);
         CREATE TABLE IF NOT EXISTS idx (k TEXT PRIMARY KEY, doc TEXT);",
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO idx (k, doc) VALUES ('index', ?1)",
        [serde_json::to_string(idx)?],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO idx (k, doc) VALUES ('structural', ?1)",
        [serde_json::to_string(graph)?],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO meta (k, v) VALUES ('written_ms', ?1)",
        [chrono_ms().to_string()],
    )?;
    Ok(db_path)
}

/// Load whatever index is stored, with NO staleness judgement — the
/// incremental scanner makes that call per file (size+mtime+extractor
/// version), which replaced the old whole-index invalidation: one touched
/// file used to throw away everything.
pub fn load_raw(root: &Path) -> (Option<Index>, Option<crate::structural::StructuralGraph>) {
    let db_path = root.join("archietect.db");
    if !db_path.exists() {
        return (None, None);
    }
    let conn = match Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(c) => c,
        Err(_) => return (None, None),
    };
    let idx: Option<Index> = conn
        .query_row("SELECT doc FROM idx WHERE k='index'", [], |r| r.get::<_, String>(0))
        .ok()
        .and_then(|doc| serde_json::from_str(&doc).ok());
    let graph: Option<crate::structural::StructuralGraph> = conn
        .query_row("SELECT doc FROM idx WHERE k='structural'", [], |r| r.get::<_, String>(0))
        .ok()
        .and_then(|doc| serde_json::from_str(&doc).ok());
    (idx, graph)
}

fn chrono_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}


#[cfg(test)]
mod archive_tests {
    use super::*;

    fn tmp_project(label: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("archietect-archive-test-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn event_count(db: &Path) -> i64 {
        let conn = Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0)).unwrap_or(0)
    }

    #[test]
    fn archive_moves_old_events_and_preserves_all_of_them() {
        let root = tmp_project("basic");
        append_events(&root, &[
            (1000, "concept_appeared".into(), "Old1".into(), "{}".into()),
            (2000, "concept_appeared".into(), "Old2".into(), "{}".into()),
            (5000, "concept_appeared".into(), "Recent".into(), "{}".into()),
        ]).unwrap();

        let (moved, archive_path) = archive_events_before(&root, 3000).unwrap();
        assert_eq!(moved, 2, "the two events before cutoff 3000 must be moved");

        let live_db = root.join("archietect.db");
        assert_eq!(event_count(&live_db), 1, "only the recent event should remain live");
        assert_eq!(event_count(&archive_path), 2, "both old events must be in the archive file");

        // Nothing lost: total across both locations equals what was written.
        let archived = read_archived_history(&root, None, 100);
        assert_eq!(archived.len(), 2);
        let live = read_history(&root, None, 100);
        assert_eq!(live.len(), 1);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn include_archived_history_read_merges_and_marks_archived() {
        let root = tmp_project("merge");
        append_events(&root, &[(1000, "k".into(), "A".into(), "{}".into())]).unwrap();
        archive_events_before(&root, 2000).unwrap();
        append_events(&root, &[(3000, "k".into(), "B".into(), "{}".into())]).unwrap();

        let live = read_history(&root, None, 10);
        assert_eq!(live.len(), 1, "A was archived, only B remains live");
        let archived = read_archived_history(&root, None, 10);
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0]["archived"], true);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn archiving_twice_appends_to_the_same_archive_file_not_overwrite() {
        let root = tmp_project("twice");
        append_events(&root, &[
            (1000, "k".into(), "A".into(), "{}".into()),
            (5000, "k".into(), "B".into(), "{}".into()),
        ]).unwrap();

        archive_events_before(&root, 2000).unwrap(); // moves A
        archive_events_before(&root, 6000).unwrap(); // moves B, on top of A

        let archived = read_archived_history(&root, None, 100);
        assert_eq!(archived.len(), 2, "second archive call must ADD to the archive, not replace it");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn protected_ontology_events_never_get_archived_regardless_of_age() {
        let root = tmp_project("protected");
        append_events(&root, &[
            (1000, "concept_appeared".into(), "Old".into(), "{}".into()),
            (1000, "decision_added".into(), "widget-is-canonical".into(), "{}".into()),
            (1000, "decision_removed".into(), "old-decision".into(), "{}".into()),
            (1000, "alias_introduced".into(), "gadget".into(), "{}".into()),
            (1000, "alias_removed".into(), "thing".into(), "{}".into()),
            (1000, "stale_alias".into(), "ghost".into(), "{}".into()),
        ]).unwrap();

        let (moved, _archive_path) = archive_events_before(&root, 3000).unwrap();
        assert_eq!(moved, 1, "only the non-protected concept_appeared event should move");

        let live = read_history(&root, None, 100);
        assert_eq!(live.len(), 5, "the five protected ontology events must remain live despite being old: {live:?}");
        let live_kinds: std::collections::BTreeSet<String> = live.iter().map(|e| e["kind"].as_str().unwrap().to_string()).collect();
        for k in PROTECTED_KINDS {
            assert!(live_kinds.contains(*k), "{k} must still be live, got {live_kinds:?}");
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn archive_on_project_with_no_db_returns_a_clear_error_not_a_panic() {
        let root = tmp_project("no-db");
        let result = archive_events_before(&root, 1000);
        assert!(result.is_err(), "archiving a project with no archietect.db must error cleanly");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_archived_history_on_project_with_no_archive_returns_empty() {
        let root = tmp_project("no-archive");
        assert!(read_archived_history(&root, None, 10).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod digest_tests {
    use super::*;
    use serde_json::json;

    fn tmp_project(label: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("archietect-digest-test-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn no_events_is_unavailable_not_an_empty_digest() {
        let root = tmp_project("empty");
        let d = history_digest(&root, 50);
        assert_eq!(d["available"], json!(false));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn narrates_real_events_grouped_by_kind_not_a_raw_dump() {
        let root = tmp_project("narrate");
        append_events(&root, &[
            (1_700_000_000_000, "concept_appeared".into(), "Widget".into(), "{}".into()),
            (1_700_000_001_000, "concept_appeared".into(), "Gadget".into(), "{}".into()),
            (1_700_000_002_000, "ci_passed".into(), "Widget".into(), "{}".into()),
            (1_700_000_003_000, "ci_blocked".into(), "Gadget".into(), "{}".into()),
            (1_700_000_004_000, "alias_introduced".into(), "gadget".into(), json!({"target": "Gadget"}).to_string()),
            (
                1_700_000_005_000,
                "concept_renamed".into(),
                "NewName".into(),
                json!({"from": "OldName", "to": "NewName"}).to_string(),
            ),
        ]).unwrap();

        let d = history_digest(&root, 50);
        assert_eq!(d["available"], json!(true));
        assert_eq!(d["event_count"], json!(6));
        assert!(d["oldest_label"].is_string(), "{d}");
        assert!(d["newest_label"].is_string(), "{d}");

        let narrative: Vec<String> = d["narrative"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect();
        assert!(narrative.iter().any(|s| s.contains("2 new concepts appeared") && s.contains("Widget") && s.contains("Gadget")), "{narrative:?}");
        assert!(narrative.iter().any(|s| s.contains("CI ran 2 times: 1 passed, 1 blocked")), "{narrative:?}");
        assert!(narrative.iter().any(|s| s.contains("1 alias introduced") && s.contains("gadget → Gadget")), "{narrative:?}");
        assert!(narrative.iter().any(|s| s.contains("1 concept renamed") && s.contains("OldName → NewName")), "{narrative:?}");

        assert_eq!(d["by_kind_count"]["concept_appeared"], json!(2));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn long_name_lists_are_capped_not_dumped() {
        let root = tmp_project("cap");
        let events: Vec<(i64, String, String, String)> = (0..15)
            .map(|i| (1_700_000_000_000 + i, "concept_appeared".into(), format!("Concept{i}"), "{}".into()))
            .collect();
        append_events(&root, &events).unwrap();

        let d = history_digest(&root, 50);
        let narrative: Vec<String> = d["narrative"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect();
        let line = narrative.iter().find(|s| s.contains("new concepts appeared")).expect("appeared line");
        assert!(line.contains("+5 more"), "{line}");
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod episodic_replay_tests {
    use super::*;
    use serde_json::json;

    fn tmp_project(label: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("archietect-replay-test-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn no_daemon_run_ever_means_no_snapshot_not_a_guess() {
        let root = tmp_project("never-ran");
        assert!(concept_at_version(&root, "Widget", 1).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn snapshot_round_trips_real_concept_state() {
        let root = tmp_project("roundtrip");
        std::fs::write(
            root.join("schema.prisma"),
            "model Widget {\n  id   Int    @id @default(autoincrement())\n  name String\n}\n",
        )
        .unwrap();
        let (idx, _graph) = crate::scan::scan(&root);
        assert!(idx.concepts.contains_key("Widget"));

        snapshot_concepts_at_version(&root, 7, 5_000_000, &idx).unwrap();

        let out = concept_at_version(&root, "Widget", 7).expect("snapshot must exist");
        assert_eq!(out["snapshot_version"], json!(7));
        assert_eq!(out["exact_version_match"], json!(true));
        assert_eq!(out["verdict"], json!("DECLARED_ONLY"));
        assert_eq!(out["table"], json!("Widget"));
        assert!(out["fields"].as_array().unwrap().iter().any(|f| f == "name"), "{out}");
        assert!(out["ts_label"].is_string(), "{out}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn querying_a_version_between_two_snapshots_falls_back_to_the_nearest_prior_one() {
        let root = tmp_project("fallback");
        std::fs::write(root.join("schema.prisma"), "model Widget {\n  id Int @id\n}\n").unwrap();
        let (idx, _graph) = crate::scan::scan(&root);

        snapshot_concepts_at_version(&root, 3, 1000, &idx).unwrap();
        snapshot_concepts_at_version(&root, 9, 2000, &idx).unwrap();

        // Version 6 has no snapshot of its own — must fall back to v3, not v9
        // (never look FORWARD in time) and not None (v3 genuinely exists).
        let out = concept_at_version(&root, "Widget", 6).expect("must fall back to the nearest PRIOR snapshot");
        assert_eq!(out["snapshot_version"], json!(3));
        assert_eq!(out["exact_version_match"], json!(false));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_concept_that_did_not_exist_yet_at_the_requested_version_returns_none() {
        let root = tmp_project("not-yet-existed");
        std::fs::write(root.join("schema.prisma"), "model Widget {\n  id Int @id\n}\n").unwrap();
        let (idx, _graph) = crate::scan::scan(&root);
        // Only ever snapshotted starting at version 10 — Widget effectively
        // "didn't exist" in this project's recorded history before that.
        snapshot_concepts_at_version(&root, 10, 1000, &idx).unwrap();

        assert!(concept_at_version(&root, "Widget", 5).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
