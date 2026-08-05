//! The fact model — what the Architect is allowed to know.
//!
//! THE NON-NEGOTIABLE PRINCIPLE: the Architect never invents architectural
//! facts. Every answer carries its evidence, and every piece of evidence is
//! labelled with its strength:
//!
//!   DECLARED — read from a schema declaration (schema.prisma, models.py,
//!              CREATE TABLE). The project itself asserts this. Strongest.
//!   USED     — observed in source as a real access (prisma.model.*,
//!              Model.objects.*, INSERT INTO ...). The code demonstrably
//!              touches it.
//!   NAMED    — name resemblance only. The weakest tier, and it says so.
//!
//! When only NAMED evidence exists the verdict is UNKNOWN with "needs human
//! confirmation" — never a confident answer built on a filename. An engine
//! that answers confidently from weak evidence is worse than grep, because it
//! LOOKS like knowledge. (Lesson already paid for once: a substring match
//! reported a broken pipeline as healthy because '%sd%' matched 'USD'.)

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Tier {
    Declared,
    Used,
    Named,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub tier: Tier,
    /// Human-readable statement of the fact, always naming its source file.
    pub what: String,
}

/// One declared concept — a model/table the project itself asserts exists.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Concept {
    pub name: String,
    /// When this concept FIRST appeared in the index — survives rescans, so
    /// the engine can say "Story has existed here since 2026-08-01" rather
    /// than only "Story exists". Memory, not cache.
    #[serde(default)]
    pub first_seen_ms: i64,
    /// When the evidence for it was last re-verified against the tree.
    #[serde(default)]
    pub last_verified_ms: i64,
    /// (file, extractor-kind) pairs — where and how it is declared.
    pub declared_in: Vec<(String, String)>,
    pub fields: Vec<String>,
    /// Other concepts this one declares relations to (FK / relation fields).
    pub relations: Vec<String>,
    /// The storage-level table name, when the declaration states one.
    /// None when we would have to GUESS (e.g. Django app-label prefixes) —
    /// a wrong table name stated confidently is worse than none.
    pub table: Option<String>,
    /// (file, access-kind) pairs — observed real accesses. USED tier.
    pub usage: Vec<(String, String)>,
}

/// A declared architectural decision — the WHY behind a shape, with the roads
/// considered and rejected. Rationale is the one architectural fact that can
/// NEVER be extracted from code: code shows the road taken, and the rejected
/// road is exactly what the next person is about to propose.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub decision: String,
    pub because: String,
    #[serde(default)]
    pub rejected: Vec<String>,
    #[serde(default)]
    pub links: Vec<String>,
}

/// Everything the scan learned about one repository.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Index {
    pub root: String,
    pub files_scanned: usize,
    pub declaration_files: Vec<(String, String)>,
    /// BTreeMap for deterministic output — same repo, same answer, same order.
    pub concepts: BTreeMap<String, Concept>,
    /// architect.toml [aliases]: concept term → the concept that implements it.
    /// The project's own ontology; DECLARED-tier evidence.
    pub aliases: BTreeMap<String, String>,
    /// architect.toml [[decision]] entries.
    pub decisions: Vec<Decision>,
    /// Per-file extraction cache: the incremental engine. A file whose size,
    /// mtime and extractor version are unchanged contributes its stored facts
    /// without being re-read.
    #[serde(default)]
    pub file_facts: BTreeMap<String, FileFacts>,
    /// Signature of the concept set (names + tables). If a rescan changes it,
    /// ALL cached usage is invalid — usage matchers are built FROM the concept
    /// set, so a schema change invalidates downstream exactly like a compiler:
    /// schema → everything; a code file → only itself.
    #[serde(default)]
    pub concepts_sig: String,
    #[serde(default)]
    pub extractor_version: u32,
}

/// What one file contributed, cached against (size, mtime, extractor version).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileFacts {
    pub size: u64,
    pub mtime_ms: i64,
    /// Declaration fragments this file asserts (merged globally at assembly).
    pub decls: Vec<DeclFragment>,
    /// (concept, access-kind) usage hits observed in this file.
    pub usage: Vec<(String, String)>,
    /// Which declaration kinds this file contributes (for declaration_files).
    pub decl_kinds: Vec<String>,
}

/// One file's assertion about one concept — pure per-file output, so the
/// global graph can be reassembled from cache without re-reading the file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeclFragment {
    pub name: String,
    pub kind: String,
    pub fields: Vec<String>,
    pub relations: Vec<String>,
    pub table: Option<String>,
}

// ── word matching ────────────────────────────────────────────────────────────
// Prefix-sharing with bounded suffixes, so story/stories match and
// story/history do not. Substring matching is actively wrong here.

pub fn same_word(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    if a == b {
        return true;
    }
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let shared = ab.iter().zip(bb).take_while(|(x, y)| x == y).count();
    let shorter = ab.len().min(bb.len());
    shared >= 4
        && shared * 10 >= shorter * 7
        && ab.len() - shared <= 3
        && bb.len() - shared <= 3
}

/// Split snake_case AND camelCase into tokens ("UserAuditLog" → user, audit, log).
pub fn name_tokens(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() || c == '_' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_ascii_uppercase() && prev_lower {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        }
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn names_concept(name: &str, term: &str) -> bool {
    name_tokens(name).iter().any(|t| same_word(t, term))
}
