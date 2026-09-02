//! The law registry — Archietect's language specification, loaded from DATA.
//!
//! The laws live in `laws/*.toml`, one file per law, embedded into the binary
//! at compile time and parsed at startup. ONE source of truth: `archietect
//! laws` reads it, the regression suite cross-checks against it (a law
//! without a covering test fails the conformance test BY NAME), and any
//! documentation is generated from it. Editing a law is editing a data file,
//! reviewable in a diff like any other declaration in this system.
//!
//! Laws are AMENDED, not edited away: a wrong law gets `status =
//! "superseded"` and its successor points back via `supersedes` — this
//! versions architectural semantics, not just code. Every law carries the
//! repository that taught it and the fixture that enforces it forever; the
//! corpus taught the laws, the fixtures preserve the lessons.

use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Deserialize)]
pub struct Law {
    pub id: String,
    pub title: String,
    pub statement: String,
    pub because: String,
    pub discovered_in: Vec<String>,
    pub introduced: String,
    pub status: String,
    /// What kind of rule this is. Determines where it naturally lives
    /// and how it should evolve:
    ///   "parser"      — scanner/extractor correctness; belongs in scan.rs
    ///   "ranking"     — canonical selection weight; belongs in scoring.rs
    ///   "constraint"  — output property; expressed as invariant in invariants.rs
    ///   "guard"       — guard exemption logic; belongs in query::guard
    ///   "philosophy"  — irreducible architectural principle; stays here forever
    pub category: String,
    pub regression: String,
    #[serde(default)]
    pub supersedes: Option<String>,
    /// The CURRENT enforcement approach — deliberately separate from
    /// `statement`. A law is a timeless claim about what Archietect is
    /// allowed to assert ("must not return a confident ABSENT when
    /// coverage is insufficient"); `mechanism` is how today's code happens
    /// to enforce that claim, and is expected to change — a rewrite, a
    /// faster algorithm, a real parser someday — without the law itself
    /// changing at all. Conflating the two (a `statement` that names a
    /// specific function) makes the law read as obsolete the moment that
    /// function gets refactored, even though the underlying claim never
    /// stopped being true.
    #[serde(default)]
    pub mechanism: Option<String>,
}

/// The law files, embedded at compile time. Adding a law = adding its .toml
/// AND its line here — the conformance test in tests/laws.rs then demands a
/// fixture for it, so a law cannot exist unenforced.
const LAW_FILES: &[&str] = &[
    include_str!("../laws/law-001.toml"),
    include_str!("../laws/law-002.toml"),
    include_str!("../laws/law-003.toml"),
    include_str!("../laws/law-004.toml"),
    include_str!("../laws/law-005.toml"),
    include_str!("../laws/law-006.toml"),
    include_str!("../laws/law-007.toml"),
    include_str!("../laws/law-008.toml"),
    include_str!("../laws/law-009.toml"),
    include_str!("../laws/law-010.toml"),
    include_str!("../laws/law-011.toml"),
    include_str!("../laws/law-012.toml"),
    include_str!("../laws/law-013.toml"),
    include_str!("../laws/law-014.toml"),
    include_str!("../laws/law-015.toml"),
];

const CORPUS: &str = include_str!("../laws/corpus.toml");

pub fn all() -> Vec<Law> {
    LAW_FILES
        .iter()
        .filter_map(|t| toml::from_str::<Law>(t).ok())
        .collect()
}

#[derive(Debug, Deserialize)]
struct CorpusFile {
    #[serde(default)]
    repo: Vec<CorpusRepo>,
}
#[derive(Debug, Deserialize)]
struct CorpusRepo {
    name: String,
    kind: String,
    validated: String,
    contributed: Vec<String>,
}

pub fn registry_json() -> Value {
    let laws = all();
    let corpus: CorpusFile = toml::from_str(CORPUS).unwrap_or(CorpusFile { repo: vec![] });
    let active = laws.iter().filter(|l| l.status == "active").count();

    // Category breakdown — makes the "laws are doing too many jobs" problem
    // visible at a glance. Philosophy laws are the irreducible ones; the rest
    // have a natural home in their respective implementation layer.
    let mut by_category: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for l in laws.iter().filter(|l| l.status == "active") {
        *by_category.entry(l.category.as_str()).or_insert(0) += 1;
    }

    json!({
        "laws": laws.iter().map(|l| json!({
            "id": l.id,
            "title": l.title,
            "statement": l.statement,
            "mechanism": l.mechanism,
            "because": l.because,
            "discovered_in": l.discovered_in,
            "introduced": l.introduced,
            "status": l.status,
            "category": l.category,
            "supersedes": l.supersedes,
            "regression": l.regression,
        })).collect::<Vec<_>>(),
        "stats": {
            "laws_total": laws.len(),
            "laws_active": active,
            "laws_superseded": laws.len() - active,
            "by_category": by_category,
            "corpus_repositories": corpus.repo.len(),
            "extractor_version": crate::scan::EXTRACTOR_VERSION,
        },
        "corpus": corpus.repo.iter().map(|r| json!({
            "name": r.name, "kind": r.kind, "validated": r.validated,
            "contributed": r.contributed,
        })).collect::<Vec<_>>(),
        "note": "Laws are the engine's language specification, loaded from laws/*.toml. Each category indicates where the rule naturally lives: 'philosophy' laws are irreducible and stay here permanently; 'parser'/'ranking'/'constraint'/'guard' laws have a home in their implementation layer and their fixture is the regression record. Laws are amended, not edited.",
    })
}
