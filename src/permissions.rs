//! The permission gate for every domain beyond code — see SYSTEM_MEMORY.md's
//! "Memory boundaries are the default, not an add-on" section, the
//! finalized spec this module implements exactly. This is the prerequisite
//! phase 5 (unstructured/personal-content domains) needs before it can
//! ship; this module only builds the gate itself, no new domain.
//!
//! Precedence, strict-descending:
//!   1. hardcoded denial list — never overridable by any config, project or
//!      global (see `resource_allowed`; mirrors `proposal.rs`'s
//!      `FORBIDDEN_EXACT`/`FORBIDDEN_PREFIX` shape).
//!   2. project-level `<root>/archietect.toml` `[domains]` override.
//!   3. global `~/.archietect/system.toml` `[domains]` default.
//!   4. absent from both = disabled, EXCEPT `code`/`git`, which default
//!      enabled with zero config present anywhere (the tool's original
//!      contract, and phase 3's low-sensitivity structural domain).
//!
//! Deliberately NOT wired into `src/scan.rs` (the code domain's existing hot
//! path every command already runs through) — that domain defaults enabled
//! regardless, so there is no user-visible reason to add risk there right
//! now. `src/git_domain.rs` IS retrofitted: use its new `scan_if_allowed`
//! instead of calling `scan` directly.
//!
//! `[domains]` parsing here is a SEPARATE read of `archietect.toml` from the
//! one `scan_with_prior` already does for `[aliases]`/`[[decision]]` in
//! scan.rs — deliberately not folded into that function, since touching
//! scan.rs's hot path was explicitly out of scope for this change. It uses
//! the exact same technique scan.rs already does (raw `toml::Value`, no
//! typed struct) so there is exactly one parsing IDIOM for this file in the
//! codebase, even though there are now two read call sites.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Domains formal/structural enough that editing config alone is sufficient
/// to enable them — see SYSTEM_MEMORY.md's "Enable friction" section. Any
/// domain name NOT in this list (including one this module has never heard
/// of) is treated as unstructured and requires interactive confirmation —
/// the safer default for an unrecognized name.
const STRUCTURED_DOMAINS: &[&str] = &["code", "git", "docker", "systemd"];

/// The unstructured domains SYSTEM_MEMORY.md names, for reporting purposes
/// only (`report()` below shows the full known vocabulary prospectively,
/// the same way `archietect laws` lists every law regardless of whether it
/// has ever fired in this repo). Membership in "unstructured" for GATING
/// purposes is "not in STRUCTURED_DOMAINS", not "in this list" — this list
/// exists so the report has something to show before any of these ship.
const KNOWN_UNSTRUCTURED_DOMAINS: &[&str] = &["photos", "messages", "documents", "browser"];

/// Domains enabled with zero config present anywhere. `code` was never
/// opt-in in this tool's history; `git` is phase 3's read-only, low-
/// sensitivity structural domain. Every other domain, absent from both
/// config files, is disabled — default-deny, not default-allow.
const DEFAULT_ENABLED_DOMAINS: &[&str] = &["code", "git"];

/// Path components no config, project or global, can ever re-enable
/// scanning of. Mirrors `proposal.rs`'s `FORBIDDEN_EXACT`/`FORBIDDEN_PREFIX`
/// pattern exactly: a proposal cannot weaken the suite it's judged by; a
/// config file cannot re-enable a hardcoded denial.
const FORBIDDEN_PATH_COMPONENTS: &[&str] = &[".ssh", ".aws", ".gnupg"];

/// Known browser-profile directory names/segments — never scannable
/// regardless of domain or config, since a browser profile can contain
/// session cookies and saved credentials.
const FORBIDDEN_PREFIX_DIRS: &[&str] = &[
    ".mozilla",
    "google-chrome",
    "chromium",
    "brave-browser",
    "BraveSoftware",
];

/// Filename substrings (matched case-insensitively) that mark a path as
/// credential/secret-shaped regardless of which directory it lives in.
const FORBIDDEN_FILENAME_PATTERNS: &[&str] =
    &["credential", "secret", ".pem", ".pfx", "id_rsa", "id_ed25519", "id_ecdsa"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainState {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone)]
struct DomainEntry {
    state: DomainState,
    /// `[domains.photos] attributes = [...]` — `None` means no explicit
    /// list was declared, not "everything is allowed"; see
    /// `attribute_allowed`'s doc for how that's resolved.
    attributes: Option<Vec<String>>,
}

/// The resolved view of both config layers for one `load()` call. Built
/// once per command invocation, consulted by every gate function below.
#[derive(Debug, Clone, Default)]
pub struct PermissionConfig {
    global: BTreeMap<String, DomainEntry>,
    project: BTreeMap<String, DomainEntry>,
}

fn parse_state(s: &str) -> DomainState {
    if s.eq_ignore_ascii_case("enabled") {
        DomainState::Enabled
    } else {
        DomainState::Disabled
    }
}

/// Reads `[domains]` out of the TOML file at `path`, tolerating a missing
/// or unparsable file exactly the way scan.rs already tolerates a missing
/// `archietect.toml` (an absent/broken file yields no entries, not an
/// error — the caller falls through to defaults).
fn parse_domains_table(path: &Path) -> BTreeMap<String, DomainEntry> {
    let mut out = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(path) else { return out };
    let Ok(v) = text.parse::<toml::Value>() else { return out };
    let Some(domains) = v.get("domains").and_then(|d| d.as_table()) else { return out };
    for (name, val) in domains {
        match val {
            // `code = "enabled"` — the common case.
            toml::Value::String(s) => {
                out.insert(name.to_lowercase(), DomainEntry { state: parse_state(s), attributes: None });
            }
            // `[domains.photos]` with a `state` key and optional `attributes`.
            toml::Value::Table(t) => {
                let state = t
                    .get("state")
                    .and_then(|s| s.as_str())
                    .map(parse_state)
                    .unwrap_or(DomainState::Disabled);
                let attributes = t.get("attributes").and_then(|a| a.as_array()).map(|arr| {
                    arr.iter().filter_map(|x| x.as_str().map(String::from)).collect()
                });
                out.insert(name.to_lowercase(), DomainEntry { state, attributes });
            }
            _ => {}
        }
    }
    out
}

/// `~/.archietect/system.toml` — the global `[domains]` config, meant to be
/// hand-edited by the user (unlike `confirmations.toml`, below, which is
/// tool-managed and never hand-edited).
pub fn default_global_config_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .context("could not determine home directory (HOME/USERPROFILE unset)")?;
    Ok(PathBuf::from(home).join(".archietect").join("system.toml"))
}

/// `~/.archietect/confirmations.toml` — where an unstructured-domain
/// interactive confirmation answer is persisted so it is asked at most
/// once. Tool-owned: `persist_confirmation` below rewrites this file
/// wholesale (data-preserving, but not comment-preserving) on every write,
/// which is only safe because nothing expects to hand-edit this file the
/// way `system.toml` is meant to be hand-edited.
pub fn default_confirmations_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .context("could not determine home directory (HOME/USERPROFILE unset)")?;
    Ok(PathBuf::from(home).join(".archietect").join("confirmations.toml"))
}

/// Load both config layers. `project_root` is the project directory (this
/// function joins `archietect.toml` itself, matching how `root.join(...)`
/// is already used elsewhere in this codebase for the same file).
pub fn load(global_path: &Path, project_root: &Path) -> Result<PermissionConfig> {
    Ok(PermissionConfig {
        global: parse_domains_table(global_path),
        project: parse_domains_table(&project_root.join("archietect.toml")),
    })
}

/// Domain name, lowercased, treated as structured (no interactive
/// confirmation needed to enable via config alone).
pub fn is_structured_domain(domain: &str) -> bool {
    STRUCTURED_DOMAINS.contains(&domain.to_lowercase().as_str())
}

/// Plain config-precedence lookup — no interactive confirmation involved.
/// This is what `resource_allowed` and every STRUCTURED domain's extractor
/// should call. An unstructured domain with no config entry at all resolves
/// to `false` here (the safe default); use
/// `domain_allowed_with_confirmation` for the real unstructured-domain gate
/// that can ask and persist an answer.
pub fn domain_allowed(cfg: &PermissionConfig, domain: &str) -> bool {
    let domain = domain.to_lowercase();
    if let Some(entry) = cfg.project.get(&domain) {
        return entry.state == DomainState::Enabled;
    }
    if let Some(entry) = cfg.global.get(&domain) {
        return entry.state == DomainState::Enabled;
    }
    DEFAULT_ENABLED_DOMAINS.contains(&domain.as_str())
}

/// Something that can ask "enable domain X?" and get a real answer — a
/// trait so this is testable with a fake asker instead of hard-wired to
/// stdin. Mirrors `packaging/onboard.sh`'s existing daemon-install y/N
/// prompt in shape.
pub trait ConfirmationAsker {
    fn confirm(&self, prompt: &str) -> bool;
}

/// The real CLI's asker: reads one line from stdin, case-insensitively
/// accepting "y"/"yes".
pub struct InteractiveAsker;
impl ConfirmationAsker for InteractiveAsker {
    fn confirm(&self, prompt: &str) -> bool {
        use std::io::Write;
        print!("{prompt} [y/N] ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return false;
        }
        matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
    }
}

/// Fails closed unconditionally — the correct behavior with no TTY, or
/// under an explicit non-interactive flag, mirroring `onboard.sh`'s own
/// "no TTY / --non-interactive → default to no" rule for its daemon
/// prompt. Never assumes consent just because nobody could be asked.
pub struct NonInteractiveAsker;
impl ConfirmationAsker for NonInteractiveAsker {
    fn confirm(&self, _prompt: &str) -> bool {
        false
    }
}

fn load_confirmation(path: &Path, domain: &str) -> Result<Option<bool>> {
    let Ok(text) = std::fs::read_to_string(path) else { return Ok(None) };
    let Ok(v) = text.parse::<toml::Value>() else { return Ok(None) };
    Ok(v.get("confirmations").and_then(|c| c.get(domain)).and_then(|x| x.as_bool()))
}

/// Read-merge-write: preserves every other domain's prior answer (and any
/// other top-level table already in the file) — never truncates the file
/// to just this one write. Comments are NOT preserved (this file is
/// tool-managed, never hand-edited, so that tradeoff is acceptable here in
/// a way it would not be for `system.toml`).
fn persist_confirmation(path: &Path, domain: &str, allowed: bool) -> Result<()> {
    let mut root_table: toml::value::Table = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| t.parse::<toml::Value>().ok())
        .and_then(|v| v.as_table().cloned())
        .unwrap_or_default();

    let mut confirmations = root_table
        .get("confirmations")
        .and_then(|c| c.as_table())
        .cloned()
        .unwrap_or_default();
    confirmations.insert(domain.to_string(), toml::Value::Boolean(allowed));
    root_table.insert("confirmations".to_string(), toml::Value::Table(confirmations));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let text = toml::to_string_pretty(&toml::Value::Table(root_table)).context("serializing confirmations")?;
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

/// The real gate for an UNSTRUCTURED domain. Structured domains should call
/// `domain_allowed` directly instead — this function only adds interactive-
/// confirmation behavior, which structured domains don't need.
///
/// Resolution order:
///   1. An explicit config entry (project or global) for this domain IS
///      itself the deliberate act SYSTEM_MEMORY.md requires — honor it,
///      never prompt.
///   2. Otherwise, check whether this domain has already been answered
///      once (`confirmations.toml`) — if so, use that answer, never
///      re-prompt.
///   3. Otherwise, ask via `asker` and persist the answer so step 2 finds
///      it next time.
pub fn domain_allowed_with_confirmation(
    cfg: &PermissionConfig,
    confirmations_path: &Path,
    domain: &str,
    asker: &dyn ConfirmationAsker,
) -> Result<bool> {
    let domain_lc = domain.to_lowercase();
    if cfg.project.contains_key(&domain_lc) || cfg.global.contains_key(&domain_lc) {
        return Ok(domain_allowed(cfg, &domain_lc));
    }
    if let Some(answered) = load_confirmation(confirmations_path, &domain_lc)? {
        return Ok(answered);
    }
    let allowed = asker.confirm(&format!(
        "archietect: enable the '{domain_lc}' domain (unstructured/personal content)?"
    ));
    persist_confirmation(confirmations_path, &domain_lc, allowed)?;
    Ok(allowed)
}

/// Whether a specific path may ever be scanned under `domain` — the
/// hardcoded denial list is checked FIRST, unconditionally, before any
/// config is even consulted; no config, project or global, can override it.
pub fn resource_allowed(cfg: &PermissionConfig, domain: &str, path: &Path) -> bool {
    if is_hardcoded_denied(path) {
        return false;
    }
    domain_allowed(cfg, domain)
}

fn is_hardcoded_denied(path: &Path) -> bool {
    for comp in path.components() {
        if let std::path::Component::Normal(c) = comp {
            let c = c.to_string_lossy().to_lowercase();
            if FORBIDDEN_PATH_COMPONENTS.iter().any(|f| c == *f)
                || FORBIDDEN_PREFIX_DIRS.iter().any(|f| c == f.to_lowercase())
            {
                return true;
            }
        }
    }
    if let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_lowercase()) {
        if FORBIDDEN_FILENAME_PATTERNS.iter().any(|p| name.contains(p)) {
            return true;
        }
    }
    false
}

/// Whether `attribute` (e.g. "filename", "metadata", "content") may be read
/// for `domain`. Structured domains operate at content-level by nature (a
/// symbol declaration IS the content) and have no attribute restriction —
/// see SYSTEM_MEMORY.md's "Attribute scoping" section. An unstructured
/// domain with no explicit `attributes` list declared defaults to the
/// SAFEST set (filename + metadata, never content) rather than "everything"
/// — silence must never be read as "and also analyze content".
pub fn attribute_allowed(cfg: &PermissionConfig, domain: &str, attribute: &str) -> bool {
    let domain_lc = domain.to_lowercase();
    if is_structured_domain(&domain_lc) {
        return true;
    }
    let entry = cfg.project.get(&domain_lc).or_else(|| cfg.global.get(&domain_lc));
    match entry.and_then(|e| e.attributes.as_ref()) {
        Some(allowed) => allowed.iter().any(|a| a.eq_ignore_ascii_case(attribute)),
        None => attribute.eq_ignore_ascii_case("filename") || attribute.eq_ignore_ascii_case("metadata"),
    }
}

fn resolve_with_source(cfg: &PermissionConfig, domain: &str) -> (bool, &'static str) {
    if let Some(entry) = cfg.project.get(domain) {
        return (entry.state == DomainState::Enabled, "project-config");
    }
    if let Some(entry) = cfg.global.get(domain) {
        return (entry.state == DomainState::Enabled, "global-config");
    }
    if DEFAULT_ENABLED_DOMAINS.contains(&domain) {
        (true, "default-enabled")
    } else {
        (false, "default-disabled")
    }
}

/// `archietect permissions` — a read-only inspection surface, same shape as
/// `archietect laws`: shows the full known domain vocabulary prospectively
/// (most of which have no extractor yet), not just the ones actually
/// scannable today, plus the hardcoded denial list verbatim so a user can
/// see what's blocked regardless of any config.
pub fn report(cfg: &PermissionConfig) -> serde_json::Value {
    let mut domains = Vec::new();
    for &d in STRUCTURED_DOMAINS.iter().chain(KNOWN_UNSTRUCTURED_DOMAINS.iter()) {
        let (allowed, source) = resolve_with_source(cfg, d);
        domains.push(serde_json::json!({
            "domain": d,
            "structured": is_structured_domain(d),
            "allowed": allowed,
            "source": source,
        }));
    }
    serde_json::json!({
        "domains": domains,
        "hardcoded_denials": {
            "path_components": FORBIDDEN_PATH_COMPONENTS,
            "prefix_dirs": FORBIDDEN_PREFIX_DIRS,
            "filename_patterns": FORBIDDEN_FILENAME_PATTERNS,
        },
        "note": "Hardcoded denials are never overridable by any config, project or global — same pattern as proposal.rs's FORBIDDEN_EXACT/FORBIDDEN_PREFIX applied to the proposal protocol. An unstructured domain shown here as 'default-disabled' with no config entry may still be enabled at runtime via a one-time interactive confirmation (see domain_allowed_with_confirmation) — this report reflects config state only, not whether that confirmation has already happened.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_file(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("archietect-permissions-test-{label}-{}.toml", std::process::id()))
    }

    fn write_toml(path: &Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
    }

    struct AlwaysYes;
    impl ConfirmationAsker for AlwaysYes {
        fn confirm(&self, _prompt: &str) -> bool {
            true
        }
    }
    struct AlwaysNo;
    impl ConfirmationAsker for AlwaysNo {
        fn confirm(&self, _prompt: &str) -> bool {
            false
        }
    }

    #[test]
    fn defaults_enable_only_code_and_git_with_zero_config() {
        let empty = PermissionConfig::default();
        assert!(domain_allowed(&empty, "code"));
        assert!(domain_allowed(&empty, "git"));
        assert!(!domain_allowed(&empty, "docker"));
        assert!(!domain_allowed(&empty, "photos"));
    }

    #[test]
    fn project_override_beats_global() {
        let global = tmp_file("global-precedence");
        let project_dir = std::env::temp_dir().join(format!("archietect-permissions-test-proj-{}", std::process::id()));
        std::fs::create_dir_all(&project_dir).unwrap();
        write_toml(&global, "[domains]\ndocker = \"enabled\"\n");
        write_toml(&project_dir.join("archietect.toml"), "[domains]\ndocker = \"disabled\"\n");

        let cfg = load(&global, &project_dir).unwrap();
        assert!(!domain_allowed(&cfg, "docker"), "project-level disable must beat global enable");

        let _ = std::fs::remove_file(&global);
        let _ = std::fs::remove_dir_all(&project_dir);
    }

    #[test]
    fn hardcoded_denial_wins_even_if_config_tries_to_enable_it() {
        let global = tmp_file("hardcoded-denial");
        write_toml(&global, "[domains]\nfilesystem = \"enabled\"\n");
        let project_dir = std::env::temp_dir().join(format!("archietect-permissions-test-hc-{}", std::process::id()));
        std::fs::create_dir_all(&project_dir).unwrap();
        let cfg = load(&global, &project_dir).unwrap();

        assert!(domain_allowed(&cfg, "filesystem"), "sanity: the domain itself is enabled");
        let ssh_key = PathBuf::from("/home/someone/.ssh/id_rsa");
        assert!(!resource_allowed(&cfg, "filesystem", &ssh_key), "a .ssh path must never be allowed, config or no");

        let _ = std::fs::remove_file(&global);
        let _ = std::fs::remove_dir_all(&project_dir);
    }

    #[test]
    fn unstructured_domain_confirmation_blocks_by_default_and_persists_a_yes() {
        let confirmations = tmp_file("confirm-flow");
        let _ = std::fs::remove_file(&confirmations);
        let empty = PermissionConfig::default();

        // First call, nobody has answered yet, asker says no.
        let first = domain_allowed_with_confirmation(&empty, &confirmations, "photos", &AlwaysNo).unwrap();
        assert!(!first);

        // A second, independent asker that would say yes must NOT be
        // consulted — the "no" from above is already persisted.
        let second = domain_allowed_with_confirmation(&empty, &confirmations, "photos", &AlwaysYes).unwrap();
        assert!(!second, "a prior persisted 'no' must not be re-asked and overturned silently");

        let _ = std::fs::remove_file(&confirmations);
    }

    #[test]
    fn unstructured_domain_confirmation_persists_a_yes_and_is_not_reasked() {
        let confirmations = tmp_file("confirm-yes");
        let _ = std::fs::remove_file(&confirmations);
        let empty = PermissionConfig::default();

        let first = domain_allowed_with_confirmation(&empty, &confirmations, "messages", &AlwaysYes).unwrap();
        assert!(first);

        let second = domain_allowed_with_confirmation(&empty, &confirmations, "messages", &AlwaysNo).unwrap();
        assert!(second, "a prior persisted 'yes' must not be re-asked and overturned silently");

        let _ = std::fs::remove_file(&confirmations);
    }

    #[test]
    fn non_interactive_asker_fails_closed() {
        let confirmations = tmp_file("non-interactive");
        let _ = std::fs::remove_file(&confirmations);
        let empty = PermissionConfig::default();

        let allowed =
            domain_allowed_with_confirmation(&empty, &confirmations, "browser", &NonInteractiveAsker).unwrap();
        assert!(!allowed, "a non-interactive context must never assume consent");

        let _ = std::fs::remove_file(&confirmations);
    }

    #[test]
    fn explicit_config_entry_for_unstructured_domain_skips_the_prompt_entirely() {
        let confirmations = tmp_file("skip-prompt");
        let _ = std::fs::remove_file(&confirmations);
        let global = tmp_file("skip-prompt-global");
        write_toml(&global, "[domains]\nphotos = \"enabled\"\n");
        let project_dir = std::env::temp_dir().join(format!("archietect-permissions-test-skip-{}", std::process::id()));
        std::fs::create_dir_all(&project_dir).unwrap();
        let cfg = load(&global, &project_dir).unwrap();

        // AlwaysNo would deny if actually consulted — proving the explicit
        // config entry is honored WITHOUT calling into the asker at all.
        let allowed = domain_allowed_with_confirmation(&cfg, &confirmations, "photos", &AlwaysNo).unwrap();
        assert!(allowed, "an explicit config entry must be honored without prompting");

        let _ = std::fs::remove_file(&global);
        let _ = std::fs::remove_file(&confirmations);
        let _ = std::fs::remove_dir_all(&project_dir);
    }

    #[test]
    fn attribute_allowed_defaults_to_filename_and_metadata_only_for_unstructured() {
        let empty = PermissionConfig::default();
        assert!(attribute_allowed(&empty, "photos", "filename"));
        assert!(attribute_allowed(&empty, "photos", "metadata"));
        assert!(!attribute_allowed(&empty, "photos", "content"), "silence must never mean 'and also content'");
        // structured domains: no restriction.
        assert!(attribute_allowed(&empty, "code", "content"));
    }

    #[test]
    fn attribute_allowed_honors_explicit_list() {
        let global = tmp_file("attr-explicit");
        write_toml(
            &global,
            "[domains.photos]\nstate = \"enabled\"\nattributes = [\"filename\"]\n",
        );
        let project_dir = std::env::temp_dir().join(format!("archietect-permissions-test-attr-{}", std::process::id()));
        std::fs::create_dir_all(&project_dir).unwrap();
        let cfg = load(&global, &project_dir).unwrap();

        assert!(attribute_allowed(&cfg, "photos", "filename"));
        assert!(!attribute_allowed(&cfg, "photos", "metadata"), "an explicit list must exclude anything not named");
        assert!(!attribute_allowed(&cfg, "photos", "content"));

        let _ = std::fs::remove_file(&global);
        let _ = std::fs::remove_dir_all(&project_dir);
    }
}
