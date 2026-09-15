//! Decision authorization for observed mutations. Configuration is parsed as
//! TOML; comments and unrelated decision records never authorize a mutation.

use std::collections::{BTreeMap, BTreeSet};

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
    Ok(records)
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
}
