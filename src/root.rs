//! Root discovery — its own subsystem, not a helper function, because it
//! backs EVERY entry point (CLI, REST, MCP, daemon, tests). Splitting it out
//! was the fix that made drift structurally impossible: before, the CLI
//! resolved a root and the subcommands each took their own `--root` flag —
//! two independent code paths that could (and did) disagree.
//!
//! One public entry point. Everything else — CLI, REST, MCP, watch — calls
//! `root::resolve(explicit)` and nothing else.

use anyhow::Context;
use std::path::{Path, PathBuf};

/// STRONG markers identify a repository root UNAMBIGUOUSLY (archietect's own
/// files, or .git). WEAK markers (Cargo.toml, package.json, ...) identify
/// *a* project but LIE inside workspaces: a single crate carries its own
/// Cargo.toml, and stopping there answers questions about one crate while
/// believing it answered for the whole repo. Found by the first
/// from-a-subdirectory test on TITAN — `crates/titan_api` has its own
/// Cargo.toml and the naive resolver stopped there. Strong beats weak at
/// ANY distance; weak is only the fallback when nothing strong exists
/// anywhere above the starting directory.
const STRONG_MARKERS: &[&str] = &["archietect.db", "archietect.toml", ".git"];
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
        // Canonicalize: an explicit relative `--root` (e.g. ".") used to
        // leak as-is into every downstream path join. Harmless for most
        // commands, but `proposal test` spawns `git apply` with its cwd set
        // to an isolated worktree — a still-relative patch path then
        // resolved against the WRONG directory and failed with "no such
        // file," found by dogfooding the proposal protocol itself.
        return r.canonicalize().with_context(|| format!("resolving root: {}", r.display()));
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

/// Cheap, shallow heuristic for "this --root probably isn't one project" —
/// built for the real-world case that surfaced it: an AI agent pointed
/// archietect at a ~42GB directory holding 40 unrelated cloned repos, and
/// the resulting full scan looked exactly like a hang — no warning, no
/// size estimate, just silence until it eventually finished. Deliberately
/// NOT a full walk of anything — that would BE the slow operation this
/// exists to warn about before paying for it. Checks immediate
/// subdirectories for their OWN `.git` one level deep, never descending
/// further; everything else about how big or slow the actual scan will be
/// is left unmeasured on purpose.
///
/// Never blocks: this returns a message for a caller to print, never a
/// prompt or a gate. A scripted/CI caller with a legitimate huge monorepo
/// (however that happens to be laid out) must never be forced through an
/// interactive confirmation it has no way to answer.
pub fn scope_warning(root: &Path) -> Option<String> {
    // A root that IS itself a project (has its own .git) is never a
    // multi-project workspace by definition, regardless of what else lives
    // under it — skip the check entirely; this is the common, expected case
    // and must never warn on it.
    if root.join(".git").exists() {
        return None;
    }
    let entries = std::fs::read_dir(root).ok()?;
    let mut nested_repos: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir() && e.path().join(".git").exists())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    const THRESHOLD: usize = 3;
    if nested_repos.len() < THRESHOLD {
        return None;
    }
    nested_repos.sort();
    let total = nested_repos.len();
    nested_repos.truncate(5);
    let more = if total > 5 { format!(" (+{} more)", total - 5) } else { String::new() };
    Some(format!(
        "'{}' looks like a directory of {total} separate projects ({}{more}), not one project — \
         archietect treats everything under --root as a SINGLE codebase, so this scan may be slow \
         and its answers won't distinguish between them. If you meant one specific project, point \
         --root at it directly.",
        root.display(),
        nested_repos.join(", "),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), "").unwrap();
    }

    #[test]
    fn strong_marker_beats_weak_at_any_distance() {
        let root = std::env::temp_dir().join(format!("archietect-root-test-{}", std::process::id()));
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
        let root = std::env::temp_dir().join(format!("archietect-root-test-explicit-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let resolved = resolve(Some(root.clone()), &std::env::temp_dir()).unwrap();
        assert_eq!(resolved, root);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn nested_repo(root: &Path, name: &str) {
        let sub = root.join(name);
        std::fs::create_dir_all(&sub).unwrap();
        touch(&sub, ".git");
    }

    #[test]
    fn warns_on_a_directory_of_several_independent_repos() {
        let root = std::env::temp_dir().join(format!("archietect-scope-test-many-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        nested_repo(&root, "project-a");
        nested_repo(&root, "project-b");
        nested_repo(&root, "project-c");

        let warning = scope_warning(&root).expect("3 nested repos must trigger the warning");
        assert!(warning.contains("3 separate projects"), "{warning}");
        assert!(warning.contains("project-a"), "{warning}");
        assert!(warning.contains("project-b"), "{warning}");
        assert!(warning.contains("project-c"), "{warning}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stays_silent_on_a_real_single_project_root() {
        let root = std::env::temp_dir().join(format!("archietect-scope-test-single-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        touch(&root, ".git"); // root itself IS a project

        assert!(scope_warning(&root).is_none(), "a root with its own .git must never warn, regardless of what's under it");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stays_silent_below_the_threshold() {
        let root = std::env::temp_dir().join(format!("archietect-scope-test-below-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        nested_repo(&root, "project-a");
        nested_repo(&root, "project-b"); // only 2 — below the 3-repo threshold

        assert!(scope_warning(&root).is_none(), "2 nested repos is a normal monorepo-adjacent layout, not the workspace-of-40 case this exists for");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn long_names_are_capped_but_the_real_count_is_stated() {
        let root = std::env::temp_dir().join(format!("archietect-scope-test-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for i in 0..8 {
            nested_repo(&root, &format!("project-{i}"));
        }

        let warning = scope_warning(&root).expect("8 nested repos must trigger the warning");
        assert!(warning.contains("8 separate projects"), "{warning}");
        assert!(warning.contains("+3 more"), "{warning}");

        let _ = std::fs::remove_dir_all(&root);
    }
}
