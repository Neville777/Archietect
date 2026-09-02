//! Canonical ranking — the precedence lattice that decides which declared
//! concept is the right answer when multiple candidates match a query term.
//!
//! ## The lattice
//!
//! Every candidate sits in exactly one tier of this partial order. Higher
//! tier wins unconditionally over lower tier; within a tier, usage and
//! relation counts break ties.
//!
//! ```text
//! Tier 1 — DeclaredOntology    archietect.toml alias — the project spoke directly
//! Tier 2 — ExactOrm            exact name + ORM/framework declaration
//! Tier 3 — ExactSql            exact name + SQL-only declaration
//! Tier 4 — TokenOrm            token match + ORM/framework declaration
//! Tier 5 — TokenSql            token match + SQL-only declaration
//! Tier 6 — Named               filename resemblance only
//! ```
//!
//! ## Why a lattice, not individual laws
//!
//! The previous system had four separate ranking laws (004, 007, 009, 010).
//! They were encoding the same partial order from different angles. A new
//! wrong answer required a new law even if the principle was already there.
//!
//! With an explicit lattice:
//! - Each law that touches ranking now points at a tier, not a rule
//! - New wrong answers either fit an existing tier (tune weights) or require
//!   a new tier (genuinely new principle — rare)
//! - The full ranking logic is readable as a table in one place
//!
//! ## Tier scores
//!
//! Scores are chosen so tier membership is structurally dominant: no amount
//! of usage+relations can push a concept from tier N into tier N-1.
//! Each tier's base score exceeds the maximum possible tiebreaker score
//! (MAX_USAGE_FILES + MAX_RELATION_LINKS = 600 points) by a large margin.
//!
//! ## Laws implemented here
//!
//! law-004 (ranking): exact name outranks token match
//!   → ExactOrm / ExactSql both outrank TokenOrm / TokenSql unconditionally
//!
//! law-007 (ranking): ORM declaration outranks SQL-only on exact ties
//!   → ExactOrm outranks ExactSql; TokenOrm outranks TokenSql
//!
//! law-009 (philosophy): declared ontology outranks name search
//!   → DeclaredOntology is tier 1, above all name-derived tiers
//!   → alias resolution in query.rs bypasses ranking entirely (exact lookup)
//!   → this tier exists to document WHERE in the lattice aliases sit if they
//!      ever do fall through to scoring

use crate::model::{same_word, Concept};
use serde::{Deserialize, Serialize};

// ── Tier weights ─────────────────────────────────────────────────────────────
// Each tier's base score must exceed MAX_TIEBREAKER so tier membership is
// unconditionally dominant. MAX_TIEBREAKER = MAX_USAGE_FILES + MAX_RELATION_LINKS.

const MAX_USAGE_FILES: i32 = 500;
const MAX_RELATION_LINKS: i32 = 100;
const MAX_TIEBREAKER: i32 = MAX_USAGE_FILES + MAX_RELATION_LINKS;

// Tier base scores — separation between tiers must exceed MAX_TIEBREAKER.
const TIER_GAP: i32 = MAX_TIEBREAKER + 1; // 601

pub const SCORE_DECLARED_ONTOLOGY: i32 = TIER_GAP * 5; // 3005  (tier 1)
pub const SCORE_EXACT_ORM: i32 = TIER_GAP * 4;         // 2404  (tier 2)
pub const SCORE_EXACT_SQL: i32 = TIER_GAP * 3;         // 1803  (tier 3)
pub const SCORE_TOKEN_ORM: i32 = TIER_GAP * 2;         // 1202  (tier 4)
pub const SCORE_TOKEN_SQL: i32 = TIER_GAP * 1;         //  601  (tier 5)
// Tier 6 (Named / filename resemblance) never reaches ranking — it is
// handled separately in query.rs before the scored path is reached.

/// Each file that observably accesses this concept. Tie-breaker only.
pub const USAGE_PER_FILE: i32 = 1;

/// Each schema-declared relation. Tie-breaker only.
pub const RELATION_PER_LINK: i32 = 1;

// ── Tier enum ─────────────────────────────────────────────────────────────────

/// The tier a candidate concept occupies in the precedence lattice.
/// Serialisable so it can appear in API / CLI output for debuggability —
/// "why did Website beat WebsiteEvent?" should have a one-word answer.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RankTier {
    /// archietect.toml alias — the project declared this mapping explicitly.
    /// law-009: declared ontology outranks all name-derived inference.
    DeclaredOntology,
    /// Candidate name matches query exactly AND has an ORM/framework declaration.
    /// law-004 + law-007: exact beats token; ORM beats SQL on exact ties.
    ExactOrm,
    /// Candidate name matches query exactly, SQL-only declaration.
    /// law-004: still beats any token match.
    ExactSql,
    /// Token match + ORM/framework declaration.
    /// law-007: ORM beats SQL on token ties.
    TokenOrm,
    /// Token match + SQL-only declaration.
    TokenSql,
}

impl RankTier {
    fn base_score(&self) -> i32 {
        match self {
            RankTier::DeclaredOntology => SCORE_DECLARED_ONTOLOGY,
            RankTier::ExactOrm        => SCORE_EXACT_ORM,
            RankTier::ExactSql        => SCORE_EXACT_SQL,
            RankTier::TokenOrm        => SCORE_TOKEN_ORM,
            RankTier::TokenSql        => SCORE_TOKEN_SQL,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            RankTier::DeclaredOntology => "declared-ontology",
            RankTier::ExactOrm        => "exact-orm",
            RankTier::ExactSql        => "exact-sql",
            RankTier::TokenOrm        => "token-orm",
            RankTier::TokenSql        => "token-sql",
        }
    }
}

// ── Score breakdown ───────────────────────────────────────────────────────────

/// Full scoring breakdown for one candidate — the "show your work" output.
/// Returned by `score()` so callers (query.rs, tests) can surface it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    pub tier: RankTier,
    pub tier_label: String,
    pub tier_score: i32,
    pub usage_score: i32,
    pub relation_score: i32,
    pub total: i32,
}

impl ScoreBreakdown {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "tier": self.tier_label,
            "tier_score": self.tier_score,
            "usage_score": self.usage_score,
            "relation_score": self.relation_score,
            "total": self.total,
        })
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Classify the tier for a candidate given the query term.
pub fn tier(name: &str, concept: &Concept, term: &str) -> RankTier {
    let exact = same_word(name, term);
    let orm = concept.declared_in.iter().any(|(_, k)| k != "sql");
    match (exact, orm) {
        (true, true)  => RankTier::ExactOrm,
        (true, false) => RankTier::ExactSql,
        (false, true) => RankTier::TokenOrm,
        (false, false)=> RankTier::TokenSql,
    }
}

/// Compute the full score breakdown for one candidate. Higher total wins.
pub fn score(name: &str, concept: &Concept, term: &str) -> ScoreBreakdown {
    let t = tier(name, concept, term);
    let tier_score = t.base_score();
    let usage_score = (concept.usage.len() as i32).min(MAX_USAGE_FILES) * USAGE_PER_FILE;
    let relation_score = (concept.relations.len() as i32).min(MAX_RELATION_LINKS) * RELATION_PER_LINK;
    let total = tier_score + usage_score + relation_score;
    ScoreBreakdown {
        tier_label: t.label().to_string(),
        tier: t,
        tier_score,
        usage_score,
        relation_score,
        total,
    }
}

/// Convenience: just the total score. Used by the sort in query.rs.
pub fn rank(name: &str, concept: &Concept, term: &str) -> i32 {
    score(name, concept, term).total
}
