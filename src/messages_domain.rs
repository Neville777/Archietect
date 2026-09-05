//! The THIRD unstructured domain — a sibling of `documents_domain.rs` and
//! `photos_domain.rs`, but structurally different from both in one
//! important way: those two scan an EXPLICIT, human-named directory for a
//! file type. This domain has no such directory to be handed — the data it
//! cares about lives at a small set of WELL-KNOWN OS-specific paths (an
//! iMessage database, a chat app's local storage folder), so it checks
//! those instead of an arbitrary caller-supplied one.
//!
//! ## What "existence/metadata only" means here, concretely
//!
//! For a single-file store (macOS's `chat.db`): `std::fs::metadata` only —
//! size and modified time, exactly like `documents_domain.rs`. The file is
//! never opened, never queried; it is a SQLite database containing real
//! message content, and nothing here reads a single byte of it.
//!
//! For a directory-based store (Signal/WhatsApp/Slack/Discord's local
//! storage): `std::fs::metadata` on the DIRECTORY ITSELF only — its own
//! modified time. Deliberately no `read_dir` into it: listing its contents
//! would itself start revealing shape (attachment cache filenames,
//! conversation-keyed subdirectories) that isn't archietect's business to
//! know. The fact reported is "this app's local data directory exists and
//! was last touched at time T" — nothing about what's inside it.
//!
//! ## Scope: macOS and Linux only
//!
//! Matches this project's existing stated platform boundary (the daemon,
//! the release binaries, and packaging/onboard.sh's own comments already
//! exclude Windows — nothing in this project has ever been built or run
//! there). Windows paths for these apps use different environment
//! variables (%APPDATA%, not HOME) and are left unimplemented rather than
//! guessed at.
//!
//! Gated through `permissions::domain_allowed_with_confirmation`, same
//! contract as `documents_domain.rs`/`photos_domain.rs` — an explicit
//! config entry is honored without asking; with none, a real TTY is
//! required and the answer is persisted so it's asked at most once.

use crate::model::{Evidence, Tier};
use crate::permissions::{ConfirmationAsker, PermissionConfig};
use crate::resource::{Identity, Location, Resource};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
enum StoreShape {
    /// A single database file — metadata (size, mtime) is reported.
    File,
    /// A directory — only the directory's OWN mtime is reported; its
    /// contents are never listed.
    Directory,
}

struct MessageStoreSpec {
    app: &'static str,
    /// Relative to $HOME. `#[cfg]`-free: both lists are checked on every
    /// platform this runs on, since a nonexistent path is just silently
    /// not found — cheaper than conditionally compiling per OS for a set
    /// this small, and it still surfaces nothing on the platform where the
    /// path never applies.
    relative_path: &'static str,
    shape: StoreShape,
}

const KNOWN_STORES: &[MessageStoreSpec] = &[
    // macOS
    MessageStoreSpec { app: "iMessage/SMS", relative_path: "Library/Messages/chat.db", shape: StoreShape::File },
    MessageStoreSpec { app: "Signal", relative_path: "Library/Application Support/Signal", shape: StoreShape::Directory },
    MessageStoreSpec { app: "WhatsApp", relative_path: "Library/Application Support/WhatsApp", shape: StoreShape::Directory },
    MessageStoreSpec { app: "Slack", relative_path: "Library/Application Support/Slack", shape: StoreShape::Directory },
    MessageStoreSpec { app: "Discord", relative_path: "Library/Application Support/discord", shape: StoreShape::Directory },
    // Linux
    MessageStoreSpec { app: "Signal", relative_path: ".config/Signal", shape: StoreShape::Directory },
    MessageStoreSpec { app: "Slack", relative_path: ".config/Slack", shape: StoreShape::Directory },
    MessageStoreSpec { app: "Discord", relative_path: ".config/discord", shape: StoreShape::Directory },
];

/// The gated entry point real callers should use. No `dir` parameter —
/// unlike `documents_domain`/`photos_domain`, this domain has no
/// caller-named target; `home` is the one input, defaulted to `$HOME`/
/// `$USERPROFILE` by `scan_if_allowed`'s own caller the same way
/// `permissions::default_confirmations_path` resolves it, but accepted
/// here as a parameter so tests never touch a real home directory.
pub fn scan_if_allowed(
    cfg: &PermissionConfig,
    confirmations_path: &Path,
    home: &Path,
    asker: &dyn ConfirmationAsker,
) -> Result<(bool, Vec<Resource>)> {
    let allowed =
        crate::permissions::domain_allowed_with_confirmation(cfg, confirmations_path, "messages", asker)?;
    if !allowed {
        return Ok((false, Vec::new()));
    }

    let allow_filename = crate::permissions::attribute_allowed(cfg, "messages", "filename");
    let allow_metadata = crate::permissions::attribute_allowed(cfg, "messages", "metadata");

    let mut resources = scan(home);
    for r in &mut resources {
        if !allow_filename {
            r.attributes.remove("app");
        }
        if !allow_metadata {
            r.attributes.remove("size_bytes");
            r.attributes.remove("modified_unix_ms");
        }
    }
    Ok((true, resources))
}

/// The real `$HOME`/`$USERPROFILE` — resolved here (not inlined at the
/// single CLI call site) so REST and MCP can share the exact same
/// resolution logic `permissions.rs`'s own path functions use.
pub fn default_home() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .context("could not determine home directory (HOME/USERPROFILE unset)")?;
    Ok(PathBuf::from(home))
}

/// Checks every known store location under `home`. UNGATED — no
/// permission check, no confirmation; see `scan_if_allowed` above for the
/// entry point real callers should use. Kept separate so this module's
/// own tests can exercise the detection logic directly against a fake
/// temp "home" without threading a `PermissionConfig` through every
/// assertion — same split `documents_domain.rs`/`photos_domain.rs` use.
pub fn scan(home: &Path) -> Vec<Resource> {
    let mut resources = Vec::new();
    for spec in KNOWN_STORES {
        let path = home.join(spec.relative_path);
        let Ok(meta) = std::fs::metadata(&path) else { continue };

        let modified_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64);

        let mut attributes = BTreeMap::new();
        attributes.insert("app".to_string(), spec.app.to_string());
        let what = match spec.shape {
            StoreShape::File => {
                let size = meta.len();
                attributes.insert("size_bytes".to_string(), size.to_string());
                format!(
                    "'{}' local message store found at '{}' ({size} bytes) — existence and size/mtime only, never opened or queried",
                    spec.app, path.display()
                )
            }
            StoreShape::Directory => {
                format!(
                    "'{}' local data directory found at '{}' — existence and the directory's own mtime only, contents never listed",
                    spec.app, path.display()
                )
            }
        };
        if let Some(m) = modified_ms {
            attributes.insert("modified_unix_ms".to_string(), m.to_string());
        }

        resources.push(Resource {
            id: Identity(format!("{}:{}", spec.app, path.display())),
            kind: "message_store".to_string(),
            domain: "messages".to_string(),
            location: Location { file: path.display().to_string(), line: None },
            attributes,
            evidence: vec![Evidence { tier: Tier::Derived, what }],
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
    struct AlwaysNo;
    impl ConfirmationAsker for AlwaysNo {
        fn answers_are_decisions(&self) -> bool {
            true
        }
        fn confirm(&self, _prompt: &str) -> bool {
            false
        }
    }

    fn fake_home(label: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("archietect-messages-domain-test-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn finds_a_real_file_based_store_with_size_and_mtime() {
        let home = fake_home("file-store");
        std::fs::create_dir_all(home.join("Library/Messages")).unwrap();
        std::fs::write(home.join("Library/Messages/chat.db"), b"fake sqlite content").unwrap();

        let resources = scan(&home);
        let r = resources.iter().find(|r| r.attributes.get("app").map(String::as_str) == Some("iMessage/SMS"))
            .expect("expected the iMessage store to be found");
        assert_eq!(r.domain, "messages");
        assert_eq!(r.kind, "message_store");
        assert_eq!(r.attributes.get("size_bytes").map(String::as_str), Some("19"));
        assert!(r.attributes.contains_key("modified_unix_ms"));
        assert_eq!(r.evidence[0].tier, Tier::Derived);
        assert!(r.evidence[0].what.contains("never opened or queried"));

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn finds_a_directory_based_store_without_reporting_its_contents() {
        let home = fake_home("dir-store");
        let signal_dir = home.join(".config/Signal");
        std::fs::create_dir_all(&signal_dir).unwrap();
        // Real content that must NEVER be reflected in the resource, proving
        // this genuinely never lists the directory.
        std::fs::write(signal_dir.join("real-conversation-data.db"), b"secret").unwrap();
        std::fs::create_dir_all(signal_dir.join("attachments.noindex")).unwrap();

        let resources = scan(&home);
        let r = resources.iter().find(|r| r.attributes.get("app").map(String::as_str) == Some("Signal") && r.location.file.contains(".config"))
            .expect("expected the Linux Signal store to be found");
        assert_eq!(r.kind, "message_store");
        assert!(!r.attributes.contains_key("size_bytes"), "a directory-shaped store must never report a size, that would imply content was measured");
        assert!(r.evidence[0].what.contains("contents never listed"));
        // The literal filename of the real content must not leak anywhere.
        let full = serde_json::to_string(r).unwrap();
        assert!(!full.contains("real-conversation-data"), "directory contents must never appear in the resource: {full}");

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn no_stores_present_returns_empty_not_a_guess() {
        let home = fake_home("empty");
        assert!(scan(&home).is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn scan_if_allowed_blocks_when_confirmation_says_no() {
        let home = fake_home("confirm-no");
        std::fs::create_dir_all(home.join(".config/Slack")).unwrap();
        let confirmations = std::env::temp_dir()
            .join(format!("archietect-messages-test-confirm-no-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&confirmations);
        let cfg = PermissionConfig::default();

        let (allowed, resources) = scan_if_allowed(&cfg, &confirmations, &home, &AlwaysNo).unwrap();
        assert!(!allowed);
        assert!(resources.is_empty());

        let _ = std::fs::remove_file(&confirmations);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn scan_if_allowed_permits_and_persists_when_confirmation_says_yes() {
        let home = fake_home("confirm-yes");
        std::fs::create_dir_all(home.join(".config/Slack")).unwrap();
        let confirmations = std::env::temp_dir()
            .join(format!("archietect-messages-test-confirm-yes-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&confirmations);
        let cfg = PermissionConfig::default();

        let (allowed, resources) = scan_if_allowed(&cfg, &confirmations, &home, &AlwaysYes).unwrap();
        assert!(allowed);
        assert_eq!(resources.len(), 1);

        let (allowed_again, _) = scan_if_allowed(&cfg, &confirmations, &home, &AlwaysNo).unwrap();
        assert!(allowed_again, "a prior persisted 'yes' must not be re-asked and overturned silently");

        let _ = std::fs::remove_file(&confirmations);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn scan_if_allowed_respects_explicit_attribute_restriction() {
        let home = fake_home("attr-restrict");
        std::fs::create_dir_all(home.join("Library/Messages")).unwrap();
        std::fs::write(home.join("Library/Messages/chat.db"), b"x").unwrap();
        let confirmations = std::env::temp_dir()
            .join(format!("archietect-messages-test-attr-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&confirmations);

        let global = std::env::temp_dir()
            .join(format!("archietect-messages-test-attr-global-{}.toml", std::process::id()));
        std::fs::write(&global, "[domains.messages]\nstate = \"enabled\"\nattributes = [\"filename\"]\n").unwrap();
        let project_dir = fake_home("attr-restrict-project");
        let cfg = crate::permissions::load(&global, &project_dir).unwrap();

        let (allowed, resources) = scan_if_allowed(&cfg, &confirmations, &home, &AlwaysNo).unwrap();
        assert!(allowed);
        assert_eq!(resources.len(), 1);
        assert!(resources[0].attributes.contains_key("app"));
        assert!(
            !resources[0].attributes.contains_key("size_bytes"),
            "an explicit attributes=[\"filename\"] list must exclude metadata fields, got: {:?}",
            resources[0].attributes
        );

        let _ = std::fs::remove_file(&global);
        let _ = std::fs::remove_file(&confirmations);
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&project_dir);
    }
}
