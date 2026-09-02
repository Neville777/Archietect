//! First non-code domain, added to prove `resource::Resource`/`Relationship`
//! generalize past code without changing shape. See SYSTEM_MEMORY.md, phase
//! 3 of the rollout ("First non-code domain as a proof of the
//! generalization... git").
//!
//! Still not wired into any existing CLI/REST/MCP command — that remains a
//! separate decision, not an accidental omission (see the phase-3 report).
//! What HAS changed since phase 3: this domain is now gated by
//! `permissions::PermissionConfig` (`scan_if_allowed`, below) — phase 3
//! shipped before that permission model existed, so `scan` originally ran
//! unconditionally.
//!
//! Reads `.git/HEAD` and `.git/config` directly rather than shelling out to
//! `git` — both are small, stable, well-known plaintext formats, and this
//! extractor needs none of git's actual logic (no ref resolution beyond one
//! level, no merge/rebase awareness), so a subprocess would be overhead for
//! no real benefit. `proposal.rs` shells out to `git` elsewhere in this
//! codebase because it needs `git`'s actual behavior (rev-parse, apply,
//! diff) — reading two plaintext files needs no such contract.

use crate::model::{Evidence, Tier};
use crate::resource::{Identity, Location, Resource};
use std::collections::BTreeMap;
use std::path::Path;

/// The gated entry point: checks `permissions::domain_allowed(cfg, "git")`
/// before delegating to `scan` below. This is the function any future
/// caller (CLI/REST/MCP) should use — `scan` itself remains the raw,
/// ungated primitive this module's own tests exercise directly. Retrofits
/// phase 3's git domain, which shipped before the permission model existed
/// and ran with no gate at all — see SYSTEM_MEMORY.md's "Memory boundaries"
/// section, which calls this retrofit out explicitly by name.
pub fn scan_if_allowed(cfg: &crate::permissions::PermissionConfig, root: &Path) -> Vec<Resource> {
    if !crate::permissions::domain_allowed(cfg, "git") {
        return Vec::new();
    }
    scan(root)
}

/// Every `Resource` this extractor can produce for the repo at `root`, or an
/// empty vec if `root` isn't a git repository (no `.git` directory) — this
/// extractor's `detect()` equivalent, inlined rather than implementing the
/// full `Extractor` trait sketch from SYSTEM_MEMORY.md, since that trait
/// isn't defined anywhere in this codebase yet (this phase proves ONE domain
/// fits the Resource shape; wiring a shared trait across domains is later
/// work, not this phase's job). UNGATED — see `scan_if_allowed` above for
/// the permission-checked entry point real callers should use instead.
pub fn scan(root: &Path) -> Vec<Resource> {
    let git_dir = root.join(".git");
    if !git_dir.is_dir() {
        return Vec::new();
    }

    let mut resources = Vec::new();
    let repo_name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| root.display().to_string());

    let branch = current_branch(&git_dir);

    // The repository itself. DECLARED: a .git directory existing at this
    // path is a formal, structural fact — the same strength as a schema
    // file asserting a model exists.
    let mut repo_attrs = BTreeMap::new();
    if let Some(b) = &branch {
        repo_attrs.insert("current_branch".to_string(), b.clone());
    }
    resources.push(Resource {
        id: Identity(repo_name.clone()),
        kind: "git_repository".to_string(),
        domain: "git".to_string(),
        location: Location { file: root.display().to_string(), line: None },
        attributes: repo_attrs,
        evidence: vec![Evidence {
            tier: Tier::Declared,
            what: format!("{} contains a .git directory", root.display()),
        }],
    });

    // The current branch. OBSERVED, not DECLARED: HEAD names whatever
    // branch git is ACTUALLY on right now — it's live, current state that
    // changes the moment someone runs `git checkout`, not a static
    // assertion a file makes once and holds. See model::Tier::Observed's
    // doc comment for why this is the tier's first real use.
    if let Some(b) = &branch {
        resources.push(Resource {
            id: Identity(format!("{repo_name}:{b}")),
            kind: "git_branch".to_string(),
            domain: "git".to_string(),
            location: Location { file: git_dir.join("HEAD").display().to_string(), line: None },
            attributes: BTreeMap::from([("repository".to_string(), repo_name.clone())]),
            evidence: vec![Evidence {
                tier: Tier::Observed,
                what: format!("{} currently has '{b}' checked out (.git/HEAD)", root.display()),
            }],
        });
    }

    // Remotes. DECLARED: an explicit, formal source (.git/config) asserts
    // the name and URL — nobody has to be running or connected to anything
    // for this to be true, exactly like a schema.prisma model declaration.
    for (name, url) in remotes(&git_dir) {
        resources.push(Resource {
            id: Identity(format!("{repo_name}:remote:{name}")),
            kind: "git_remote".to_string(),
            domain: "git".to_string(),
            location: Location { file: git_dir.join("config").display().to_string(), line: None },
            attributes: BTreeMap::from([
                ("repository".to_string(), repo_name.clone()),
                ("name".to_string(), name.clone()),
                ("url".to_string(), url.clone()),
            ]),
            evidence: vec![Evidence {
                tier: Tier::Declared,
                what: format!("remote '{name}' -> '{url}' declared in .git/config"),
            }],
        });
    }

    resources
}

/// The branch HEAD currently points at, or `None` for a detached HEAD (a
/// raw commit hash, not a `ref:` line) — reading a hash off HEAD confidently
/// as a "branch" would be exactly the kind of invented-fact mistake this
/// project exists to avoid, so detached HEAD yields no branch resource at
/// all rather than a wrong one.
fn current_branch(git_dir: &Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    head.strip_prefix("ref: refs/heads/").map(|b| b.to_string())
}

/// Every `[remote "name"] url = ...` pair in `.git/config`. Hand-rolled
/// rather than pulled from the `toml` dependency already in this project:
/// git's config format looks similar (`[section "sub"]`) but isn't TOML
/// (`[remote "origin"]` vs. TOML's `[remote.origin]`) — reusing the toml
/// parser here would silently mis-parse or reject a normal git config, not
/// save real work.
fn remotes(git_dir: &Path) -> Vec<(String, String)> {
    let text = match std::fs::read_to_string(git_dir.join("config")) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut current_remote: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            current_remote = rest
                .strip_prefix("remote \"")
                .and_then(|s| s.strip_suffix('"'))
                .map(|s| s.to_string());
            continue;
        }
        if let Some(name) = &current_remote {
            if let Some((key, val)) = line.split_once('=') {
                if key.trim() == "url" {
                    out.push((name.clone(), val.trim().to_string()));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs against THIS repository's own real .git directory — a real
    /// fixture, not a synthetic one, per SYSTEM_MEMORY.md's own testing
    /// discipline preference for real corpora over invented ones where the
    /// real thing is cheaply available.
    fn this_repo_root() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn scan_finds_this_repository() {
        let resources = scan(&this_repo_root());
        let repo = resources.iter().find(|r| r.kind == "git_repository");
        assert!(repo.is_some(), "expected a git_repository resource");
        assert_eq!(repo.unwrap().domain, "git");
    }

    #[test]
    fn scan_finds_origin_remote_pointing_at_archietect() {
        let resources = scan(&this_repo_root());
        let origin = resources
            .iter()
            .find(|r| r.kind == "git_remote" && r.attributes.get("name").map(String::as_str) == Some("origin"));
        assert!(origin.is_some(), "expected an 'origin' remote resource");
        let url = origin.unwrap().attributes.get("url").expect("remote resource must have a url attribute");
        assert!(
            url.to_lowercase().contains("archietect"),
            "expected origin URL to reference archietect, got: {url}"
        );
        assert_eq!(origin.unwrap().evidence[0].tier, Tier::Declared);
    }

    #[test]
    fn scan_finds_current_branch_as_observed() {
        let resources = scan(&this_repo_root());
        let branch = resources.iter().find(|r| r.kind == "git_branch");
        assert!(branch.is_some(), "expected a git_branch resource for a non-detached checkout");
        let branch = branch.unwrap();
        assert_eq!(branch.evidence[0].tier, Tier::Observed);
        assert!(!branch.attributes.get("repository").unwrap().is_empty());
    }

    #[test]
    fn scan_on_non_git_directory_returns_empty() {
        let tmp = std::env::temp_dir().join("archietect-git-domain-test-non-repo");
        let _ = std::fs::create_dir_all(&tmp);
        assert!(scan(&tmp).is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn scan_if_allowed_blocks_when_git_domain_disabled() {
        let global = std::env::temp_dir()
            .join(format!("archietect-git-domain-test-disabled-{}.toml", std::process::id()));
        std::fs::write(&global, "[domains]\ngit = \"disabled\"\n").unwrap();
        let cfg = crate::permissions::load(&global, &this_repo_root()).unwrap();

        assert!(
            scan_if_allowed(&cfg, &this_repo_root()).is_empty(),
            "an explicitly disabled git domain must yield no resources, even in a real git repo"
        );
        let _ = std::fs::remove_file(&global);
    }

    #[test]
    fn scan_if_allowed_permits_when_git_domain_enabled_by_default() {
        use crate::permissions::PermissionConfig;
        let cfg = PermissionConfig::default(); // no config anywhere -> git defaults enabled
        assert!(
            !scan_if_allowed(&cfg, &this_repo_root()).is_empty(),
            "git defaults enabled with zero config present, so this repo's own resources must appear"
        );
    }
}
