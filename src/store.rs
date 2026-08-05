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
    let Ok(conn) = Connection::open(root.join("architect.db")) else {
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

pub fn save(idx: &Index, root: &Path) -> Result<std::path::PathBuf> {
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
        "INSERT OR REPLACE INTO meta (k, v) VALUES ('written_ms', ?1)",
        [chrono_ms().to_string()],
    )?;
    Ok(db_path)
}

/// Load whatever index is stored, with NO staleness judgement — the
/// incremental scanner makes that call per file (size+mtime+extractor
/// version), which replaced the old whole-index invalidation: one touched
/// file used to throw away everything.
pub fn load_raw(root: &Path) -> Option<Index> {
    let db_path = root.join("architect.db");
    if !db_path.exists() {
        return None;
    }
    let conn = Connection::open(&db_path).ok()?;
    let doc: String = conn
        .query_row("SELECT doc FROM idx WHERE k='index'", [], |r| r.get(0))
        .ok()?;
    serde_json::from_str(&doc).ok()
}

fn chrono_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

