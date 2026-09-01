//! Persistence — one SQLite file in the repo root (`architect.db`).
//!
//! `init` writes it; queries load it when fresh instead of rescanning. The
//! staleness check is deliberately blunt: any source file newer than the
//! index invalidates it. A stale index that answers anyway gives confident
//! wrong answers — the exact failure this tool exists to prevent — so the
//! bias is always toward rescanning.

use crate::model::Index;
use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

/// Append architectural events (daemon-only — the daemon is the single
/// writer; queries never write). The timeline is APPEND-ONLY: history that
/// can be rewritten is not history.
pub fn append_events(root: &Path, events: &[(i64, String, String, String)]) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    let conn = Connection::open(root.join("architect.db"))?;
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
    let db = root.join("architect.db");
    // Existence check BEFORE open: SQLite CREATES a file on open, so an
    // unchecked open turns this read into a write — the glance was leaving
    // 0-byte architect.db droppings wherever it ran, and each dropping then
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

/// Bump and return the ARCHITECTURE version — a monotonic counter that
/// advances only when the concept SET changes (a new concept, a removed one,
/// a rename). Like a migration number, but for architectural knowledge:
/// "v12 → v13: + WebsiteStats, - CheckoutSession". Daemon-only write.
pub fn bump_arch_version(root: &Path) -> Result<i64> {
    let conn = Connection::open(root.join("architect.db"))?;
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
    let db_path = root.join("architect.db");
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
    let db_path = root.join("architect.db");
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

