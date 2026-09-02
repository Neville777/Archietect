//! The general shape every domain-specific fact reduces to.
//!
//! See SYSTEM_MEMORY.md ("Core abstraction: Resource") for the design this
//! implements a slice of. Phase 1 only: prove the code domain — today
//! represented by two separate types, `model::Concept` (schema layer) and
//! `structural::Symbol` (structural layer) — is one instance of a single
//! general shape, with ZERO change to what the CLI/REST/MCP surface emits.
//! `domain` is hardcoded to "code" everywhere in this phase; a second value
//! only becomes meaningful once a non-code extractor exists (rollout step 3
//! in SYSTEM_MEMORY.md), which this phase deliberately does not attempt.
//!
//! `Identity` is intentionally just a name wrapper right now — real identity
//! resolution (when are two resources actually the same entity) is called
//! out in SYSTEM_MEMORY.md as its own hard problem, not something to
//! improvise here as a side effect of a type-shape refactor.

use crate::model::Evidence;
use std::collections::BTreeMap;

/// A resource's name, standing in for real cross-domain identity for now.
/// See the module doc: this is deliberately not doing identity resolution.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Identity(pub String);

/// Where a resource lives. `line` is `None` for anything that isn't a single
/// source-line declaration (e.g. a schema concept declared across several
/// files — see `Concept::to_resource`, which uses the first declaration site
/// as the primary location and keeps the rest as evidence, exactly like the
/// existing JSON output already does).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub file: String,
    pub line: Option<usize>,
}

/// One fact, in the general shape described in SYSTEM_MEMORY.md. Not wired
/// into storage (store.rs) or extraction (scan.rs) in this phase — those
/// stay exactly as they are. This is the shape `Concept` and `Symbol`
/// project themselves onto at query time, proving the code domain fits the
/// general model without migrating how it's scanned or persisted yet.
#[derive(Debug, Clone)]
pub struct Resource {
    pub id: Identity,
    pub kind: String,
    pub domain: String,
    pub location: Location,
    pub attributes: BTreeMap<String, String>,
    pub evidence: Vec<Evidence>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Concept, Tier};
    use crate::structural::{Symbol, SymbolKind};

    #[test]
    fn concept_to_resource_is_domain_code() {
        let c = Concept {
            name: "Widget".into(),
            declared_in: vec![("schema.prisma".into(), "prisma".into())],
            usage: vec![("src/orders.ts".into(), "prisma.widget.*".into())],
            table: Some("widgets".into()),
            ..Default::default()
        };
        let r = c.to_resource("Widget");
        assert_eq!(r.domain, "code");
        assert_eq!(r.id.0, "Widget");
        assert_eq!(r.location.file, "schema.prisma");
        assert!(r.evidence.iter().any(|e| e.tier == Tier::Declared));
        assert!(r.evidence.iter().any(|e| e.tier == Tier::Used));
    }

    #[test]
    fn symbol_to_resource_is_domain_code() {
        let s = Symbol {
            name: "doctor".into(),
            kind: SymbolKind::Function,
            file: "src/query.rs".into(),
            linked_concept: None,
            line: 677,
        };
        let r = s.to_resource();
        assert_eq!(r.domain, "code");
        assert_eq!(r.id.0, "doctor");
        assert_eq!(r.location.file, "src/query.rs");
        assert_eq!(r.location.line, Some(677));
        assert_eq!(r.evidence.len(), 1);
        assert_eq!(r.evidence[0].tier, Tier::Declared);
        assert_eq!(r.evidence[0].what, "Function declared in src/query.rs:677");
    }
}
