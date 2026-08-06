//! Invariant regression suite — runs the architectural invariants against
//! every validation corpus repo.
//!
//! ## Why this is separate from laws.rs
//!
//! laws.rs tests PER-LAW fixtures: tiny synthetic repos that reproduce one
//! specific past wrong answer. Those are counterexample records.
//!
//! This file tests CROSS-CUTTING PROPERTIES against real repos: does any
//! concept in the full chatwoot scan have two storage identities? Does any
//! alias in lobe-chat point at nothing? Those questions can't be asked of a
//! six-line fixture — they need the full corpus.
//!
//! Together the two suites give different guarantees:
//!   laws.rs        — "the specific bug that taught us this rule cannot recur"
//!   invariants.rs  — "the class of bug this invariant defines cannot occur
//!                     in any scanned corpus, not just the one we've seen"

use architect::{invariants, scan};
use std::path::PathBuf;

fn corpus_root(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("validation").join(name)
}

/// Run invariant checks on a pre-built corpus repo.
/// Uses scan_with_prior so the pre-built architect.db is the source of truth
/// (same as a real `architect concept` call on that repo).
fn check_corpus(name: &str) -> Vec<invariants::Violation> {
    let root = corpus_root(name);
    assert!(root.exists(), "corpus repo missing: {}", root.display());
    // Load from the pre-built db — no need to rescan the whole repo in CI.
    let prior = architect::store::load_raw(&root);
    let idx = scan::scan_with_prior(&root, prior);
    invariants::check(&idx)
}

// ── corpus repos ─────────────────────────────────────────────────────────────

#[test]
fn invariants_chatwoot() {
    let v = check_corpus("chatwoot");
    assert!(
        v.is_empty(),
        "chatwoot violated {} invariant(s):\n{}",
        v.len(),
        fmt(&v)
    );
}

#[test]
fn invariants_lobe_chat() {
    let v = check_corpus("lobe-chat");
    assert!(
        v.is_empty(),
        "lobe-chat violated {} invariant(s):\n{}",
        v.len(),
        fmt(&v)
    );
}

#[test]
fn invariants_umami() {
    let v = check_corpus("umami");
    assert!(
        v.is_empty(),
        "umami violated {} invariant(s):\n{}",
        v.len(),
        fmt(&v)
    );
}

#[test]
fn invariants_saleor() {
    let v = check_corpus("saleor");
    assert!(
        v.is_empty(),
        "saleor violated {} invariant(s):\n{}",
        v.len(),
        fmt(&v)
    );
}

#[test]
fn invariants_bookstack() {
    let v = check_corpus("BookStack");
    assert!(
        v.is_empty(),
        "BookStack violated {} invariant(s):\n{}",
        v.len(),
        fmt(&v)
    );
}

#[test]
fn invariants_dub() {
    let v = check_corpus("dub");
    assert!(
        v.is_empty(),
        "dub violated {} invariant(s):\n{}",
        v.len(),
        fmt(&v)
    );
}

#[test]
fn invariants_analytics() {
    let v = check_corpus("analytics");
    assert!(
        v.is_empty(),
        "analytics violated {} invariant(s):\n{}",
        v.len(),
        fmt(&v)
    );
}

#[test]
fn invariants_redash() {
    let v = check_corpus("redash");
    assert!(
        v.is_empty(),
        "redash violated {} invariant(s):\n{}",
        v.len(),
        fmt(&v)
    );
}

// ── helper ───────────────────────────────────────────────────────────────────

fn fmt(violations: &[invariants::Violation]) -> String {
    violations
        .iter()
        .map(|v| format!("  [{}] {}", v.invariant, v.detail))
        .collect::<Vec<_>>()
        .join("\n")
}
