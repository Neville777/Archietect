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

pub fn load(root: &Path) -> Option<Index> {
    let db_path = root.join("architect.db");
    if !db_path.exists() {
        return None;
    }
    let conn = Connection::open(&db_path).ok()?;
    let written: i64 = conn
        .query_row("SELECT v FROM meta WHERE k='written_ms'", [], |r| {
            r.get::<_, String>(0)
        })
        .ok()?
        .parse()
        .ok()?;
    // staleness: any newer source file invalidates the index
    if newest_mtime_ms(root) > written {
        return None;
    }
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

fn newest_mtime_ms(root: &Path) -> i64 {
    use walkdir::WalkDir;
    WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| {
            !(e.file_type().is_dir()
                && matches!(
                    e.file_name().to_str(),
                    Some("node_modules" | ".git" | "target" | "dist" | ".next" | "__pycache__")
                ))
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.file_name().to_str() != Some("architect.db"))
        .filter_map(|e| e.metadata().ok())
        .filter_map(|m| m.modified().ok())
        .filter_map(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .max()
        .unwrap_or(i64::MAX) // unreadable mtimes → treat as stale, rescan
}
