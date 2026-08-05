//! The law registry — Architect's language specification, as data.
//!
//! Laws are not documentation and not implementation details: they are the
//! DEFINITION of what this engine is allowed to believe. Each one was
//! distilled from a wrong answer on a real repository, and each is enforced
//! forever by a regression test in `tests/laws.rs` with a self-contained
//! fixture. A law without its regression test is a wish; the test is what
//! makes "the engine got stricter" a fact instead of a claim.
//!
//! The registry is queryable (`architect laws`) so the questions the ledger
//! invites — which repository taught which law, which laws are newest, where
//! the engine's beliefs come from — are answerable by machines, not just by
//! reading markdown.

use serde_json::{json, Value};

pub struct Law {
    pub id: &'static str,
    pub title: &'static str,
    pub statement: &'static str,
    pub because: &'static str,
    pub discovered_in: &'static str,
    pub regression: &'static str,
}

pub const LAWS: &[Law] = &[
    Law {
        id: "law-001",
        title: "Word-boundary matching, never substring",
        statement: "A concept term matches a name only at snake_case/camelCase token boundaries with bounded inflection.",
        because: "'%sd%' matched USD and declared a broken pipeline healthy; 'story' claimed harvest_hiSTORY as a competing implementation, manufacturing a fake architectural problem.",
        discovered_in: "TITAN (diagnose), local sweep (story/history)",
        regression: "tests/laws.rs::law_001_word_boundary",
    },
    Law {
        id: "law-002",
        title: "The guard's re-declaration exemption is exact-name only",
        statement: "A proposed CREATE TABLE is exempt from blocking only when it exactly equals (case-insensitive) the canonical concept's name or its declared table.",
        because: "Fuzzy same_word exemption allowed CREATE TABLE ghosts while model Ghost was ACTIVE. Near-names are exactly what duplicates look like, so near-names must block.",
        discovered_in: "ghosttrack-monorepo",
        regression: "tests/laws.rs::law_002_exact_exemption",
    },
    Law {
        id: "law-003",
        title: "Prose about schema is not schema (comments)",
        statement: "Comment lines are stripped before CREATE TABLE extraction from non-.sql sources.",
        because: "The guard's own doc comment — 'a patch proposing CREATE TABLE episodes is REJECTED' — minted a phantom episodes concept that then satisfied the exemption and defeated the guard it documented.",
        discovered_in: "TITAN (architecture.rs doc comment)",
        regression: "tests/laws.rs::law_003_comment_prose",
    },
    Law {
        id: "law-004",
        title: "Exact-name match outranks token match",
        statement: "When ranking canonical candidates, a concept whose full name matches the queried term outranks token matches regardless of usage counts.",
        because: "umami: 'website' returned WebsiteEvent (more usage) while a model literally named Website sat in the schema. The thing named for the concept IS the concept; heavier neighbours are still neighbours.",
        discovered_in: "umami",
        regression: "tests/laws.rs::law_004_exact_over_token",
    },
    Law {
        id: "law-005",
        title: "Declarations sharing a table are one concept",
        statement: "An SQL-only declaration whose name equals another concept's declared table mapping folds into that concept instead of competing with it.",
        because: "umami declared `website` in SQL migrations and `Website` in Prisma mapped to the same table — one concept, reported as competing with itself.",
        discovered_in: "umami",
        regression: "tests/laws.rs::law_005_same_table_merge",
    },
    Law {
        id: "law-006",
        title: "table=True declares storage regardless of base names",
        statement: "A Python class with table=True in its bases is a storage declaration even when no base is literally named SQLModel or BaseModel.",
        because: "fastapi-template: class Item(ItemBase, table=True) names neither — the real storage models were invisible while their API contracts were indexed.",
        discovered_in: "full-stack-fastapi-template",
        regression: "tests/laws.rs::law_006_table_true",
    },
    Law {
        id: "law-007",
        title: "ORM declarations outrank SQL-string-only concepts on exact ties",
        statement: "When two exact-name candidates tie, the one with a non-SQL (ORM/schema) declaration wins.",
        because: "redash: a phantom lowercase `query` outranked the real Query model on usage inflated by every FROM query in the repo. A model class is a strong declaration; text that happens to say CREATE TABLE is the weakest.",
        discovered_in: "redash",
        regression: "tests/laws.rs::law_007_orm_over_sql",
    },
    Law {
        id: "law-008",
        title: "Prose about schema is not schema (strings)",
        statement: "A CREATE TABLE match must be followed by its column list `(` or AS — otherwise it is prose.",
        because: "redash: logger.debug(\"CREATE TABLE query: %s\") — a log message — minted the phantom that law-007 then had to out-rank. The follower requirement kills log/prose strings structurally, with no blacklist to maintain.",
        discovered_in: "redash",
        regression: "tests/laws.rs::law_008_follower_required",
    },
    Law {
        id: "law-009",
        title: "Declared ontology outranks name search; its absence is not ABSENT",
        statement: "architect.toml aliases resolve concepts no name search can see, at DECLARED tier; a stale alias pointing at nothing is reported as a finding, never silently ignored.",
        because: "TITAN: 'episode' has no table named for it, but the project itself declares episode = stories. Without the ontology the guard allowed CREATE TABLE episodes on the very repo whose internal architect rejects it.",
        discovered_in: "TITAN (standalone-vs-internal divergence)",
        regression: "tests/laws.rs::law_009_alias_resolution",
    },
];

pub fn registry_json() -> Value {
    json!({
        "laws": LAWS.iter().map(|l| json!({
            "id": l.id,
            "title": l.title,
            "statement": l.statement,
            "because": l.because,
            "discovered_in": l.discovered_in,
            "regression": l.regression,
        })).collect::<Vec<_>>(),
        "count": LAWS.len(),
        "extractor_version": crate::scan::EXTRACTOR_VERSION,
        "note": "Laws are the engine's language specification: each one was distilled from a wrong answer on a real repository, and each is enforced forever by the named regression test. A law without its test is a wish.",
    })
}
