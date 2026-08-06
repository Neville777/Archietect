//! The law regression suite — the enforcement arm of laws/*.toml.
//!
//! Compiler-conformance model: per-law FIXTURE DIRECTORIES on disk
//! (tests/fixtures/law_NNN/ — tiny repositories, the distilled minimal
//! reproduction of each original wrong answer), ONE harness. Each law fails
//! independently by test name. Deliberately not one .rs file per law: every
//! file in tests/ compiles as its own crate, so 200 laws would mean 200
//! crate compilations per run — rustc's own suite solved this with fixture
//! programs plus a shared harness, and so does this one.
//!
//! `conformance_registry_matches_suite` binds the two sources: every ACTIVE
//! law in the registry must be covered here, so a law cannot exist
//! unenforced — a law without its test is a wish, and the wish now fails CI.

use architect::{laws, query, scan};
use std::path::PathBuf;

/// Laws covered by this harness. The conformance test cross-checks this
/// against the registry — adding a law without extending the suite fails.
const COVERED: &[&str] = &[
    "law-001", "law-002", "law-003", "law-004", "law-005",
    "law-006", "law-007", "law-008", "law-009", "law-010", "law-011",
];

fn fixture(law: &str) -> architect::model::Index {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(law);
    assert!(root.exists(), "fixture directory missing: {}", root.display());
    scan::scan_with_prior(&root, None)
}

#[test]
fn conformance_registry_matches_suite() {
    let registry = laws::all();
    assert!(!registry.is_empty(), "law registry failed to parse");
    for law in registry.iter().filter(|l| l.status == "active") {
        assert!(
            COVERED.contains(&law.id.as_str()),
            "{} ('{}') is ACTIVE in laws/ but has no covering test — a law without its regression test is a wish",
            law.id,
            law.title
        );
    }
    // and the reverse: no test claims a law the registry doesn't know
    for id in COVERED {
        assert!(
            registry.iter().any(|l| l.id == *id),
            "suite covers {id} but the registry has no such law"
        );
    }
}

#[test]
fn law_001_word_boundary() {
    let idx = fixture("law_001");
    let r = query::concept(&idx, "story");
    assert_eq!(r["canonical"], "story");
    let competing: Vec<String> = r["competing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(
        !competing.iter().any(|c| c.contains("history")),
        "substring matching resurfaced: {competing:?}"
    );
}

#[test]
fn law_002_exact_exemption() {
    let idx = fixture("law_002");
    let blocked = query::guard(&idx, "CREATE TABLE ghosts (id SERIAL);");
    assert_eq!(blocked["allowed"], false, "near-name must block");
    let allowed = query::guard(&idx, "CREATE TABLE \"Ghost\" (id TEXT);");
    assert_eq!(allowed["allowed"], true, "exact re-declaration is exempt");
}

#[test]
fn law_003_comment_prose() {
    let idx = fixture("law_003");
    assert!(
        !idx.concepts.contains_key("episodes") && !idx.concepts.contains_key("phantoms"),
        "prose in comments minted a concept: {:?}",
        idx.concepts.keys().collect::<Vec<_>>()
    );
}

#[test]
fn law_004_exact_over_token() {
    let idx = fixture("law_004");
    let r = query::concept(&idx, "website");
    assert_eq!(
        r["canonical"], "Website",
        "usage-heavy token match outranked the exact name"
    );
}

#[test]
fn law_005_same_table_merge() {
    let idx = fixture("law_005");
    assert!(
        idx.concepts.contains_key("Website") && !idx.concepts.contains_key("website"),
        "same-table declarations failed to merge: {:?}",
        idx.concepts.keys().collect::<Vec<_>>()
    );
    let c = &idx.concepts["Website"];
    assert!(c.declared_in.iter().any(|(_, k)| k == "prisma"));
    assert!(c.declared_in.iter().any(|(_, k)| k == "sql"));
}

#[test]
fn law_006_table_true() {
    let idx = fixture("law_006");
    let r = query::concept(&idx, "item");
    assert_eq!(r["canonical"], "Item");
    assert_eq!(r["table"], "item", "SQLModel default table name rule");
}

#[test]
fn law_007_orm_over_sql() {
    let idx = fixture("law_007");
    let r = query::concept(&idx, "query");
    assert_eq!(
        r["canonical"], "Query",
        "sql-string concept outranked the ORM declaration"
    );
}

#[test]
fn law_008_follower_required() {
    let idx = fixture("law_008");
    assert!(
        !idx.concepts.contains_key("query"),
        "log-string prose minted a concept"
    );
    assert!(idx.concepts.contains_key("results"), "real DDL must still extract");
}

#[test]
fn law_009_alias_resolution() {
    let idx = fixture("law_009");
    let r = query::concept(&idx, "episode");
    assert_eq!(r["canonical"], "stories", "alias resolution failed");
    assert_eq!(r["resolved_via"], "alias");
    let g = query::guard(&idx, "CREATE TABLE episodes (id BIGSERIAL);");
    assert_eq!(g["allowed"], false, "guard must block through the ontology");
    assert!(
        g["reason"].as_str().unwrap().contains("stories-own-episodes"),
        "rejection must cite the governing decision, got: {}",
        g["reason"]
    );
}

#[test]
fn law_010_alias_exact_target() {
    let idx = fixture("law_010");
    let r = query::concept(&idx, "theory");
    assert_eq!(
        r["canonical"], "causal_hypotheses",
        "multi-token alias target must resolve by exact name, got: {}",
        r["verdict"]
    );
    assert_eq!(r["resolved_via"], "alias");
}

#[test]
fn law_011_ontology_before_name_search() {
    // Reproduces the exact TITAN collision: architect.toml declares
    // theory = "causal_hypotheses", while an UNRELATED public struct
    // (GameTheoryEngine) independently token-matches "theory". The ontology
    // must win — an unrelated concept sharing one token with an alias key
    // must never silently defeat the declared alias.
    let idx = fixture("law_011");
    assert!(
        idx.concepts.contains_key("GameTheoryEngine"),
        "fixture must actually produce the colliding concept, or this test proves nothing"
    );
    let r = query::concept(&idx, "theory");
    assert_eq!(
        r["canonical"], "causal_hypotheses",
        "declared ontology was defeated by an unrelated name-token match, got: {}",
        r["canonical"]
    );
    assert_eq!(r["resolved_via"], "alias");
}
