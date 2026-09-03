//! The first UNSTRUCTURED domain — see SYSTEM_MEMORY.md's "Evidence has two
//! vocabularies, not one" and "Enable friction: structured vs. unstructured
//! domains". Everything before this (code, git, docker) is a STRUCTURED
//! domain: a formal, self-declaring artifact (a schema, a `.git/config`, a
//! Dockerfile) that asserts what it is. A document has no such assertion —
//! there is no schema saying what a `.pdf` "is about". So this domain:
//!
//!   - never produces DECLARED/USED/OBSERVED/NAMED evidence (the structured
//!     vocabulary) — every `Resource` here carries `Tier::Derived`, the
//!     unstructured vocabulary's tier for "structured metadata says so, not
//!     content interpretation" (see `model::Tier`'s doc).
//!   - is gated through `permissions::domain_allowed_with_confirmation`, NOT
//!     the plain `permissions::domain_allowed` that git/docker use — an
//!     explicit config entry is honored without asking, but with none, this
//!     is the first REAL (non-test) caller of the interactive confirmation
//!     flow permissions.rs built and could previously only exercise via its
//!     own fake askers.
//!   - is deliberately NOT wired into `archietect status` the way git/docker
//!     were. `status` runs on every invocation of that command; this domain
//!     needs an explicit target directory (there is no sensible default
//!     "the documents folder" to assume) and can trigger an interactive
//!     prompt, neither of which belongs firing implicitly inside a read of
//!     `status`. It gets its own explicit subcommand instead
//!     (`archietect documents scan --dir <path>`), mirroring how
//!     `archietect system register` is an explicit act, never automatic.
//!
//! ## Content is never read — this is the actual privacy boundary, not a
//! ## side effect of what happens to be implemented
//!
//! `scan` below touches exactly `std::fs::read_dir` (list filenames) and
//! `std::fs::metadata`/`DirEntry::metadata` (size, modified time). There is
//! no `File::open`, no `std::fs::read`/`read_to_string`, no byte-sniffing to
//! guess file type — classification is extension-based only. A `.pdf` that
//! is actually a renamed `.txt` file is reported as a `.pdf` by name; this
//! extractor has no opinion about what's actually inside it, on purpose.
//!
//! ## Non-recursive, single directory, explicit target only
//!
//! No default "photos folder"/"documents folder" is ever assumed — the
//! caller must pass an explicit `dir`. No recursion into subdirectories:
//! unlike `docker_domain.rs`'s root-only Dockerfile scan (which stays inside
//! one project the caller already trusted enough to run archietect on at
//! all), this domain can be pointed at an ARBITRARY directory anywhere on
//! the machine, so there is no tree-walk-exclusion convention (`.git`,
//! `node_modules`, etc.) to lean on the way `scan.rs` has for code. A single
//! flat listing keeps the blast radius of one invocation exactly as large as
//! the one directory a human explicitly named.

use crate::model::{Evidence, Tier};
use crate::permissions::{ConfirmationAsker, PermissionConfig};
use crate::resource::{Identity, Location, Resource};
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;

/// Extensions this domain recognizes as "a document" — a small, deliberately
/// unambitious list. Anything else in `dir` is simply not reported; this is
/// not a claim that other files aren't documents, only that this extractor
/// doesn't assert an opinion about them.
const DOCUMENT_EXTENSIONS: &[&str] = &["pdf", "docx", "txt", "md", "odt"];

/// The gated entry point real callers should use — checks the interactive-
/// confirmation gate (`permissions::domain_allowed_with_confirmation`) and,
/// if allowed, enforces attribute policy EXPLICITLY on the result rather
/// than relying only on `scan` never having produced anything else: strips
/// the `filename` attribute if `attribute_allowed(cfg, "documents",
/// "filename")` is false, and strips every metadata attribute
/// (extension/size/modified time) if `attribute_allowed(cfg, "documents",
/// "metadata")` is false. With permissions.rs's own defaults this never
/// removes anything (filename+metadata are the safe default), but a project
/// that explicitly restricts `[domains.documents] attributes = [...]` to a
/// narrower list must see that respected here, not just assumed from what
/// this extractor happens not to do.
///
/// Returns `(domain_allowed, resources)` — mirrors the `{"enabled": bool,
/// "resources": [...]}` shape `query::status`'s git/docker sections already
/// use, so a caller can report "the domain wasn't enabled" honestly instead
/// of an empty result being ambiguous with "enabled, but nothing found".
pub fn scan_if_allowed(
    cfg: &PermissionConfig,
    confirmations_path: &Path,
    dir: &Path,
    asker: &dyn ConfirmationAsker,
) -> Result<(bool, Vec<Resource>)> {
    let allowed =
        crate::permissions::domain_allowed_with_confirmation(cfg, confirmations_path, "documents", asker)?;
    if !allowed {
        return Ok((false, Vec::new()));
    }

    let allow_filename = crate::permissions::attribute_allowed(cfg, "documents", "filename");
    let allow_metadata = crate::permissions::attribute_allowed(cfg, "documents", "metadata");

    let mut resources = scan(dir);
    for r in &mut resources {
        if !allow_filename {
            r.attributes.remove("filename");
        }
        if !allow_metadata {
            r.attributes.remove("extension");
            r.attributes.remove("size_bytes");
            r.attributes.remove("modified_unix_ms");
        }
    }
    Ok((true, resources))
}

/// Every document `Resource` found by a flat (non-recursive) listing of
/// `dir` — filename, extension, size, and modified time only; see this
/// module's doc for why content is never touched. UNGATED — no permission
/// check, no confirmation — see `scan_if_allowed` above for the entry point
/// real callers should use instead. Kept separate so this module's own
/// tests can exercise the extraction logic directly without also having to
/// thread a `PermissionConfig`/confirmation path through every assertion,
/// the same split `git_domain.rs`/`docker_domain.rs` already use.
pub fn scan(dir: &Path) -> Vec<Resource> {
    let mut resources = Vec::new();
    let dir_name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| dir.display().to_string());

    let Ok(entries) = std::fs::read_dir(dir) else { return resources };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else { continue };
        if !file_type.is_file() {
            continue; // no recursion into subdirectories — see module doc
        }
        let path = entry.path();
        let Some(ext) = path.extension().map(|e| e.to_string_lossy().to_lowercase()) else { continue };
        if !DOCUMENT_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        // `entry.metadata()` (or `path.metadata()`, equivalent here) is the
        // ONLY filesystem call beyond the directory listing itself — no
        // `File::open`, no reading a single byte of the file's content.
        let Ok(meta) = entry.metadata() else { continue };
        let filename = entry.file_name().to_string_lossy().to_string();
        let size = meta.len();
        let modified_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64);

        let mut attributes = BTreeMap::new();
        attributes.insert("filename".to_string(), filename.clone());
        attributes.insert("extension".to_string(), ext.clone());
        attributes.insert("size_bytes".to_string(), size.to_string());
        if let Some(m) = modified_ms {
            attributes.insert("modified_unix_ms".to_string(), m.to_string());
        }

        resources.push(Resource {
            id: Identity(format!("{dir_name}:{filename}")),
            kind: "document".to_string(),
            domain: "documents".to_string(),
            location: Location { file: path.display().to_string(), line: None },
            attributes,
            evidence: vec![Evidence {
                tier: Tier::Derived,
                what: format!(
                    "file '{filename}' (.{ext}, {size} bytes) found via directory listing of '{}' — filename and metadata only, content never read",
                    dir.display()
                ),
            }],
        });
    }
    resources
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::PermissionConfig;

    struct AlwaysYes;
    impl ConfirmationAsker for AlwaysYes {
        fn answers_are_decisions(&self) -> bool {
            true
        }
        fn confirm(&self, _prompt: &str) -> bool {
            true
        }
    }
    /// A PERSON declining — a real decision, so it is durable. Contrast
    /// `NonInteractiveAsker`, whose "no" means "could not ask" and must not be.
    struct AlwaysNo;
    impl ConfirmationAsker for AlwaysNo {
        fn answers_are_decisions(&self) -> bool {
            true
        }
        fn confirm(&self, _prompt: &str) -> bool {
            false
        }
    }

    fn tmp_dir(label: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir()
            .join(format!("archietect-documents-domain-test-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn scan_finds_real_files_with_real_metadata() {
        let dir = tmp_dir("basic");
        std::fs::write(dir.join("notes.md"), b"# hello").unwrap();
        std::fs::write(dir.join("report.pdf"), b"%PDF-1.4 fake").unwrap();
        std::fs::write(dir.join("ignored.exe"), b"not a document").unwrap();

        let resources = scan(&dir);
        assert_eq!(resources.len(), 2, "expected exactly the .md and .pdf files, got: {resources:?}");

        let md = resources.iter().find(|r| r.attributes.get("extension").map(String::as_str) == Some("md"))
            .expect("expected a .md resource");
        assert_eq!(md.domain, "documents");
        assert_eq!(md.kind, "document");
        assert_eq!(md.attributes.get("filename").map(String::as_str), Some("notes.md"));
        assert_eq!(md.attributes.get("size_bytes").map(String::as_str), Some("7"));
        assert!(md.attributes.contains_key("modified_unix_ms"));
        assert_eq!(md.evidence[0].tier, Tier::Derived);
        assert!(md.evidence[0].what.contains("content never read"));

        let pdf = resources.iter().find(|r| r.attributes.get("extension").map(String::as_str) == Some("pdf"))
            .expect("expected a .pdf resource");
        assert_eq!(pdf.attributes.get("filename").map(String::as_str), Some("report.pdf"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_on_empty_or_missing_directory_returns_empty() {
        let dir = tmp_dir("empty");
        assert!(scan(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(scan(&dir).is_empty(), "a missing directory must yield empty, not panic");
    }

    #[test]
    fn scan_never_reads_content_even_for_invalid_utf8_or_large_files() {
        let dir = tmp_dir("content-safety");
        // Invalid UTF-8 bytes: if this extractor ever tried to read/parse
        // file content as a string (e.g. to sniff type or preview text),
        // this would panic or produce mangled output. It doesn't, because
        // only metadata() is ever called.
        std::fs::write(dir.join("binary.txt"), [0xFF, 0xFE, 0x00, 0xD8, 0x00, 0x00]).unwrap();
        // A file well beyond anything reasonable to read into memory for a
        // "peek at the content" — completes near-instantly because only its
        // metadata (a stat() syscall) is ever touched, not its bytes.
        let big = dir.join("big.txt");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(50 * 1024 * 1024).unwrap(); // 50MB, sparse — cheap to create, would be slow to actually read
        drop(f);

        let start = std::time::Instant::now();
        let resources = scan(&dir);
        let elapsed = start.elapsed();

        assert_eq!(resources.len(), 2);
        let big_resource = resources.iter().find(|r| r.attributes.get("filename").map(String::as_str) == Some("big.txt")).unwrap();
        assert_eq!(big_resource.attributes.get("size_bytes").map(String::as_str), Some((50 * 1024 * 1024).to_string()).as_deref());
        assert!(elapsed.as_secs() < 2, "scanning must only stat files, not read them — took {elapsed:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_does_not_recurse_into_subdirectories() {
        let dir = tmp_dir("no-recurse");
        std::fs::create_dir_all(dir.join("subdir")).unwrap();
        std::fs::write(dir.join("subdir").join("nested.md"), b"nested").unwrap();
        std::fs::write(dir.join("top.md"), b"top level").unwrap();

        let resources = scan(&dir);
        assert_eq!(resources.len(), 1, "expected only the top-level file, got: {resources:?}");
        assert_eq!(resources[0].attributes.get("filename").map(String::as_str), Some("top.md"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_if_allowed_blocks_when_confirmation_says_no() {
        let dir = tmp_dir("confirm-no");
        std::fs::write(dir.join("a.md"), b"x").unwrap();
        let confirmations = std::env::temp_dir()
            .join(format!("archietect-documents-test-confirm-no-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&confirmations);
        let cfg = PermissionConfig::default();

        let (allowed, resources) = scan_if_allowed(&cfg, &confirmations, &dir, &AlwaysNo).unwrap();
        assert!(!allowed);
        assert!(resources.is_empty());

        let _ = std::fs::remove_file(&confirmations);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_if_allowed_permits_and_persists_when_confirmation_says_yes() {
        let dir = tmp_dir("confirm-yes");
        std::fs::write(dir.join("a.md"), b"x").unwrap();
        let confirmations = std::env::temp_dir()
            .join(format!("archietect-documents-test-confirm-yes-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&confirmations);
        let cfg = PermissionConfig::default();

        let (allowed, resources) = scan_if_allowed(&cfg, &confirmations, &dir, &AlwaysYes).unwrap();
        assert!(allowed);
        assert_eq!(resources.len(), 1);

        // A second call with an asker that would say NO must not be
        // consulted — the "yes" from above is already persisted.
        let (allowed_again, _) = scan_if_allowed(&cfg, &confirmations, &dir, &AlwaysNo).unwrap();
        assert!(allowed_again, "a prior persisted 'yes' must not be re-asked and overturned silently");

        let _ = std::fs::remove_file(&confirmations);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_if_allowed_respects_explicit_attribute_restriction() {
        let dir = tmp_dir("attr-restrict");
        std::fs::write(dir.join("a.md"), b"x").unwrap();
        let confirmations = std::env::temp_dir()
            .join(format!("archietect-documents-test-attr-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&confirmations);

        let global = std::env::temp_dir()
            .join(format!("archietect-documents-test-attr-global-{}.toml", std::process::id()));
        std::fs::write(&global, "[domains.documents]\nstate = \"enabled\"\nattributes = [\"filename\"]\n").unwrap();
        let project_dir = tmp_dir("attr-restrict-project");
        let cfg = crate::permissions::load(&global, &project_dir).unwrap();

        // AlwaysNo would deny if the confirmation gate were actually
        // consulted — proving the explicit config entry above is honored
        // without prompting, consistent with permissions.rs's own contract.
        let (allowed, resources) = scan_if_allowed(&cfg, &confirmations, &dir, &AlwaysNo).unwrap();
        assert!(allowed);
        assert_eq!(resources.len(), 1);
        assert!(resources[0].attributes.contains_key("filename"));
        assert!(
            !resources[0].attributes.contains_key("size_bytes"),
            "an explicit attributes=[\"filename\"] list must exclude metadata fields, got: {:?}",
            resources[0].attributes
        );

        let _ = std::fs::remove_file(&global);
        let _ = std::fs::remove_file(&confirmations);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&project_dir);
    }
}
