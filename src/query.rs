//! The queries — deterministic, evidence-carrying, refusing to bluff.
//!
//! Verdict vocabulary:
//!   ACTIVE        — declared AND observably used. Extend it.
//!   DECLARED_ONLY — the schema asserts it but nothing observed touches it.
//!                   May be scaffolding; confirm before extending or replacing.
//!   UNKNOWN       — only name resemblance. Needs human confirmation.
//!   ABSENT        — no declaration, no usage, no resemblance. Building is
//!                   justified.
//!
//! Canonical selection: the most USED declared concept, ties broken by
//! declared relations. Rows/usage alone never outrank declarations — a
//! declaration is the project speaking; usage is the project acting.

use crate::model::{names_concept, Evidence, Index, Tier};
use serde_json::{json, Value};
use walkdir::WalkDir;

pub fn concept(idx: &Index, term: &str) -> Value {
    let term = term.trim();
    let mut declared: Vec<&String> = idx
        .concepts
        .keys()
        .filter(|n| names_concept(n, term))
        .collect();
    declared.sort_by_key(|n| {
        let c = &idx.concepts[n.as_str()];
        std::cmp::Reverse((c.usage.len(), c.relations.len()))
    });

    if let Some(&canon) = declared.first() {
        let c = &idx.concepts[canon];
        let mut evidence: Vec<Evidence> = c
            .declared_in
            .iter()
            .map(|(f, k)| Evidence {
                tier: Tier::Declared,
                what: format!("{k} declaration in {f}"),
            })
            .collect();
        evidence.extend(c.usage.iter().take(8).map(|(f, k)| Evidence {
            tier: Tier::Used,
            what: format!("{k} access in {f}"),
        }));
        let used = !c.usage.is_empty();
        return json!({
            "concept": term,
            "verdict": if used { "ACTIVE" } else { "DECLARED_ONLY" },
            "canonical": canon,
            "table": c.table,
            "fields": c.fields.iter().take(15).collect::<Vec<_>>(),
            "relations": c.relations,
            "competing": declared.iter().skip(1).take(5).collect::<Vec<_>>(),
            "used_by_files": c.usage.iter().map(|(f, _)| f).take(10).collect::<Vec<_>>(),
            "evidence": evidence,
            "confidence": if used { "high" } else {
                "medium — declared but no observed access; may be scaffolding"
            },
            "recommendation": if used {
                format!("'{term}' already exists as '{canon}'. Extend it; do not create a second implementation.")
            } else {
                format!("'{term}' is declared as '{canon}' but nothing observably uses it. Confirm whether it is scaffolding before extending OR replacing.")
            },
        });
    }

    // NAMED tier: filename resemblance only — and the answer says so.
    let named: Vec<String> = WalkDir::new(&idx.root)
        .into_iter()
        .filter_entry(|e| {
            !(e.file_type().is_dir()
                && matches!(
                    e.file_name().to_str(),
                    Some("node_modules" | ".git" | "target" | "dist" | ".next" | "__pycache__")
                ))
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let stem = e.path().file_stem()?.to_str()?.to_string();
            if names_concept(&stem, term) {
                Some(
                    e.path()
                        .strip_prefix(&idx.root)
                        .unwrap_or(e.path())
                        .display()
                        .to_string(),
                )
            } else {
                None
            }
        })
        .take(10)
        .collect();

    if !named.is_empty() {
        return json!({
            "concept": term,
            "verdict": "UNKNOWN",
            "canonical": null,
            "evidence": named.iter().map(|f| Evidence {
                tier: Tier::Named,
                what: format!("filename resemblance only: {f}"),
            }).collect::<Vec<_>>(),
            "confidence": "low — name resemblance is not an architectural fact",
            "recommendation": "Needs human confirmation. Similarly named files exist but no schema declares this concept — inspect them before building anything.",
        });
    }
    json!({
        "concept": term,
        "verdict": "ABSENT",
        "canonical": null,
        "evidence": [],
        "confidence": "high — no declaration, no observed usage, no name resemblance",
        "recommendation": "Genuinely new for this project. Building it is justified.",
    })
}

const STOP: &[&str] = &[
    "want", "need", "make", "build", "better", "improve", "with", "that", "this", "have", "from",
    "into", "would", "should", "could", "will", "more", "less", "system", "support", "please",
    "about", "when", "then", "them", "track", "tracking", "show", "view", "page", "data", "info",
    "some", "every", "feature", "the", "and", "for", "add", "create", "user", "users",
];

pub fn intent(idx: &Index, text: &str) -> Value {
    let mut terms = Vec::new();
    for w in text
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() >= 4 && !STOP.contains(w))
    {
        if !terms.contains(&w.to_string()) {
            terms.push(w.to_string());
        }
    }
    terms.truncate(8);

    let mut extend = Vec::new();
    let mut create = Vec::new();
    let mut needs_confirmation = Vec::new();
    for t in &terms {
        let r = concept(idx, t);
        match r["verdict"].as_str().unwrap_or("") {
            "ACTIVE" | "DECLARED_ONLY" => extend.push(json!({
                "concept": t,
                "canonical": r["canonical"],
                "verdict": r["verdict"],
                "relations": r["relations"],
            })),
            "UNKNOWN" => needs_confirmation.push(json!({
                "concept": t,
                "note": "name-resemblance only — confirm by hand",
            })),
            _ => create.push(t.clone()),
        }
    }
    let summary = if !extend.is_empty() {
        let names: Vec<String> = extend
            .iter()
            .filter_map(|e| e["canonical"].as_str().map(String::from))
            .collect();
        format!(
            "{} concept(s) already exist: extend {}. {}",
            extend.len(),
            names.join(", "),
            if create.is_empty() {
                "Nothing genuinely new required.".to_string()
            } else {
                format!("Only genuinely new: {}.", create.join(", "))
            }
        )
    } else if !create.is_empty() && needs_confirmation.is_empty() {
        "No named concept exists here — this intent is greenfield for this project.".to_string()
    } else {
        "Nothing matched declarations. Either the vocabulary differs from the project's or this is new territory.".to_string()
    };
    json!({
        "intent": text,
        "recognised": terms,
        "extend": extend,
        "create": create,
        "needs_confirmation": needs_confirmation,
        "smallest_correct_change": summary,
    })
}

pub fn impact(idx: &Index, term: &str) -> Value {
    let r = concept(idx, term);
    let Some(canon) = r["canonical"].as_str().map(String::from) else {
        return json!({
            "target": term,
            "impact": "unknown — concept not declared in this project",
            "detail": r,
        });
    };
    let c = &idx.concepts[&canon];
    let mut files: Vec<&String> = c.usage.iter().map(|(f, _)| f).collect();
    files.sort();
    files.dedup();
    let dependents: Vec<&String> = idx
        .concepts
        .iter()
        .filter(|(_, other)| other.relations.iter().any(|r| r == &canon))
        .map(|(n, _)| n)
        .collect();
    json!({
        "target": canon,
        "severity": if files.len() > 8 || dependents.len() > 3 {
            "HIGH — widely used and other models declare relations to it"
        } else if !files.is_empty() || !dependents.is_empty() {
            "MODERATE — several touchpoints"
        } else {
            "NONE OBSERVED — declared but nothing seen touching it"
        },
        "used_by_files": files.iter().take(20).collect::<Vec<_>>(),
        "declared_dependents": dependents,
        "evidence_note": "used_by = observed ORM/SQL access (USED tier); dependents = schema-declared relations (DECLARED tier).",
    })
}

pub fn status(idx: &Index) -> Value {
    let used = idx.concepts.values().filter(|c| !c.usage.is_empty()).count();
    let dead: Vec<&String> = idx
        .concepts
        .iter()
        .filter(|(_, c)| {
            c.usage.is_empty() && c.declared_in.iter().any(|(_, k)| k != "prisma-enum")
        })
        .map(|(n, _)| n)
        .take(25)
        .collect();
    json!({
        "root": idx.root,
        "files_scanned": idx.files_scanned,
        "declaration_files": idx.declaration_files,
        "concepts_declared": idx.concepts.len(),
        "concepts_with_observed_usage": used,
        "declared_but_never_observed_in_use": dead,
        "note": "'never observed in use' is evidence of absence at USED tier only — access styles v0 doesn't parse (raw drivers, GraphQL resolvers, services in other repos) are invisible. Stated so it cannot be mistaken for proof of death.",
    })
}
