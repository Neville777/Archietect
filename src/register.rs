//! `archietect register` — the map of the bag: what is known, what is NOT
//! known, why it isn't, and where the boundary is.
//!
//! See SYSTEM_MEMORY.md's vision section. A consumer must be able to tell
//! *"X does not exist"* apart from *"X cannot be established, because the
//! domain that would carry the evidence has not been looked at."*
//! Those are different answers with different next actions — the first ends
//! an investigation, the second says exactly where to go investigate. Every
//! other command answers a specific question; this one answers "before I ask
//! anything: what can this memory even speak to, and where does it go
//! silent?" Without it a helper has to rediscover the boundary one
//! `INSUFFICIENT_COVERAGE` at a time.
//!
//! Pure COMPOSITION of existing signals — nothing here is re-scanned or
//! re-derived, and no unknown is invented: every `not_known` entry is backed
//! by a signal the codebase already produced somewhere (structural coverage,
//! the permission resolver, the confirmation file, `status`'s
//! never-observed list, and the per-domain tier table below). A category
//! with zero instances for this repository is OMITTED, not emitted empty —
//! an empty unknown is noise wearing the shape of information.
//!
//! ## The tier-producibility table (`PRODUCIBLE`)
//!
//! Derived by reading each extractor's actual `Evidence { tier: ... }`
//! constructions, not from what the domain "should" know:
//!
//! - code — Declared, Used, Named (static analysis; scan.rs/structural.rs)
//! - git — Declared (repository: git_domain.rs:76; remotes: :115);
//!   Observed for the current branch only (git_domain.rs:94)
//! - docker — Declared (Dockerfile FROM: docker_domain.rs:78; compose
//!   services: :111) from `scan`/`scan_if_allowed`, PLUS Observed via the
//!   separate, explicit `docker_domain::scan_observed` (`archietect docker
//!   observe` / `/docker/observe` / `docker_observe`) — not automatic, so it
//!   earns no NOT_PRODUCIBLE row below (the tier is producible, just not
//!   free): the honest per-service gap when it IS unavailable (docker
//!   missing, daemon unreachable) is silence, not a blanket "docker has no
//!   Observed tier" claim that would now be false
//! - documents — Derived only (documents_domain.rs:158); content is never
//!   read, so Inferred is impossible by design, and no tagging mechanism
//!   exists, so Explicit is impossible today
//!
//! `tier_not_producible` entries are emitted only for ENABLED domains (a
//! disabled one is already covered by `domain_disabled`) and only for the
//! tier a consumer plausibly wants and would otherwise assume exists. The
//! code domain deliberately has no such entry: its real gap is per-concept
//! ("declared, never observed in use"), which `usage_unobserved` states with
//! the actual concept names — a blanket "code has no Observed tier" line
//! would fire identically on every repository forever and say nothing.

use crate::model::{Index, Tier};
use crate::structural::StructuralGraph;
use serde_json::{json, Value};
use std::path::Path;

/// Lists inside `not_known` are capped here, with a `*_count` beside them —
/// universal_trader has thousands of declared-only concepts, and a register
/// that dumps all of them is the whole-bag problem `shape.rs` exists to fix.
const LIST_CAP: usize = 20;

/// Domains that have an extractor in this binary. `permissions::report`
/// lists the full prospective vocabulary (systemd, photos, ...); the
/// register only reasons about domains that could actually be looked at —
/// "systemd is disabled" for a domain nothing can scan is not an unknown, it
/// is an absence of capability, and belongs in `permissions`, not here.
const IMPLEMENTED_DOMAINS: &[&str] = &["code", "git", "docker", "documents"];

/// Per domain: the tier a consumer would plausibly want that the extractor
/// cannot produce, with the honest reason and the honest way to establish it
/// without archietect. See the module doc for how each row was derived.
const NOT_PRODUCIBLE: &[(&str, Tier, &str, &str)] = &[
    (
        "git",
        Tier::Observed,
        "remotes are Declared from .git/config (git_domain.rs); only the current branch is Observed (.git/HEAD). Whether a remote is reachable, or whether the local branch is ahead of/behind it, is never observed — unknown here by construction, not 'in sync'",
        "observe it yourself: `git fetch --dry-run` / `git status -sb`. Such a fact would be Observed-tier and is not established by archietect today",
    ),
    (
        "documents",
        Tier::Explicit,
        "no mechanism exists for a user to tag or label a document; the extractor produces Derived-tier facts only (filename/extension/size/mtime, documents_domain.rs)",
        "archietect cannot help today: there is no tagging surface. A user-asserted fact about a document would be Explicit-tier and has nowhere to be recorded yet",
    ),
    (
        "documents",
        Tier::Inferred,
        "content is never read (documents_domain.rs: only read_dir + metadata), so nothing about what a document is ABOUT can be inferred — by design, not omission",
        "archietect cannot help: read the document yourself. Any conclusion about its contents would be Inferred-tier and must never be recorded as Derived or Declared",
    ),
];

fn tier_name(t: &Tier) -> String {
    format!("{t:?}")
}

/// The register. `idx`/`graph` are the same already-built index every other
/// query composes over — this function never scans.
pub fn register(idx: &Index, _graph: &StructuralGraph, root: &Path) -> Value {
    // `_graph` is accepted so every transport passes the same (idx, graph)
    // pair `status`/`doctor` take — the register composes over the code
    // index and the permission/coverage signals today; relationship-level
    // unknowns ("edges never observed") would need the graph and are the
    // obvious next entry kind. Deferred, not forgotten.
    let cfg = crate::permissions::default_global_config_path()
        .and_then(|p| crate::permissions::load(&p, root))
        .unwrap_or_default();
    let confirmations_path = crate::permissions::default_confirmations_path().ok();
    let perms = crate::permissions::report(&cfg);

    // Per-domain resolution, taken from permissions::report so the register
    // and `archietect permissions` can never disagree about a source label.
    // (domain → (allowed, source, structured))
    let resolved: Vec<(String, bool, String, bool)> = perms["domains"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|d| {
                    (
                        d["domain"].as_str().unwrap_or("").to_string(),
                        d["allowed"].as_bool().unwrap_or(false),
                        d["source"].as_str().unwrap_or("").to_string(),
                        d["structured"].as_bool().unwrap_or(true),
                    )
                })
                .collect()
        })
        .unwrap_or_default();

    let confirmed_for = |domain: &str| -> Option<bool> {
        confirmations_path
            .as_deref()
            .and_then(|p| crate::permissions::confirmation_state(p, domain))
    };

    // A domain is "looked at" if its config resolves to allowed, OR — for an
    // unstructured domain with no explicit config entry — a prior interactive
    // confirmation recorded "yes". Same rule `domain_allowed_with_confirmation`
    // applies at scan time, restated here without prompting.
    let effectively_enabled = |domain: &str, allowed: bool, source: &str, structured: bool| -> bool {
        if allowed {
            return true;
        }
        if !structured && !source.ends_with("-config") {
            return confirmed_for(domain) == Some(true);
        }
        false
    };

    let mut not_known: Vec<Value> = Vec::new();

    // ── unsupported_language ────────────────────────────────────────────────
    // The same live, extension-only walk coverage_report already does — the
    // one place a language nobody ever classified becomes visible at all.
    let unclassified = crate::scan::unclassified_files(root, &idx.excludes, 5000);
    if !unclassified.is_empty() {
        let mut languages: Vec<String> = unclassified.iter().map(|(_, ext)| format!(".{ext} (unclassified)")).collect();
        languages.sort();
        languages.dedup();
        let files: Vec<&str> = unclassified.iter().take(LIST_CAP).map(|(rel, _)| rel.as_str()).collect();
        not_known.push(json!({
            "kind": "unsupported_language",
            "languages": languages,
            "files": files,
            "file_count": unclassified.len(),
            "why": "no structural extractor for this language — a concept implemented only here gets INSUFFICIENT_COVERAGE, never a guessed ABSENT",
            "how_to_establish": "read the listed files directly; or add an extractor (`archietect proposal submit --kind extractor`)",
        }));
    }

    // ── per-domain: disabled / unconfirmed / tier_not_producible ────────────
    let mut domains_enabled: Vec<String> = Vec::new();
    for (domain, allowed, source, structured) in &resolved {
        if !IMPLEMENTED_DOMAINS.contains(&domain.as_str()) {
            continue;
        }
        let explicit = source.ends_with("-config");
        let confirmed = if *structured { None } else { confirmed_for(domain) };

        if effectively_enabled(domain, *allowed, source, *structured) {
            domains_enabled.push(domain.clone());
            for (d, tier, why, how) in NOT_PRODUCIBLE {
                if d == domain {
                    not_known.push(json!({
                        "kind": "tier_not_producible",
                        "domain": domain,
                        "tier": tier_name(tier),
                        "why": why,
                        "how_to_establish": how,
                    }));
                }
            }
            continue;
        }

        if !*structured && !explicit {
            match confirmed {
                None => not_known.push(json!({
                    "kind": "unstructured_domain_unconfirmed",
                    "domain": domain,
                    "configured": source,
                    "confirmed": Value::Null,
                    "why": "unstructured domains require a one-time interactive confirmation before anything is looked at; none has been recorded, so nothing in this domain has been observed",
                    "how_to_establish": format!("run `archietect {domain} scan --dir <path>` in a terminal and answer the prompt, or set [domains.{domain}] state = \"enabled\" explicitly in archietect.toml"),
                })),
                Some(false) => not_known.push(json!({
                    "kind": "domain_disabled",
                    "domain": domain,
                    "source": "confirmation-declined",
                    "why": "a prior interactive confirmation answered no; it is not re-asked, so nothing in this domain has been looked at",
                    "how_to_establish": format!("set [domains.{domain}] state = \"enabled\" explicitly in archietect.toml (an explicit config entry is honored without prompting), or remove the entry from ~/.archietect/confirmations.toml to be asked again"),
                })),
                Some(true) => unreachable!("Some(true) is handled by effectively_enabled above"),
            }
            continue;
        }

        not_known.push(json!({
            "kind": "domain_disabled",
            "domain": domain,
            "source": source,
            "why": "disabled by permission config — nothing in this domain has been looked at, so its absence from any answer is a boundary, not a finding",
            "how_to_establish": format!("enable it: [domains] {domain} = \"enabled\" in archietect.toml (this project) or ~/.archietect/system.toml (every project), then re-run"),
        }));
    }

    // ── usage_unobserved ────────────────────────────────────────────────────
    // Exactly `status`'s `declared_but_never_observed_in_use` filter, with the
    // count alongside the capped list instead of a silent take(25).
    let dead: Vec<&String> = idx
        .concepts
        .iter()
        .filter(|(_, c)| c.usage.is_empty() && c.declared_in.iter().any(|(_, k)| k != "prisma-enum"))
        .map(|(n, _)| n)
        .collect();
    if !dead.is_empty() {
        not_known.push(json!({
            "kind": "usage_unobserved",
            "concept_count": dead.len(),
            "concepts": dead.iter().take(LIST_CAP).collect::<Vec<_>>(),
            "why": "declared, but no observed access in the access styles v0 parses (raw drivers, GraphQL resolvers, services in other repos are invisible) — absence at USED tier only, not proof of death",
            "how_to_establish": "check for an access style archietect cannot see, or accept DECLARED_ONLY as the verdict",
        }));
    }

    // ── boundary ────────────────────────────────────────────────────────────
    let boundary_domains: Vec<Value> = resolved
        .iter()
        .map(|(domain, allowed, source, structured)| {
            json!({
                "domain": domain,
                "structured": structured,
                "allowed": allowed,
                "source": source,
                "confirmed": if *structured { Value::Null } else { json!(confirmed_for(domain)) },
            })
        })
        .collect();

    let newest_verification_ms = idx.concepts.values().map(|c| c.last_verified_ms).max();

    json!({
        "as_of": {
            "index": crate::query::index_freshness(root),
            "newest_verification_ms": newest_verification_ms,
        },
        "known": {
            "files_scanned": idx.files_scanned,
            "concepts_declared": idx.concepts.len(),
            "concepts_with_observed_usage": idx.concepts.values().filter(|c| !c.usage.is_empty()).count(),
            "domains_enabled": domains_enabled,
            "see": "archietect status — this register summarizes; it does not duplicate",
        },
        "not_known": not_known,
        "boundary": {
            "hardcoded_denials": perms["hardcoded_denials"].clone(),
            "domains": boundary_domains,
        },
        "note": "Read `not_known` before trusting any ABSENT: an entry here means a category of evidence was never looked at or cannot be produced, which is a boundary of this memory, not a fact about the world. Every entry names how to establish the fact without archietect.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(label: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("archietect-register-test-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(
            p.join("schema.prisma"),
            "model Widget {\n  id   Int    @id @default(autoincrement())\n  name String\n}\n",
        )
        .unwrap();
        std::fs::write(p.join("handler.lua"), "function processOrder() end\n").unwrap();
        p
    }

    fn kinds(out: &Value) -> Vec<(String, String)> {
        out["not_known"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (e["kind"].as_str().unwrap().to_string(), e["domain"].as_str().unwrap_or("").to_string()))
            .collect()
    }

    /// Test isolation: every test points HOME at its own tempdir so the real
    /// ~/.archietect/{system.toml,confirmations.toml} are never read.
    struct HomeGuard(Option<std::ffi::OsString>);
    impl HomeGuard {
        fn set(dir: &Path) -> Self {
            let prev = std::env::var_os("HOME");
            std::env::set_var("HOME", dir);
            HomeGuard(prev)
        }
    }
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    // HOME is process-global; the tests below serialize on this so they can't
    // race each other's HomeGuard.
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn reports_unsupported_language_usage_unobserved_and_docker_disabled() {
        let _g = HOME_LOCK.lock().unwrap();
        let root = fixture("basic");
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let _h = HomeGuard::set(&home);

        let (idx, graph) = crate::scan::scan(&root);
        let out = register(&idx, &graph, &root);
        let ks = kinds(&out);

        let lua = out["not_known"].as_array().unwrap().iter().find(|e| e["kind"] == "unsupported_language").expect("unsupported_language entry");
        assert!(lua["files"].as_array().unwrap().iter().any(|f| f.as_str().unwrap().ends_with("handler.lua")), "{lua}");
        assert_eq!(lua["file_count"], 1);

        let dead = out["not_known"].as_array().unwrap().iter().find(|e| e["kind"] == "usage_unobserved").expect("usage_unobserved entry");
        assert!(dead["concepts"].as_array().unwrap().iter().any(|c| c == "Widget"), "{dead}");
        assert_eq!(dead["concept_count"], 1);

        assert!(ks.contains(&("domain_disabled".into(), "docker".into())), "{ks:?}");
        // git is default-enabled → its unproducible tier is stated, not a disabled entry
        assert!(ks.contains(&("tier_not_producible".into(), "git".into())), "{ks:?}");
        assert!(!ks.contains(&("domain_disabled".into(), "git".into())), "{ks:?}");
        // documents: never asked, no config → unconfirmed, and boundary says confirmed: null
        assert!(ks.contains(&("unstructured_domain_unconfirmed".into(), "documents".into())), "{ks:?}");
        let docs = out["boundary"]["domains"].as_array().unwrap().iter().find(|d| d["domain"] == "documents").unwrap();
        assert!(docs["confirmed"].is_null());
        // code + git enabled, docker not
        assert_eq!(out["known"]["domains_enabled"], json!(["code", "git"]));
        // no docker tier entry while docker is disabled — that would be double-counting
        assert!(!ks.contains(&("tier_not_producible".into(), "docker".into())), "{ks:?}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn enabling_docker_swaps_disabled_entry_for_nothing_since_observed_is_now_producible() {
        // Was `..._swaps_disabled_entry_for_observed_tier_entry` — deliberate,
        // disclosed behavior change: `docker_domain::scan_observed`
        // (`archietect docker observe`) now makes the Observed tier
        // genuinely producible for an enabled docker domain, just not
        // automatically. A NOT_PRODUCIBLE row would be a stale, now-false
        // claim, so enabling docker must swap the disabled entry for
        // NOTHING, not for a replacement tier_not_producible row.
        let _g = HOME_LOCK.lock().unwrap();
        let root = fixture("docker-enabled");
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let _h = HomeGuard::set(&home);
        std::fs::write(root.join("archietect.toml"), "[domains]\ndocker = \"enabled\"\n").unwrap();

        let (idx, graph) = crate::scan::scan(&root);
        let out = register(&idx, &graph, &root);
        let ks = kinds(&out);

        assert!(!ks.contains(&("domain_disabled".into(), "docker".into())), "{ks:?}");
        assert!(
            !ks.contains(&("tier_not_producible".into(), "docker".into())),
            "docker's Observed tier is producible via scan_observed now — a tier_not_producible row would be stale, got: {ks:?}"
        );
        assert!(out["known"]["domains_enabled"].as_array().unwrap().iter().any(|d| d == "docker"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn confirmation_state_is_exposed_in_boundary_and_flips_documents_to_enabled() {
        let _g = HOME_LOCK.lock().unwrap();
        let root = fixture("confirmed");
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let _h = HomeGuard::set(&home);

        // Write through the REAL persist path, not a hand-rolled file.
        struct Yes;
        impl crate::permissions::ConfirmationAsker for Yes {
            fn confirm(&self, _: &str) -> bool { true }
        }
        let confirmations = crate::permissions::default_confirmations_path().unwrap();
        assert!(confirmations.starts_with(&home), "test must not touch the real confirmations file");
        let cfg = crate::permissions::PermissionConfig::default();
        assert!(crate::permissions::domain_allowed_with_confirmation(&cfg, &confirmations, "documents", &Yes).unwrap());

        let (idx, graph) = crate::scan::scan(&root);
        let out = register(&idx, &graph, &root);
        let docs = out["boundary"]["domains"].as_array().unwrap().iter().find(|d| d["domain"] == "documents").unwrap();
        assert_eq!(docs["confirmed"], json!(true));
        assert!(out["known"]["domains_enabled"].as_array().unwrap().iter().any(|d| d == "documents"));
        let ks = kinds(&out);
        assert!(!ks.contains(&("unstructured_domain_unconfirmed".into(), "documents".into())), "{ks:?}");
        // enabled → what it still can't produce is stated
        assert!(ks.contains(&("tier_not_producible".into(), "documents".into())), "{ks:?}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn omits_categories_with_no_instances() {
        let _g = HOME_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!("archietect-register-test-clean-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.ts"), "export class A {}\n").unwrap();
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let _h = HomeGuard::set(&home);

        let (idx, graph) = crate::scan::scan(&root);
        let out = register(&idx, &graph, &root);
        let ks = kinds(&out);
        assert!(!ks.iter().any(|(k, _)| k == "unsupported_language"), "no .lua here → no entry, got {ks:?}");
        assert!(!ks.iter().any(|(k, _)| k == "usage_unobserved"), "no schema concepts here → no entry, got {ks:?}");

        let _ = std::fs::remove_dir_all(&root);
    }
}
