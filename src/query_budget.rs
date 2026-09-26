//! Deterministic query-efficiency instrumentation.
//!
//! Archietect normally loads its graph into memory, so an N+1 query is not
//! visible in a result.  Database-backed extensions can opt into this small
//! recorder around their SQLite calls.  It deliberately reports a suspicion,
//! not a false certainty: repeated statements with different bind keys in one
//! operation are evidence of an N+1 shape and should be replaced by a batch.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueryEvent {
    pub sql: String,
    /// A stable caller-supplied representation of the bound entity key.
    /// Values are never persisted by this module.
    pub bind_key: Option<String>,
    pub elapsed_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueryGroup {
    pub fingerprint: String,
    pub executions: usize,
    pub distinct_bind_keys: usize,
    pub total_elapsed_us: u64,
    pub example_sql: String,
    pub recommendation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueryAudit {
    pub total_queries: usize,
    pub suspicious_groups: Vec<QueryGroup>,
    pub n_plus_one_detected: bool,
    pub note: String,
}

/// A bounded recorder for one request/operation.  The bound prevents a
/// broken loop from turning diagnostics into an unbounded allocation.
#[derive(Debug, Clone)]
pub struct QueryBudget {
    events: Vec<QueryEvent>,
    max_events: usize,
    threshold: usize,
}

impl QueryBudget {
    pub fn new(max_events: usize, suspicious_threshold: usize) -> Self {
        Self {
            events: Vec::new(),
            max_events: max_events.max(1),
            threshold: suspicious_threshold.max(2),
        }
    }

    pub fn record(&mut self, sql: impl Into<String>, bind_key: Option<String>, elapsed: Duration) {
        if self.events.len() < self.max_events {
            self.events.push(QueryEvent {
                sql: sql.into(),
                bind_key,
                elapsed_us: elapsed.as_micros().min(u64::MAX as u128) as u64,
            });
        }
    }

    pub fn audit(&self) -> QueryAudit {
        audit_events(&self.events, self.threshold)
    }
}

/// Normalize SQL enough to group the same statement with different literals.
/// This intentionally does not attempt to parse SQL; it is a diagnostic guard,
/// and callers should supply `bind_key` for parameterized statements.
pub fn fingerprint(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\'' {
            out.push('?');
            while let Some(next) = chars.next() {
                if next == '\\' {
                    let _ = chars.next();
                } else if next == '\'' {
                    break;
                }
            }
        } else if ch.is_ascii_digit() {
            out.push('?');
            while chars
                .peek()
                .is_some_and(|c| c.is_ascii_digit() || *c == '.')
            {
                let _ = chars.next();
            }
        } else if ch.is_whitespace() {
            if !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push(ch.to_ascii_lowercase());
        }
    }
    out.trim().to_string()
}

pub fn audit_events(events: &[QueryEvent], threshold: usize) -> QueryAudit {
    let threshold = threshold.max(2);
    let mut groups: BTreeMap<String, (usize, BTreeSet<String>, u64, String)> = BTreeMap::new();
    for event in events {
        let key = fingerprint(&event.sql);
        let entry = groups
            .entry(key)
            .or_insert_with(|| (0, BTreeSet::new(), 0, event.sql.clone()));
        entry.0 += 1;
        if let Some(bind) = &event.bind_key {
            entry.1.insert(bind.clone());
        }
        entry.2 = entry.2.saturating_add(event.elapsed_us);
    }

    let suspicious_groups = groups
        .into_iter()
        .filter_map(|(fingerprint, (executions, keys, total, example_sql))| {
            // A repeated literal-free query can be a legitimate poll. Require
            // either distinct entity keys or a very high repetition count.
            if (keys.len() >= threshold && executions >= threshold)
                || executions >= threshold.saturating_mul(4)
            {
                Some(QueryGroup {
                    fingerprint,
                    executions,
                    distinct_bind_keys: keys.len(),
                    total_elapsed_us: total,
                    example_sql,
                    recommendation: "batch this lookup (IN/joins) or cache the parent query".into(),
                })
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    QueryAudit {
        total_queries: events.len(),
        n_plus_one_detected: !suspicious_groups.is_empty(),
        suspicious_groups,
        note: "This is a heuristic signal: inspect the caller before changing query shape.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_parameterized_queries_and_flags_n_plus_one() {
        let mut budget = QueryBudget::new(100, 3);
        for id in ["a", "b", "c", "d"] {
            budget.record(
                "SELECT * FROM concepts WHERE id = ?1",
                Some(id.into()),
                Duration::from_micros(10),
            );
        }
        let report = budget.audit();
        assert!(report.n_plus_one_detected);
        assert_eq!(report.suspicious_groups[0].executions, 4);
        assert_eq!(report.suspicious_groups[0].distinct_bind_keys, 4);
    }

    #[test]
    fn does_not_flag_a_single_lookup_or_small_repeat() {
        let events = vec![
            QueryEvent {
                sql: "SELECT * FROM concepts WHERE id = ?1".into(),
                bind_key: Some("a".into()),
                elapsed_us: 1,
            },
            QueryEvent {
                sql: "SELECT * FROM concepts WHERE id = ?1".into(),
                bind_key: Some("b".into()),
                elapsed_us: 1,
            },
        ];
        assert!(!audit_events(&events, 3).n_plus_one_detected);
    }

    #[test]
    fn bounds_recording() {
        let mut budget = QueryBudget::new(2, 2);
        for _ in 0..5 {
            budget.record("SELECT 1", None, Duration::ZERO);
        }
        assert_eq!(budget.audit().total_queries, 2);
    }
}
