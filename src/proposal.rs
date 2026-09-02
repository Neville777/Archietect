//! The proposal protocol — the only door through which a change (AI- or
//! human-authored: `source` just records which) can reach this repository,
//! and it never opens on its own.
//!
//! `submit` only ever writes an inert patch file under `.archietect/proposals/`.
//! `test` runs that patch through the EXISTING deterministic regression
//! suite (`invariants::check` for a decision/alias, the full `laws` +
//! `invariants` test binaries for an extractor) inside an isolated git
//! worktree — the real working tree, `Index`, and `archietect.db` are never
//! touched. `accept` requires: `status == Passed`, the patch bytes
//! unchanged since that test, AND the repository HEAD unchanged since that
//! test — before it will `git apply` to the real working tree, and even
//! then only leaves an ordinary uncommitted diff — a human runs `git diff`
//! / `git commit`. All three checks live in `accept()` itself, not the CLI,
//! so no other caller (MCP, REST, a future wrapper) can skip them.
//!
//! `check_scope` is the other half of the trust boundary: a patch is
//! rejected outright if it touches anything outside its kind's fixed
//! allow-list, and unconditionally rejected if it touches the validation
//! machinery itself (`src/laws.rs`, `tests/laws.rs`, `tests/invariants.rs`,
//! `src/invariants.rs`, `src/proposal.rs`, `src/model.rs`, `src/store.rs`,
//! `Cargo.toml`/`Cargo.lock`, `laws/`, `.github/`). Without that, a
//! proposal could pass `test` by editing the test it's judged against —
//! "I fixed the test" is not an acceptable answer to "how does this pass."
//!
//! This is deliberately the only new capability here. This module adds no
//! writer for `Index`/`archietect.db`: a proposal is work, never evidence.
//! `Tier::Inferred` now exists (added for the unstructured-domain
//! vocabulary — see `model::Tier`'s doc, SYSTEM_MEMORY.md), but that changes
//! nothing about this boundary: `src/model.rs` is itself on `check_scope`'s
//! forbidden list above, so no proposal can ever edit it to grant itself a
//! new evidence-writing capability, and this module still has no code path
//! that constructs an `Evidence` of any tier and persists it. See `query.rs`'s
//! `ai_investigation` / `escalation` fields for the companion, ephemeral
//! half of this boundary — a one-off finding that never gets this far.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// New src/structural.rs extractor code (and its registration/tests).
    Extractor,
    /// A new archietect.toml [[decision]] entry.
    Decision,
    /// A new archietect.toml [aliases] entry.
    Alias,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pending,
    Passed,
    Failed,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Meta {
    id: u64,
    kind: Kind,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    language: Option<String>,
    /// Optional path to a real repo to preview an extractor proposal
    /// against — informational only, never gates pass/fail.
    #[serde(default)]
    preview_repo: Option<String>,
    /// Who/what authored this — "ai", "human", "archietect", a tool name...
    /// Free text: the mechanism doesn't care about the source, only about
    /// what the patch touches and whether it passes.
    #[serde(default = "default_source")]
    source: String,
    created_ms: i64,
    status: Status,
    /// Fingerprint of patch.diff at submit/test time — accept() refuses if
    /// the patch has changed since the last passing test.
    patch_hash: String,
    /// Repository HEAD (git rev-parse) at the moment of the last `test`
    /// run — accept() refuses if HEAD has moved since, forcing a re-test
    /// against whatever the repository looks like now.
    #[serde(default)]
    tested_head: Option<String>,
}

fn default_source() -> String {
    "unspecified".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ResultReport {
    ran_ms: i64,
    passed: bool,
    summary: String,
    #[serde(default)]
    log_tail: String,
    #[serde(default)]
    preview: Option<Value>,
}

fn proposals_root(root: &Path) -> PathBuf {
    root.join(".archietect").join("proposals")
}
fn proposal_dir(root: &Path, id: u64) -> PathBuf {
    proposals_root(root).join(id.to_string())
}
fn meta_path(root: &Path, id: u64) -> PathBuf {
    proposal_dir(root, id).join("meta.toml")
}
fn patch_path(root: &Path, id: u64) -> PathBuf {
    proposal_dir(root, id).join("patch.diff")
}
fn result_path(root: &Path, id: u64) -> PathBuf {
    proposal_dir(root, id).join("result.toml")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Not cryptographic — this only needs to detect "the patch changed since
/// the last test run", not resist tampering.
fn hash_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    Ok(format!("{:016x}", h.finish()))
}

fn current_head(root: &Path) -> Result<String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .context("failed to run `git rev-parse HEAD`")?;
    anyhow::ensure!(out.status.success(), "git rev-parse HEAD failed: {}", String::from_utf8_lossy(&out.stderr));
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Every path a unified diff touches, old and new name (so a rename or
/// delete is caught too), taken from `diff --git a/X b/Y` headers with a
/// fallback to `+++`/`---` lines for patches lacking the git extended
/// header.
fn touched_files(patch_text: &str) -> Vec<String> {
    let git_re = regex::Regex::new(r"^diff --git a/(.+?) b/(.+)$").unwrap();
    let mut files = std::collections::BTreeSet::new();
    for line in patch_text.lines() {
        if let Some(c) = git_re.captures(line) {
            files.insert(c[1].to_string());
            files.insert(c[2].to_string());
        } else if let Some(rest) = line.strip_prefix("+++ b/") {
            files.insert(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("--- a/") {
            files.insert(rest.trim().to_string());
        }
    }
    files.into_iter().collect()
}

/// Files no proposal, of any kind, may ever touch — the validation
/// machinery a proposal is judged by, plus the supply-chain / CI surface.
const FORBIDDEN_EXACT: &[&str] = &[
    "src/proposal.rs",
    "src/laws.rs",
    "src/invariants.rs",
    "src/store.rs",
    "src/model.rs",
    "tests/laws.rs",
    "tests/invariants.rs",
    "Cargo.toml",
    "Cargo.lock",
];
const FORBIDDEN_PREFIX: &[&str] = &[".github/", ".git/", "laws/"];

/// Reject a patch that touches anything outside its kind's allow-list, or
/// anything in `FORBIDDEN_EXACT`/`FORBIDDEN_PREFIX` regardless of kind.
/// Checked at the start of BOTH `test` and `accept` — cheap, and closes the
/// gap where a proposal weakens the very suite it's about to be judged by.
fn check_scope(kind: Kind, patch_text: &str) -> Result<()> {
    let files = touched_files(patch_text);
    anyhow::ensure!(!files.is_empty(), "patch does not appear to touch any files");
    for f in &files {
        let p = std::path::Path::new(f);
        let escapes = p.is_absolute()
            || p.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            });
        if escapes {
            bail!("proposal touches a path outside the repository: '{f}'");
        }
        if FORBIDDEN_EXACT.contains(&f.as_str()) || FORBIDDEN_PREFIX.iter().any(|p| f.starts_with(p)) {
            bail!(
                "proposal touches '{f}', which is part of Archietect's own validation machinery \
                 and is never permitted in a proposal patch"
            );
        }
    }
    match kind {
        Kind::Decision | Kind::Alias => {
            for f in &files {
                if f != "archietect.toml" {
                    bail!("a decision/alias proposal may only touch archietect.toml — this patch also touches '{f}'");
                }
            }
        }
        Kind::Extractor => {
            for f in &files {
                let allowed = f == "src/structural.rs" || f.starts_with("tests/fixtures/") || f.starts_with("validation/");
                if !allowed {
                    bail!(
                        "an extractor proposal may only touch src/structural.rs, tests/fixtures/**, \
                         or validation/** — this patch also touches '{f}'"
                    );
                }
            }
        }
    }
    Ok(())
}

fn tail_chars(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        chars[chars.len() - max..].iter().collect()
    }
}

fn load_meta(root: &Path, id: u64) -> Result<Meta> {
    let text = std::fs::read_to_string(meta_path(root, id))
        .with_context(|| format!("no proposal {id} (or its meta.toml is missing)"))?;
    Ok(toml::from_str(&text)?)
}

fn save_meta(root: &Path, id: u64, meta: &Meta) -> Result<()> {
    std::fs::write(meta_path(root, id), toml::to_string_pretty(meta)?)?;
    Ok(())
}

fn save_result(root: &Path, id: u64, report: &ResultReport) -> Result<()> {
    std::fs::write(result_path(root, id), toml::to_string_pretty(report)?)?;
    Ok(())
}

fn load_result(root: &Path, id: u64) -> Option<ResultReport> {
    let text = std::fs::read_to_string(result_path(root, id)).ok()?;
    toml::from_str(&text).ok()
}

fn next_id(dir: &Path) -> Result<u64> {
    let counter = dir.join("NEXT_ID");
    let cur: u64 = std::fs::read_to_string(&counter)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let next = cur + 1;
    std::fs::write(&counter, next.to_string())?;
    Ok(next)
}

#[allow(clippy::too_many_arguments)]
pub fn submit(
    root: &Path,
    kind: Kind,
    title: &str,
    description: &str,
    language: Option<&str>,
    preview_repo: Option<&str>,
    source: &str,
    patch_src: &Path,
) -> Result<Value> {
    let patch_text = std::fs::read_to_string(patch_src)
        .with_context(|| format!("reading patch file {}", patch_src.display()))?;
    anyhow::ensure!(!patch_text.trim().is_empty(), "patch file is empty");
    // Fail fast at submit time too, not just at test/accept — no point
    // writing a proposal that can never pass scope.
    check_scope(kind, &patch_text)?;

    let dir = proposals_root(root);
    std::fs::create_dir_all(&dir)?;
    let id = next_id(&dir)?;
    let pdir = proposal_dir(root, id);
    std::fs::create_dir_all(&pdir)?;
    std::fs::write(patch_path(root, id), &patch_text)?;
    let patch_hash = hash_file(&patch_path(root, id))?;

    let meta = Meta {
        id,
        kind,
        title: title.to_string(),
        description: description.to_string(),
        language: language.map(String::from),
        preview_repo: preview_repo.map(String::from),
        source: source.to_string(),
        created_ms: now_ms(),
        status: Status::Pending,
        patch_hash,
        tested_head: None,
    };
    save_meta(root, id, &meta)?;

    Ok(json!({
        "id": id,
        "kind": kind,
        "status": "pending",
        "dir": pdir.display().to_string(),
        "next": format!("archietect proposal test {id}"),
        "note": "Recommend adding .archietect/proposals/ to .gitignore — these are local, pending artifacts, not committed source, until `accept` applies one to the working tree.",
    }))
}

pub fn list(root: &Path) -> Value {
    let dir = proposals_root(root);
    let mut items = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            if !e.path().is_dir() {
                continue;
            }
            let Some(id) = e.file_name().to_str().and_then(|s| s.parse::<u64>().ok()) else {
                continue;
            };
            if let Ok(m) = load_meta(root, id) {
                items.push(json!({
                    "id": m.id,
                    "kind": m.kind,
                    "status": m.status,
                    "title": m.title,
                    "language": m.language,
                }));
            }
        }
    }
    items.sort_by_key(|v| v["id"].as_u64().unwrap_or(0));
    json!({ "proposals": items })
}

pub fn inspect(root: &Path, id: u64) -> Result<Value> {
    let meta = load_meta(root, id)?;
    let patch = std::fs::read_to_string(patch_path(root, id)).unwrap_or_default();
    let result = load_result(root, id);
    Ok(json!({ "meta": meta, "patch": patch, "result": result }))
}

/// Run the proposal through the existing regression suite inside an
/// isolated `git worktree` at HEAD — the real working tree is never
/// modified. Always cleans up the worktree, pass or fail.
pub fn test(root: &Path, id: u64) -> Result<Value> {
    let mut meta = load_meta(root, id)?;
    let patch_file = patch_path(root, id);
    let patch_text = std::fs::read_to_string(&patch_file)
        .with_context(|| format!("reading {}", patch_file.display()))?;
    check_scope(meta.kind, &patch_text)?;
    meta.patch_hash = hash_file(&patch_file)?;
    let head = current_head(root)?;

    let worktree = std::env::temp_dir().join(format!("archietect-proposal-{id}-{}", now_ms()));
    let add = Command::new("git")
        .current_dir(root)
        .args(["worktree", "add", "--detach"])
        .arg(&worktree)
        .arg("HEAD")
        .output()
        .context("failed to run `git worktree add` — is this an archietect repo under git?")?;
    if !add.status.success() {
        bail!("git worktree add failed: {}", String::from_utf8_lossy(&add.stderr));
    }

    // `git worktree add` only carries tracked content — the real-corpus
    // validation/ directory is gitignored (854MB of cloned third-party
    // repos, deliberately not committed), so `cargo test --test invariants`
    // would otherwise fail every single extractor proposal with "corpus
    // repo missing", regardless of the patch. Symlink it in read-only reuse
    // rather than copying: tests only read from it.
    let real_validation = root.join("validation");
    if real_validation.is_dir() {
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink(&real_validation, worktree.join("validation"));
        #[cfg(not(unix))]
        let _ = std::fs::create_dir_all(worktree.join("validation"));
    }

    let outcome = run_in_worktree(&worktree, &patch_file, &meta);

    let _ = Command::new("git")
        .current_dir(root)
        .args(["worktree", "remove", "--force"])
        .arg(&worktree)
        .output();
    let _ = std::fs::remove_dir_all(&worktree);

    let report = outcome?;
    meta.status = if report.passed { Status::Passed } else { Status::Failed };
    meta.tested_head = Some(head);
    save_meta(root, id, &meta)?;
    save_result(root, id, &report)?;

    Ok(json!({ "id": id, "status": meta.status, "result": report }))
}

fn run_in_worktree(worktree: &Path, patch_file: &Path, meta: &Meta) -> Result<ResultReport> {
    let apply = Command::new("git")
        .current_dir(worktree)
        .arg("apply")
        .arg(patch_file)
        .output()
        .context("failed to run `git apply`")?;
    if !apply.status.success() {
        return Ok(ResultReport {
            ran_ms: now_ms(),
            passed: false,
            summary: "patch did not apply".to_string(),
            log_tail: tail_chars(&String::from_utf8_lossy(&apply.stderr), 4000),
            preview: None,
        });
    }

    match meta.kind {
        Kind::Decision | Kind::Alias => {
            // No compilation needed: this just changes archietect.toml, so
            // an ordinary scan of the worktree already reflects it —
            // I-2/I-3 in invariants::check are exactly the safety net a
            // proposed decision/alias needs (dangling alias, dangling
            // decision link).
            let (idx, _graph) = crate::scan::scan(worktree);
            let violations = crate::invariants::check(&idx);
            if violations.is_empty() {
                Ok(ResultReport {
                    ran_ms: now_ms(),
                    passed: true,
                    summary: "invariants clean".to_string(),
                    ..Default::default()
                })
            } else {
                let detail = violations
                    .iter()
                    .map(|v| format!("[{}] {}", v.invariant, v.detail))
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(ResultReport {
                    ran_ms: now_ms(),
                    passed: false,
                    summary: format!("{} invariant violation(s)", violations.len()),
                    log_tail: detail,
                    preview: None,
                })
            }
        }
        Kind::Extractor => {
            // Compiled code changed: only the full existing regression
            // suite (laws + real-corpus invariants) can validate it. This
            // is what turns "AI broke law-013" into a hard `failed`.
            let t = Command::new("cargo")
                .current_dir(worktree)
                .args(["test", "--test", "laws", "--test", "invariants"])
                .output()
                .context("failed to run `cargo test`")?;
            let passed = t.status.success();
            let mut log = String::from_utf8_lossy(&t.stdout).to_string();
            log.push_str(&String::from_utf8_lossy(&t.stderr));
            let preview = if passed {
                preview_extractor(worktree, meta.preview_repo.as_deref())
            } else {
                None
            };
            Ok(ResultReport {
                ran_ms: now_ms(),
                passed,
                summary: if passed {
                    "laws + invariants passed".to_string()
                } else {
                    "cargo test (laws + invariants) failed".to_string()
                },
                log_tail: tail_chars(&log, 4000),
                preview,
            })
        }
    }
}

/// Informational only — never gates pass/fail. Builds the worktree's own
/// binary (so the NEW extractor code actually runs, not this process's
/// already-compiled one) and runs it against a real repo to show what it
/// would see.
fn preview_extractor(worktree: &Path, preview_repo: Option<&str>) -> Option<Value> {
    let repo = preview_repo?;
    let build = Command::new("cargo")
        .current_dir(worktree)
        .args(["build", "--quiet"])
        .output()
        .ok()?;
    if !build.status.success() {
        return Some(json!({ "error": "preview build failed" }));
    }
    let bin = worktree.join("target").join("debug").join("archietect");
    let out = Command::new(&bin).arg("--root").arg(repo).arg("status").output().ok()?;
    if !out.status.success() {
        return Some(json!({ "error": "preview run failed" }));
    }
    serde_json::from_slice::<Value>(&out.stdout).ok()
}

/// Requires a `passed` status from a `test` run whose patch is byte-for-byte
/// what's still on disk. Applies to the REAL working tree — uncommitted.
/// Never runs `git commit`.
pub fn accept(root: &Path, id: u64) -> Result<Value> {
    let mut meta = load_meta(root, id)?;
    if meta.status != Status::Passed {
        bail!(
            "proposal {id} is '{:?}', not 'passed' — run `archietect proposal test {id}` first",
            meta.status
        );
    }
    let patch_file = patch_path(root, id);
    let current_hash = hash_file(&patch_file)?;
    if current_hash != meta.patch_hash {
        bail!("patch.diff changed since the last passing test — re-run `archietect proposal test {id}`");
    }
    let patch_text = std::fs::read_to_string(&patch_file)
        .with_context(|| format!("reading {}", patch_file.display()))?;
    check_scope(meta.kind, &patch_text)?;

    let head_now = current_head(root)?;
    match &meta.tested_head {
        Some(h) if *h == head_now => {}
        Some(h) => bail!(
            "repository HEAD has moved since this proposal was tested (tested against {h}, now at \
             {head_now}) — re-run `archietect proposal test {id}`"
        ),
        None => bail!("proposal {id} has no recorded tested HEAD — run `archietect proposal test {id}` first"),
    }

    let apply = Command::new("git")
        .current_dir(root)
        .arg("apply")
        .arg(&patch_file)
        .output()
        .context("failed to run `git apply`")?;
    if !apply.status.success() {
        bail!("git apply to the working tree failed: {}", String::from_utf8_lossy(&apply.stderr));
    }

    meta.status = Status::Accepted;
    save_meta(root, id, &meta)?;

    let mut note = "Applied to the working tree, uncommitted. Review with `git diff` and commit \
        when ready — Archietect does not commit on your behalf."
        .to_string();
    if meta.kind == Kind::Extractor {
        note.push_str(" A rebuild (`cargo build`) is needed before the new extractor is live.");
    }
    Ok(json!({ "id": id, "status": "accepted", "note": note }))
}

pub fn reject(root: &Path, id: u64, purge: bool) -> Result<Value> {
    let mut meta = load_meta(root, id)?;
    meta.status = Status::Rejected;
    save_meta(root, id, &meta)?;
    if purge {
        let _ = std::fs::remove_dir_all(proposal_dir(root, id));
    }
    Ok(json!({ "id": id, "status": "rejected", "purged": purge }))
}
