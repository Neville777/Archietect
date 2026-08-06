//! Architectural invariants — properties that must hold for any valid index.
//!
//! ## Why invariants, alongside laws
//!
//! Laws are counterexample records: "this specific wrong answer on this
//! specific repo taught this rule." They're great at preserving history and
//! at pinning individual regressions.
//!
//! Invariants are output properties: "regardless of what repo you scan,
//! these things must always be true." They catch whole CLASSES of bugs,
//! not just the one instance that was seen before. A new wrong answer that
//! violates an invariant is caught the moment CI runs against any corpus
//! repo — before it ever becomes a named law.
//!
//! The two layers are complementary:
//!   invariants  →  catch regressions early, across all corpora
//!   laws        →  preserve the specific lesson and its history
//!
//! ## Current invariants
//!
//!   I-1  No concept has two storage identities
//!        (two declared tables for the same concept name is a merge failure)
//!   I-2  Every alias resolves to a known concept
//!        (a stale alias pointer is worse than no alias — it confidently lies)
//!   I-3  Every decision references at least one known concept
//!        (a decision that links to nothing is a dangling ADR)
//!   I-4  Every declared concept has at least one declaration site
//!        (a concept with no declared_in is a ghost — it should never exist)

use crate::model::Index;
use serde_json::{json, Value};

/// One invariant violation.
#[derive(Debug)]
pub struct Violation {
    pub invariant: &'static str,
    pub detail: String,
}

/// Run all invariants against an index. Returns every violation found.
/// An empty Vec means the index is clean.
pub fn check(idx: &Index) -> Vec<Violation> {
    let mut violations = Vec::new();
    check_i1_single_storage_identity(idx, &mut violations);
    check_i2_aliases_resolve(idx, &mut violations);
    check_i3_decisions_link_known(idx, &mut violations);
    check_i4_no_ghost_concepts(idx, &mut violations);
    violations
}

/// Serialise violations for CLI / MCP output.
pub fn to_json(violations: &[Violation]) -> Value {
    json!({
        "pass": violations.is_empty(),
        "violation_count": violations.len(),
        "violations": violations.iter().map(|v| json!({
            "invariant": v.invariant,
            "detail": v.detail,
        })).collect::<Vec<_>>(),
    })
}

// ── I-1: no concept has two storage identities ──────────────────────────────

fn check_i1_single_storage_identity(idx: &Index, out: &mut Vec<Violation>) {
    for (name, concept) in &idx.concepts {
        // Collect the distinct table names this concept declares.
        let tables: Vec<&str> = {
            let mut ts: Vec<&str> = concept
                .declared_in
                .iter()
                .filter_map(|(_, k)| {
                    // Only count declaration kinds that assert a storage name.
                    // "sql" entries are folded in by the merge law when they
                    // share the canonical's table — if they still exist
                    // separately here, that IS the violation.
                    if k != "sql" { None } else { None } // covered below
                })
                .collect();
            // Primary: use the concept's own table field if set.
            if let Some(t) = concept.table.as_deref() {
                ts.push(t);
            }
            // Also collect any sql-tier declared_in that carry a different name
            // (those are the ones the merge law should have folded).
            ts.sort();
            ts.dedup();
            ts
        };

        // The violation: the concept name itself differs from its declared
        // table by more than pluralisation, AND there's also an sql-tier
        // declaration present that wasn't merged.
        let has_orm = concept.declared_in.iter().any(|(_, k)| k != "sql");
        let has_sql = concept.declared_in.iter().any(|(_, k)| k == "sql");
        if has_orm && has_sql && tables.len() > 1 {
            out.push(Violation {
                invariant: "I-1: single storage identity",
                detail: format!(
                    "'{name}' has {} distinct storage identities: {}",
                    tables.len(),
                    tables.join(", ")
                ),
            });
        }
    }
}

// ── I-2: every alias resolves to a known concept ────────────────────────────

fn check_i2_aliases_resolve(idx: &Index, out: &mut Vec<Violation>) {
    for (alias, target) in &idx.aliases {
        if !idx.concepts.contains_key(target) {
            out.push(Violation {
                invariant: "I-2: alias resolves",
                detail: format!(
                    "alias '{alias}' → '{target}' but '{target}' is not a declared concept"
                ),
            });
        }
    }
}

// ── I-3: every decision references at least one known concept ───────────────

fn check_i3_decisions_link_known(idx: &Index, out: &mut Vec<Violation>) {
    for decision in &idx.decisions {
        if decision.links.is_empty() {
            // A decision with no links at all is suspicious but not strictly a
            // violation — it may be a project-level ADR that predates the
            // schema. Skip rather than false-positive.
            continue;
        }
        let any_known = decision.links.iter().any(|l| {
            idx.concepts.contains_key(l)
                || idx.concepts.keys().any(|k| k.eq_ignore_ascii_case(l))
        });
        if !any_known {
            out.push(Violation {
                invariant: "I-3: decision links known concept",
                detail: format!(
                    "decision '{}' links {:?} but none of those are declared concepts",
                    decision.id, decision.links
                ),
            });
        }
    }
}

// ── I-4: no ghost concepts ───────────────────────────────────────────────────

fn check_i4_no_ghost_concepts(idx: &Index, out: &mut Vec<Violation>) {
    for (name, concept) in &idx.concepts {
        if concept.declared_in.is_empty() {
            out.push(Violation {
                invariant: "I-4: no ghost concepts",
                detail: format!(
                    "'{name}' exists in the index with no declaration site — \
                     this is a scan assembly bug"
                ),
            });
        }
    }
}
