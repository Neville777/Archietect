//! The SECOND unstructured domain — a sibling of `documents_domain.rs`,
//! built to prove that module's discipline generalizes rather than being a
//! one-off. See SYSTEM_MEMORY.md's "Evidence has two vocabularies, not one"
//! and "Memory boundaries are the default, not an add-on". Every rule
//! documents_domain.rs established applies here unchanged:
//!
//!   - every `Resource` carries `Tier::Derived` — structured metadata says
//!     so, never content interpretation. No EXIF, no pixel data, no
//!     image-format sniffing.
//!   - gated through `permissions::domain_allowed_with_confirmation`, not
//!     the plain `permissions::domain_allowed` structured domains use.
//!   - deliberately NOT wired into `archietect status` — needs an explicit
//!     target directory and can trigger a one-time interactive y/N prompt,
//!     neither of which belongs firing implicitly on a routine status read.
//!     Its own explicit subcommand instead (`archietect photos scan --dir
//!     <path>`).
//!
//! ## Content is never read
//!
//! `scan` below touches exactly `std::fs::read_dir` (list filenames) and
//! `DirEntry::metadata()` (size, modified time). No `File::open`, no
//! `std::fs::read`, no image-crate dependency, no byte-sniffing — a `.png`
//! that is actually a renamed `.txt` file is reported as a `.png` by name;
//! this extractor has no opinion about what's actually inside it, on
//! purpose. EXIF metadata (camera model, GPS coordinates, capture time) is
//! embedded IN the file's content and would require opening and parsing
//! bytes to read — explicitly out of scope for the same reason
//! documents_domain.rs never sniffs file type from content.
//!
//! ## Non-recursive, single directory, explicit target only
//!
//! No default "photos folder" is ever assumed — the caller must pass an
//! explicit `dir`. No recursion into subdirectories, for the same reason
//! documents_domain.rs gives: this domain can be pointed at an arbitrary
//! directory anywhere on the machine, so there is no tree-walk-exclusion
//! convention to lean on the way `scan.rs` has for code.

use crate::model::{Evidence, Tier};
use crate::permissions::{ConfirmationAsker, PermissionConfig};
use crate::resource::{Identity, Location, Resource};
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;

/// Extensions this domain recognizes as "a photo" — a small, deliberately
/// unambitious list, same framing as `documents_domain::DOCUMENT_EXTENSIONS`.
/// Anything else in `dir` is simply not reported.
const PHOTO_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "heic", "webp"];

/// The gated entry point real callers should use — identical contract to
/// `documents_domain::scan_if_allowed`: checks the interactive-confirmation
/// gate, and if allowed, enforces attribute policy explicitly on the result
/// (stripping `filename` / metadata attributes a project has restricted)
/// rather than relying only on `scan` never having produced anything else.
pub fn scan_if_allowed(
    cfg: &PermissionConfig,
    confirmations_path: &Path,
    dir: &Path,
    asker: &dyn ConfirmationAsker,
) -> Result<(bool, Vec<Resource>)> {
    let allowed =
        crate::permissions::domain_allowed_with_confirmation(cfg, confirmations_path, "photos", asker)?;
    if !allowed {
        return Ok((false, Vec::new()));
    }

    let allow_filename = crate::permissions::attribute_allowed(cfg, "photos", "filename");
    let allow_metadata = crate::permissions::attribute_allowed(cfg, "photos", "metadata");

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

/// Every photo `Resource` found by a flat (non-recursive) listing of `dir`
/// — filename, extension, size, and modified time only; see this module's
/// doc for why content is never touched. UNGATED — no permission check, no
/// confirmation — see `scan_if_allowed` above for the entry point real
/// callers should use instead. Kept separate so this module's own tests can
/// exercise the extraction logic directly, the same split
/// `documents_domain.rs`/`git_domain.rs`/`docker_domain.rs` already use.
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
        if !PHOTO_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        // `entry.metadata()` is the ONLY filesystem call beyond the
        // directory listing itself — no `File::open`, no reading a single
        // byte of the file's actual image content.
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
            kind: "photo".to_string(),
            domain: "photos".to_string(),
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
            .join(format!("archietect-photos-domain-test-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn scan_finds_real_files_with_real_metadata() {
        let dir = tmp_dir("basic");
        std::fs::write(dir.join("photo.jpg"), b"fake jpeg bytes").unwrap();
        std::fs::write(dir.join("image.png"), b"fake png bytes").unwrap();
        std::fs::write(dir.join("readme.txt"), b"not a photo").unwrap();

        let resources = scan(&dir);
        assert_eq!(resources.len(), 2, "expected exactly the .jpg and .png files, got: {resources:?}");

        let jpg = resources.iter().find(|r| r.attributes.get("extension").map(String::as_str) == Some("jpg"))
            .expect("expected a .jpg resource");
        assert_eq!(jpg.domain, "photos");
        assert_eq!(jpg.kind, "photo");
        assert_eq!(jpg.attributes.get("filename").map(String::as_str), Some("photo.jpg"));
        assert_eq!(jpg.attributes.get("size_bytes").map(String::as_str), Some("15"));
        assert!(jpg.attributes.contains_key("modified_unix_ms"));
        assert_eq!(jpg.evidence[0].tier, Tier::Derived);
        assert!(jpg.evidence[0].what.contains("content never read"));

        let png = resources.iter().find(|r| r.attributes.get("extension").map(String::as_str) == Some("png"))
            .expect("expected a .png resource");
        assert_eq!(png.attributes.get("filename").map(String::as_str), Some("image.png"));

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
        // file content (e.g. to sniff an image format signature), this
        // would be exercised. It isn't, because only metadata() is called.
        std::fs::write(dir.join("binary.jpg"), [0xFF, 0xD8, 0xFF, 0x00, 0xD8, 0x00]).unwrap();
        // A file well beyond anything reasonable to read for a "peek at the
        // content" — completes near-instantly because only its metadata (a
        // stat() syscall) is ever touched, not its bytes.
        let big = dir.join("big.png");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(50 * 1024 * 1024).unwrap(); // 50MB, sparse — cheap to create, would be slow to actually read
        drop(f);

        let start = std::time::Instant::now();
        let resources = scan(&dir);
        let elapsed = start.elapsed();

        assert_eq!(resources.len(), 2);
        let big_resource = resources.iter().find(|r| r.attributes.get("filename").map(String::as_str) == Some("big.png")).unwrap();
        assert_eq!(big_resource.attributes.get("size_bytes").map(String::as_str), Some((50 * 1024 * 1024).to_string()).as_deref());
        assert!(elapsed.as_secs() < 2, "scanning must only stat files, not read them — took {elapsed:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_does_not_recurse_into_subdirectories() {
        let dir = tmp_dir("no-recurse");
        std::fs::create_dir_all(dir.join("subdir")).unwrap();
        std::fs::write(dir.join("subdir").join("nested.jpg"), b"nested").unwrap();
        std::fs::write(dir.join("top.jpg"), b"top level").unwrap();

        let resources = scan(&dir);
        assert_eq!(resources.len(), 1, "expected only the top-level file, got: {resources:?}");
        assert_eq!(resources[0].attributes.get("filename").map(String::as_str), Some("top.jpg"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_if_allowed_blocks_when_confirmation_says_no() {
        let dir = tmp_dir("confirm-no");
        std::fs::write(dir.join("a.jpg"), b"x").unwrap();
        let confirmations = std::env::temp_dir()
            .join(format!("archietect-photos-test-confirm-no-{}.toml", std::process::id()));
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
        std::fs::write(dir.join("a.jpg"), b"x").unwrap();
        let confirmations = std::env::temp_dir()
            .join(format!("archietect-photos-test-confirm-yes-{}.toml", std::process::id()));
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

    /// The bug fixed in commit 473b83f (a non-interactive "no" is not a
    /// decision and must not be persisted) must hold for THIS domain too —
    /// the fix lives in the shared `domain_allowed_with_confirmation`
    /// function, not per-domain, so this proves it applies here for free,
    /// not that it was reimplemented (there is nothing to reimplement).
    #[test]
    fn non_interactive_decline_is_not_persisted_so_a_later_interactive_run_still_asks() {
        let dir = tmp_dir("non-interactive");
        std::fs::write(dir.join("a.jpg"), b"x").unwrap();
        let confirmations = std::env::temp_dir()
            .join(format!("archietect-photos-test-non-interactive-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&confirmations);
        let cfg = PermissionConfig::default();

        let (allowed, resources) =
            scan_if_allowed(&cfg, &confirmations, &dir, &crate::permissions::NonInteractiveAsker).unwrap();
        assert!(!allowed, "fail closed with no real TTY");
        assert!(resources.is_empty());

        // A later run that CAN ask must actually be asked — not pre-empted
        // by the earlier no-TTY call's "no".
        let (allowed_again, resources_again) = scan_if_allowed(&cfg, &confirmations, &dir, &AlwaysYes).unwrap();
        assert!(allowed_again, "the interactive run must be asked, not blocked by a prior non-interactive decline");
        assert_eq!(resources_again.len(), 1);

        let _ = std::fs::remove_file(&confirmations);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_if_allowed_respects_explicit_attribute_restriction() {
        let dir = tmp_dir("attr-restrict");
        std::fs::write(dir.join("a.jpg"), b"x").unwrap();
        let confirmations = std::env::temp_dir()
            .join(format!("archietect-photos-test-attr-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&confirmations);

        let global = std::env::temp_dir()
            .join(format!("archietect-photos-test-attr-global-{}.toml", std::process::id()));
        std::fs::write(&global, "[domains.photos]\nstate = \"enabled\"\nattributes = [\"filename\"]\n").unwrap();
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
