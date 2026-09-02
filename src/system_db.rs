//! System-level project registry — the pointer half of SYSTEM_MEMORY.md's
//! "two-level graph, not one flattened store" (phase 4 of that doc's
//! rollout). Lives at `~/.archietect/system.db`, entirely separate from any
//! project's own `<root>/archietect.db`.
//!
//! What this stores: a project's canonicalized root path, a display name,
//! and two timestamps (first registered, last seen). That is ALL. It never
//! stores a project's concepts, evidence, decisions, aliases, or any other
//! architectural fact — those remain exclusively in that project's own
//! archietect.db, exactly as before this phase. This is a pointer table,
//! not a cache: resist the temptation to persist a project's concept count
//! or similar summary here "for convenience" — recompute it by querying the
//! pointed-to project db live instead. See SYSTEM_MEMORY.md's "Two-level
//! graph" and "Memory boundaries" sections for why that distinction is
//! load-bearing, not stylistic.
//!
//! Registration is an explicit, deliberate act (`archietect system
//! register`) — never a side effect of `init` or any other existing
//! command. Nothing in this module is called from anywhere except the
//! `system` subcommand, so every existing command's behavior is completely
//! unaffected by this module's existence.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// `~/.archietect/system.db`, resolved once at the one real CLI call site.
/// Every other function in this module takes the db path as an explicit
/// argument instead of resolving it internally, so tests can point at a
/// tempdir file and never touch a real user's `~/.archietect/`.
pub fn default_db_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .context("could not determine home directory (HOME/USERPROFILE unset)")?;
    Ok(PathBuf::from(home).join(".archietect").join("system.db"))
}

/// A pointer to a registered project — identity/location/timestamps only,
/// never that project's actual architectural facts.
#[derive(Debug, Clone)]
pub struct ProjectPointer {
    pub root: String,
    pub name: String,
    pub first_registered_ms: i64,
    pub last_seen_ms: i64,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn open(db_path: &Path) -> Result<Connection> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let conn = Connection::open(db_path).with_context(|| format!("opening {}", db_path.display()))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS projects (
            root_path TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            first_registered_ms INTEGER NOT NULL,
            last_seen_ms INTEGER NOT NULL
        )",
    )?;
    Ok(conn)
}

/// Register `project_root` as a known project, or — if it's already known —
/// update its last-seen timestamp without touching first_registered_ms or
/// duplicating the row. Mirrors store.rs's own "re-running init never drops
/// history" convention: an upsert that preserves the original timestamp,
/// never a blind REPLACE that would reset it on every re-registration.
pub fn register_project(db_path: &Path, project_root: &Path) -> Result<ProjectPointer> {
    let canonical = project_root
        .canonicalize()
        .with_context(|| format!("resolving {}", project_root.display()))?;
    let root_str = canonical.display().to_string();
    let name = canonical
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| root_str.clone());
    let now = now_ms();

    let conn = open(db_path)?;
    conn.execute(
        "INSERT INTO projects (root_path, name, first_registered_ms, last_seen_ms)
         VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(root_path) DO UPDATE SET last_seen_ms = excluded.last_seen_ms",
        rusqlite::params![root_str, name, now],
    )?;

    let (first_registered_ms, last_seen_ms) = conn.query_row(
        "SELECT first_registered_ms, last_seen_ms FROM projects WHERE root_path = ?1",
        [&root_str],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
    )?;

    Ok(ProjectPointer { root: root_str, name, first_registered_ms, last_seen_ms })
}

/// Every registered project pointer — root path, name, and timestamps only.
/// Never opens, reads, or otherwise touches any project's own archietect.db;
/// this is exactly and only the pointer table itself.
pub fn list_projects(db_path: &Path) -> Result<Vec<ProjectPointer>> {
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let conn = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening {}", db_path.display()))?;
    let mut stmt = conn.prepare(
        "SELECT root_path, name, first_registered_ms, last_seen_ms FROM projects ORDER BY last_seen_ms DESC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ProjectPointer {
                root: r.get(0)?,
                name: r.get(1)?,
                first_registered_ms: r.get(2)?,
                last_seen_ms: r.get(3)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// One project's answer to a cross-project `query`: what its OWN db says
/// about `term`, fetched live, read-only, every call — never cached back
/// into system.db. See this module's own doc: system.db stays a pointer
/// table; this function is how a query gets to "what do my projects know"
/// without weakening that guarantee to make the question easier to ask.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectQueryResult {
    pub root: String,
    pub name: String,
    /// `None` when the pointer is stale — `root_path` no longer contains an
    /// `archietect.db` (moved, deleted, or simply never `init`'d after
    /// registration). Reported honestly, not skipped silently and not
    /// treated as a hard error that aborts every other project's result.
    pub found: Option<serde_json::Value>,
}

/// Fan out `query::concept(term)` over every registered project's own db,
/// read-only, live. Pure aggregation of an already-existing per-project
/// query function — no new cross-project matching logic, no identity
/// resolution, nothing written back to system.db or to any project's db.
/// A registered project whose `archietect.db` is missing (moved/deleted
/// since registration, or never `init`'d) is reported with `found: None`
/// rather than aborting the whole command over one bad pointer.
pub fn query_registered_projects(db_path: &Path, term: &str) -> Result<Vec<ProjectQueryResult>> {
    let pointers = list_projects(db_path)?;
    let mut out = Vec::with_capacity(pointers.len());
    for p in pointers {
        let root = PathBuf::from(&p.root);
        let (idx, graph) = crate::store::load_raw(&root);
        let found = idx.map(|idx| {
            let graph = graph.unwrap_or_default();
            crate::query::concept(&idx, &graph, term)
        });
        out.push(ProjectQueryResult { root: p.root, name: p.name, found });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every test uses its own uniquely-named tempfile (never the real
    /// `~/.archietect/system.db`) so this suite can run on any machine,
    /// including CI, without touching real user state.
    fn tmp_db(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("archietect-system-db-test-{label}-{}.db", std::process::id()))
    }

    fn tmp_project(label: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("archietect-system-db-test-proj-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn register_then_list_finds_the_project() {
        let db = tmp_db("register-list");
        let _ = std::fs::remove_file(&db);
        let proj = tmp_project("register-list");

        let pointer = register_project(&db, &proj).unwrap();
        assert_eq!(pointer.root, proj.canonicalize().unwrap().display().to_string());

        let listed = list_projects(&db).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].root, pointer.root);

        let _ = std::fs::remove_file(&db);
        let _ = std::fs::remove_dir_all(&proj);
    }

    #[test]
    fn re_registering_updates_last_seen_not_first_registered() {
        let db = tmp_db("reregister");
        let _ = std::fs::remove_file(&db);
        let proj = tmp_project("reregister");

        let first = register_project(&db, &proj).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let second = register_project(&db, &proj).unwrap();

        assert_eq!(first.first_registered_ms, second.first_registered_ms);
        assert!(second.last_seen_ms >= first.last_seen_ms);

        let listed = list_projects(&db).unwrap();
        assert_eq!(listed.len(), 1, "re-registering must not duplicate the row");

        let _ = std::fs::remove_file(&db);
        let _ = std::fs::remove_dir_all(&proj);
    }

    #[test]
    fn list_on_nonexistent_db_returns_empty() {
        let db = tmp_db("nonexistent");
        let _ = std::fs::remove_file(&db);
        assert!(list_projects(&db).unwrap().is_empty());
    }

    /// Real end-to-end: two tempdir projects, each actually `init`'d
    /// (scan::scan + store::save, exactly what `archietect init` does) with
    /// DIFFERENT declared concepts, both registered, then queried. Proves
    /// query_registered_projects genuinely opens each project's own db
    /// live rather than getting lucky on one.
    #[test]
    fn query_registered_projects_finds_the_right_project_and_reports_absence_in_the_other() {
        let db = tmp_db("query-cross-project");
        let _ = std::fs::remove_file(&db);
        let proj_a = tmp_project("query-a");
        let proj_b = tmp_project("query-b");

        std::fs::write(
            proj_a.join("schema.prisma"),
            "model Widget {\n  id Int @id @default(autoincrement())\n  name String\n}\n",
        )
        .unwrap();
        std::fs::write(
            proj_b.join("schema.prisma"),
            "model Sprocket {\n  id Int @id @default(autoincrement())\n  name String\n}\n",
        )
        .unwrap();

        let (idx_a, graph_a) = crate::scan::scan(&proj_a);
        crate::store::save(&idx_a, &graph_a, &proj_a).unwrap();
        let (idx_b, graph_b) = crate::scan::scan(&proj_b);
        crate::store::save(&idx_b, &graph_b, &proj_b).unwrap();

        register_project(&db, &proj_a).unwrap();
        register_project(&db, &proj_b).unwrap();

        let results = query_registered_projects(&db, "Widget").unwrap();
        assert_eq!(results.len(), 2, "must check every registered project, not just one");

        let a_result = results.iter().find(|r| r.root.contains(proj_a.file_name().unwrap().to_str().unwrap())).unwrap();
        let b_result = results.iter().find(|r| r.root.contains(proj_b.file_name().unwrap().to_str().unwrap())).unwrap();

        let a_verdict = a_result.found.as_ref().unwrap()["verdict"].as_str().unwrap();
        assert_ne!(a_verdict, "ABSENT", "project A declares Widget, must not be reported absent");

        let b_verdict = b_result.found.as_ref().unwrap()["verdict"].as_str().unwrap();
        assert_eq!(b_verdict, "ABSENT", "project B never declared Widget, must be genuinely checked and found absent, not just skipped");

        let _ = std::fs::remove_file(&db);
        let _ = std::fs::remove_dir_all(&proj_a);
        let _ = std::fs::remove_dir_all(&proj_b);
    }

    #[test]
    fn query_registered_projects_reports_missing_db_without_crashing() {
        let db = tmp_db("query-missing-db");
        let _ = std::fs::remove_file(&db);
        // Registered but never init'd — no archietect.db ever created here.
        let proj = tmp_project("query-missing");

        register_project(&db, &proj).unwrap();
        let results = query_registered_projects(&db, "Anything").unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].found.is_none(), "a project with no archietect.db must report found: None, not crash or fabricate a verdict");

        let _ = std::fs::remove_file(&db);
        let _ = std::fs::remove_dir_all(&proj);
    }

    #[test]
    fn distinct_projects_both_appear() {
        let db = tmp_db("distinct");
        let _ = std::fs::remove_file(&db);
        let proj_a = tmp_project("distinct-a");
        let proj_b = tmp_project("distinct-b");

        register_project(&db, &proj_a).unwrap();
        register_project(&db, &proj_b).unwrap();

        let listed = list_projects(&db).unwrap();
        assert_eq!(listed.len(), 2);

        let _ = std::fs::remove_file(&db);
        let _ = std::fs::remove_dir_all(&proj_a);
        let _ = std::fs::remove_dir_all(&proj_b);
    }
}
