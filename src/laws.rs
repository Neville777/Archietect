//! The law registry — Architect's language specification, loaded from DATA.
//!
//! The laws live in `laws/*.toml`, one file per law, embedded into the binary
//! at compile time and parsed at startup. ONE source of truth: `architect
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
    pub regression: String,
    #[serde(default)]
    pub supersedes: Option<String>,
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
    json!({
        "laws": laws.iter().map(|l| json!({
            "id": l.id,
            "title": l.title,
            "statement": l.statement,
            "because": l.because,
            "discovered_in": l.discovered_in,
            "introduced": l.introduced,
            "status": l.status,
            "supersedes": l.supersedes,
            "regression": l.regression,
        })).collect::<Vec<_>>(),
        // The trust panel: countable facts only. No precision percentage is
        // reported because none has been MEASURED against ground truth — a
        // trust panel that includes an unmeasured number teaches readers to
        // distrust the measured ones.
        "stats": {
            "laws_total": laws.len(),
            "laws_active": active,
            "laws_superseded": laws.len() - active,
            "corpus_repositories": corpus.repo.len(),
            "extractor_version": crate::scan::EXTRACTOR_VERSION,
        },
        "corpus": corpus.repo.iter().map(|r| json!({
            "name": r.name, "kind": r.kind, "validated": r.validated,
            "contributed": r.contributed,
        })).collect::<Vec<_>>(),
        "note": "Laws are the engine's language specification, loaded from laws/*.toml (one source of truth for the CLI, the regression suite, and documentation). Each was distilled from a wrong answer on a real repository and is enforced forever by its named fixture. Laws are amended, not edited: superseded laws remain in the registry with their history.",
    })
}
