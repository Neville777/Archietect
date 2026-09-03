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

    let inserted = conn.execute(
        "INSERT INTO archive.events (ts_ms, kind, concept, detail)
             SELECT ts_ms, kind, concept, detail FROM events WHERE ts_ms < ?1",
        [cutoff_ms],
    )?;
    conn.execute("DELETE FROM events WHERE ts_ms < ?1", [cutoff_ms])?;
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
