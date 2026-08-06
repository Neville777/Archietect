//! Root discovery — its own subsystem, not a helper function, because it
//! backs EVERY entry point (CLI, REST, MCP, daemon, tests). Splitting it out
//! was the fix that made drift structurally impossible: before, the CLI
//! resolved a root and the subcommands each took their own `--root` flag —
//! two independent code paths that could (and did) disagree.
//!
//! One public entry point. Everything else — CLI, REST, MCP, watch — calls
//! `root::resolve(explicit)` and nothing else.

use std::path::{Path, PathBuf};

/// STRONG markers identify a repository root UNAMBIGUOUSLY (architect's own
/// files, or .git). WEAK markers (Cargo.toml, package.json, ...) identify
/// *a* project but LIE inside workspaces: a single crate carries its own
/// Cargo.toml, and stopping there answers questions about one crate while
/// believing it answered for the whole repo. Found by the first
/// from-a-subdirectory test on TITAN — `crates/titan_api` has its own
/// Cargo.toml and the naive resolver stopped there. Strong beats weak at
/// ANY distance; weak is only the fallback when nothing strong exists
/// anywhere above the starting directory.
const STRONG_MARKERS: &[&str] = &["architect.db", "architect.toml", ".git"];
const WEAK_MARKERS: &[&str] = &[
    "Cargo.toml", "package.json", "composer.json", "manage.py", "mix.exs",
    "go.mod", "Gemfile", "pom.xml",
];

/// Resolve the repository root ONCE: explicit override wins; otherwise the
/// NEAREST strong marker walking upward from `start`; otherwise the nearest
/// weak marker; otherwise `start` itself (a bare directory still scans — it
/// just found nothing to anchor to).
pub fn resolve(explicit: Option<PathBuf>, start: &Path) -> anyhow::Result<PathBuf> {
    if let Some(r) = explicit {
        anyhow::ensure!(r.exists(), "root does not exist: {}", r.display());
        return Ok(r);
    }
    let mut weak_hit: Option<PathBuf> = None;
    let mut dir = start.to_path_buf();
    loop {
        if STRONG_MARKERS.iter().any(|m| dir.join(m).exists()) {
            return Ok(dir);
        }
        if weak_hit.is_none() && WEAK_MARKERS.iter().any(|m| dir.join(m).exists()) {
            weak_hit = Some(dir.clone());
        }
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => return Ok(weak_hit.unwrap_or_else(|| start.to_path_buf())),
        }
    }
}

/// Convenience for entry points that mean "from the current directory".
pub fn resolve_from_cwd(explicit: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    resolve(explicit, &std::env::current_dir()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), "").unwrap();
    }

    #[test]
    fn strong_marker_beats_weak_at_any_distance() {
        let root = std::env::temp_dir().join(format!("architect-root-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let sub = root.join("crates/titan_api/src");
        std::fs::create_dir_all(&sub).unwrap();
        touch(&root, ".git"); // strong, at the true root
        touch(&root.join("crates/titan_api"), "Cargo.toml"); // weak, nearer

        let resolved = resolve(None, &sub).unwrap();
        assert_eq!(resolved, root, "weak marker in a subdirectory must not win over a strong marker above it");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn explicit_root_always_wins() {
        let root = std::env::temp_dir().join(format!("architect-root-test-explicit-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let resolved = resolve(Some(root.clone()), &std::env::temp_dir()).unwrap();
        assert_eq!(resolved, root);
        let _ = std::fs::remove_dir_all(&root);
    }
}
