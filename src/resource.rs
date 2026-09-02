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

/// One relationship between two resources — with its OWN evidence, never
/// implied merely by both endpoints existing. See SYSTEM_MEMORY.md
/// ("Relationships need their own evidence, not borrowed evidence").
///
/// Phase 2, code-only, one instance: `structural::Route::relationship_to`
/// turns what `routes_for_concept` already computed as a disposable inline
/// filter (handler name matches? path contains the name?) into an explicit,
/// evidenced edge — a route "handling" a concept is a real fact distinct
/// from the route existing and the concept existing.
#[derive(Debug, Clone)]
pub struct Relationship {
    pub from: Identity,
    pub kind: String,
    pub to: Identity,
    pub evidence: Evidence,
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn route_relationship_used_tier_on_handler_name_match() {
        use crate::structural::Route;
        let r = Route {
            method: "GET".into(),
            path: "/widgets".into(),
            handler: "Widget".into(),
            file: "src/routes.ts".into(),
        };
        let rel = r.relationship_to("Widget").expect("expected a relationship");
        assert_eq!(rel.from.0, "GET /widgets");
        assert_eq!(rel.kind, "handles");
        assert_eq!(rel.to.0, "Widget");
        assert_eq!(rel.evidence.tier, Tier::Used);
    }

    #[test]
    fn route_relationship_named_tier_on_path_substring_only() {
        use crate::structural::Route;
        let r = Route {
            method: "GET".into(),
            path: "/widgets/list".into(),
            handler: "unknown".into(),
            file: "src/routes.ts".into(),
        };
        let rel = r.relationship_to("widgets").expect("expected a relationship");
        assert_eq!(rel.evidence.tier, Tier::Named);
    }

    #[test]
    fn route_relationship_none_when_unrelated() {
        use crate::structural::Route;
        let r = Route {
            method: "GET".into(),
            path: "/orders".into(),
            handler: "OrderHandler".into(),
            file: "src/routes.ts".into(),
        };
        assert!(r.relationship_to("Widget").is_none());
    }
}
