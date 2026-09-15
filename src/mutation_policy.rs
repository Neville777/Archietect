//! Decision authorization for observed mutations. Configuration is parsed as
//! TOML; comments and unrelated decision records never authorize a mutation.

use std::collections::{BTreeMap, BTreeSet};
use crate::model::DecisionStatus;

/// Return the new or substantively updated decision IDs applicable to each
/// target. An empty ID list means the target has no supplied authorization.
/// Names resolve only by exact identity, explicit alias, or an unambiguous
/// unqualified symbol name among the supplied targets.
pub fn applicable_decisions(
    before: &str,
    after: &str,
    targets: &[String],
    aliases: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let before = decisions(before)?;
    let after = decisions(after)?;
    // Validate all aliases, including those not mentioned by this patch.
    for alias in aliases.keys() {
        canonical(alias, aliases)?;
    }
    let identities: BTreeMap<_, _> = targets
        .iter()
        .map(|target| canonical(target, aliases).map(|name| (target.clone(), name)))
        .collect::<Result<_, _>>()?;
    let mut result: BTreeMap<String, Vec<String>> = targets
        .iter()
        .map(|target| (target.clone(), Vec::new()))
        .collect();
    for (id, record) in after {
        if record.status != DecisionStatus::Active {
            continue;
        }
        if before.get(&id) == Some(&record) {
            continue;
        }
        let mut applicable = BTreeSet::new();
        for link in &record.links {
            let link = canonical(link, aliases)?;
            let matched: Vec<_> = identities
                .iter()
                .filter(|(_, identity)| {
                    if link.contains("::") {
                        **identity == link
                    } else {
                        identity.rsplit("::").next() == Some(link.as_str())
                    }
                })
                .map(|(target, _)| target.clone())
                .collect();
            if matched.len() > 1 {
                return Err(format!(
                    "decision {id} link {link:?} is ambiguous; use a qualified target: {}",
                    matched.join(", ")
                ));
            }
            applicable.extend(matched);
        }
        for target in applicable {
            result.get_mut(&target).unwrap().push(id.clone());
        }
    }
    Ok(result)
}

#[derive(Debug, PartialEq, serde::Deserialize)]
struct Authorization {
    id: String,
    decision: String,
    because: String,
    #[serde(default)]
    rejected: Vec<String>,
    #[serde(default)]
    links: Vec<String>,
    #[serde(default)]
    status: DecisionStatus,
    #[serde(default)]
    superseded_by: Option<String>,
}

fn decisions(text: &str) -> Result<BTreeMap<String, Authorization>, String> {
    #[derive(serde::Deserialize)]
    struct Configuration {
        #[serde(default)]
        decision: Vec<Authorization>,
    }
    let parsed: Configuration =
        toml::from_str(text).map_err(|err| format!("invalid decision configuration: {err}"))?;
    let mut records = BTreeMap::new();
    for mut record in parsed.decision {
        if record.id.trim().is_empty()
            || record.decision.trim().is_empty()
            || record.because.trim().is_empty()
            || record.links.iter().any(|link| link.trim().is_empty())
        {
            return Err(
                "decision id, decision, because, and supplied links must be nonempty".into(),
            );
        }
        // Whitespace and link order changes alone do not supply a decision.
        record.id = record.id.trim().to_owned();
        record.decision = record.decision.trim().to_owned();
        record.because = record.because.trim().to_owned();
        record.links.sort();
        record.links.dedup();
        record.rejected.sort();
        record.rejected.dedup();
        let id = record.id.clone();
        if records.insert(id.clone(), record).is_some() {
            return Err(format!("duplicate decision id {id:?}"));
        }
    }
    validate_lifecycle(&records)?;
    Ok(records)
}

fn validate_lifecycle(records: &BTreeMap<String, Authorization>) -> Result<(), String> {
    for (id, record) in records {
        if record.status == DecisionStatus::Superseded {
            let target = record.superseded_by.as_deref().ok_or_else(|| {
                format!("superseded decision {id:?} must name superseded_by")
            })?;
            if target == id {
                return Err(format!("decision {id:?} cannot supersede itself"));
            }
            let successor = records.get(target).ok_or_else(|| {
                format!("decision {id:?} superseded_by target {target:?} does not exist")
            })?;
            if successor.status != DecisionStatus::Active {
                return Err(format!("decision {id:?} superseded_by target {target:?} is not active"));
            }
        } else if record.superseded_by.is_some() {
            return Err(format!("decision {id:?} has superseded_by but status is not superseded"));
        }
    }
    // Follow every supersession edge. The target must be active above, but
    // retaining cycle detection protects this validator if that rule evolves.
    for id in records.keys() {
        let mut seen = BTreeSet::new();
        let mut current = id.as_str();
        while let Some(next) = records.get(current).and_then(|r| r.superseded_by.as_deref()) {
            if !seen.insert(current) {
                return Err(format!("decision supersession cycle involving {id:?}"));
            }
            current = next;
        }
    }
    Ok(())
}

fn canonical(name: &str, aliases: &BTreeMap<String, String>) -> Result<String, String> {
    let mut current = name;
    let mut visited = BTreeSet::new();
    while let Some(next) = aliases.get(current) {
        if !visited.insert(current) {
            return Err(format!("alias cycle while resolving {name:?}"));
        }
        if next.trim().is_empty() {
            return Err(format!("empty alias target for {current:?}"));
        }
        current = next;
    }
    Ok(current.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(links: &str) -> String {
        format!(
            "[[decision]]\nid = 'tenant'\ndecision = 'Scope tenant data'\nbecause = 'Prevent cross-tenant reads'\nlinks = {links}\n"
        )
    }

    fn evaluate(
        before: &str,
        after: &str,
        targets: &[&str],
    ) -> Result<BTreeMap<String, Vec<String>>, String> {
        applicable_decisions(
            before,
            after,
            &targets.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            &BTreeMap::new(),
        )
    }

    #[test]
    fn multiline_links_authorize_each_exact_target() {
        let result = evaluate(
            "",
            &record("[\n 'Member',\n 'Contribution',\n]"),
            &["Member", "Contribution", "Tenant"],
        )
        .unwrap();
        assert_eq!(result["Member"], ["tenant"]);
        assert_eq!(result["Contribution"], ["tenant"]);
        assert!(result["Tenant"].is_empty());
    }

    #[test]
    fn substrings_and_comment_markers_do_not_authorize() {
        let result = evaluate("", &record("['ContributionModel']"), &["Contribution"]).unwrap();
        assert!(result["Contribution"].is_empty());
        assert!(evaluate(
            "",
            "# [[decision]]\nmessage = '[[decision]]'",
            &["Contribution"]
        )
        .unwrap()["Contribution"]
            .is_empty());
    }

    #[test]
    fn existing_decision_requires_a_substantive_update() {
        let old = record("['Member', 'Contribution']");
        assert!(evaluate(&old, &old, &["Member"]).unwrap()["Member"].is_empty());
        let reordered = record("['Contribution', 'Member']");
        assert!(evaluate(&old, &reordered, &["Member"]).unwrap()["Member"].is_empty());
        let updated = old.replace(
            "Prevent cross-tenant reads",
            "Enforce scope on background jobs too",
        );
        assert_eq!(
            evaluate(&old, &updated, &["Member"]).unwrap()["Member"],
            ["tenant"]
        );
    }

    #[test]
    fn aliases_resolve_transitively_and_cycles_fail() {
        let aliases = BTreeMap::from([
            ("ContributionModel".into(), "Payment".into()),
            ("Payment".into(), "Contribution".into()),
        ]);
        let result = applicable_decisions(
            "",
            &record("['ContributionModel']"),
            &["Contribution".into()],
            &aliases,
        )
        .unwrap();
        assert_eq!(result["Contribution"], ["tenant"]);
        let cycle = BTreeMap::from([("A".into(), "B".into()), ("B".into(), "A".into())]);
        assert!(applicable_decisions("", "", &[], &cycle).is_err());
    }

    #[test]
    fn qualified_symbols_require_unambiguous_links() {
        let targets = ["src/a.rs::Member", "src/b.rs::Member"];
        assert!(evaluate("", &record("['Member']"), &targets)
            .unwrap_err()
            .contains("ambiguous"));
        let result = evaluate("", &record("['src/a.rs::Member']"), &targets).unwrap();
        assert_eq!(result[targets[0]], ["tenant"]);
        assert!(result[targets[1]].is_empty());
        assert_eq!(
            evaluate("", &record("['Member']"), &targets[..1]).unwrap()[targets[0]],
            ["tenant"]
        );
    }

    #[test]
    fn alias_cannot_hide_ambiguous_symbol_identity() {
        let aliases = BTreeMap::from([("TenantMember".into(), "Member".into())]);
        let result = applicable_decisions(
            "",
            &record("['TenantMember']"),
            &["a.py::Member".into(), "b.py::Member".into()],
            &aliases,
        );
        assert!(result.unwrap_err().contains("ambiguous"));
    }

    #[test]
    fn malformed_and_duplicate_decisions_fail_closed() {
        for invalid in [
            "[[decision]]\nid = 'dummy'".to_owned(),
            record("['Member']") + &record("['Member']"),
            record("['Member']").replace("Scope tenant data", " "),
            "[[decision".into(),
        ] {
            assert!(evaluate("", &invalid, &["Member"]).is_err(), "{invalid}");
        }
    }

    #[test]
    fn legacy_decisions_default_to_active_and_authorize() {
        let result = evaluate("", &record("['Member']"), &["Member"]).unwrap();
        assert_eq!(result["Member"], ["tenant"]);
    }

    #[test]
    fn only_active_decisions_authorize_a_mutation() {
        let superseded = record("['Member']")
            .replace("id = 'tenant'", "id = 'old'\nstatus = 'superseded'\nsuperseded_by = 'current'");
        let current = record("['Member']")
            .replace("id = 'tenant'", "id = 'current'")
            .replace("Scope tenant data", "Current tenant policy");
        let result = evaluate("", &(superseded + &current), &["Member"]).unwrap();
        assert_eq!(result["Member"], ["current"]);
    }

    #[test]
    fn supersession_requires_active_existing_successor() {
        let missing = record("['Member']")
            .replace("id = 'tenant'", "id = 'old'\nstatus = 'superseded'\nsuperseded_by = 'missing'");
        assert!(applicable_decisions(&missing, &missing, &[], &BTreeMap::new())
            .unwrap_err().contains("does not exist"));

        let inactive_successor = missing.replace("missing", "current") +
            &record("['Member']").replace("id = 'tenant'", "id = 'current'\nstatus = 'retired'");
        assert!(applicable_decisions("", &inactive_successor, &["Member".into()], &BTreeMap::new())
            .unwrap_err().contains("not active"));
    }

    #[test]
    fn lifecycle_rejects_invalid_status_and_bad_superseded_by_shape() {
        let invalid = record("['Member']").replace("id = 'tenant'", "id = 'x'\nstatus = 'paused'");
        assert!(evaluate("", &invalid, &["Member"]).is_err());
        let active_with_target = record("['Member']")
            .replace("id = 'tenant'", "id = 'x'\nsuperseded_by = 'y'");
        assert!(evaluate("", &active_with_target, &["Member"]).is_err());
    }
}
