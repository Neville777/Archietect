//! Read-only change-workflow evidence.
//!
//! `workflow_check` is an advisory composition of the existing architectural
//! queries. It does not mutate the repository, create a plan record, or block
//! unrelated work. Its purpose is to make missing evidence explicit before an
//! agent edits a cross-cutting code path.

use crate::model::Index;
use crate::structural::StructuralGraph;
use serde_json::{json, Value};
use std::path::Path;

/// Report which pieces of change evidence are available for a proposed goal.
///
/// A check is `complete` only when its input was supplied and the underlying
/// query ran successfully. Missing optional inputs are reported as
/// `not_provided`, never as failures. Only an explicitly failing guard or
/// edit verifier yields `blocked`; this keeps the tool useful for read-only
/// exploration and unrelated changes.
pub fn workflow_check(
    root: &Path,
    idx: &Index,
    graph: &StructuralGraph,
    goal: &str,
    impact_term: Option<&str>,
    patch: Option<&str>,
    file: Option<&str>,
    content: Option<&str>,
) -> Value {
    let freshness = crate::query::status(idx, graph)["freshness"].clone();
    let freshness_status = freshness["status"].as_str().unwrap_or("unknown");
    let plan = if goal.trim().is_empty() {
        json!({"status":"not_provided", "reason":"goal is empty"})
    } else {
        json!({"status":"complete", "evidence": crate::query::plan(idx, graph, goal)})
    };

    let impact = match impact_term.map(str::trim).filter(|s| !s.is_empty()) {
        Some(term) => json!({
            "status": "complete",
            "term": term,
            "evidence": crate::query::impact(idx, graph, term)
        }),
        None => json!({
            "status": "not_provided",
            "reason": "provide impact_term to trace a canonical concept before changing it"
        }),
    };

    let guard = match patch {
        Some(text) if !text.trim().is_empty() => {
            let result = crate::query::guard(idx, graph, text);
            let allowed = result["allowed"].as_bool().unwrap_or(false);
            json!({
                "status": if allowed { "complete" } else { "blocked" },
                "evidence": result
            })
        }
        _ => json!({
            "status": "not_provided",
            "reason": "provide patch to run the final duplicate-storage guard"
        }),
    };

    let verify = match (file.map(str::trim), content) {
        (Some(path), Some(text)) if !path.is_empty() && !text.is_empty() => {
            let result = crate::structural::verify_edit(path, text);
            json!({
                "status": if result.valid { "complete" } else { "blocked" },
                "file": path,
                "evidence": {
                    "valid": result.valid,
                    "errors": result.errors,
                    "warnings": result.warnings
                }
            })
        }
        _ => json!({
            "status": "not_provided",
            "reason": "provide file and full proposed content to run pre-write verification"
        }),
    };

    let checks = json!({
        "fresh_index": {
            "status": if freshness_status == "fresh" { "complete" } else { "advisory" },
            "evidence": freshness,
            "reason": if freshness_status == "fresh" { "persisted index matches a clean tracked HEAD" } else { "refresh/init the index before relying on static evidence" }
        },
        "plan": plan,
        "impact": impact,
        "guard": guard,
        "verify_edit": verify,
    });

    let blocked = ["guard", "verify_edit"].iter().any(|key| checks[*key]["status"] == "blocked");
    let all_provided = ["plan", "impact", "guard", "verify_edit"].iter().all(|key| {
        checks[*key]["status"] == "complete"
    });
    json!({
        "evidence": ["DECLARED", "USED"],
        "kind": "workflow_prerequisite_report",
        "repository": root.display().to_string(),
        "goal": goal,
        "checks": checks,
        "decision": if blocked { "blocked" } else if all_provided && freshness_status == "fresh" { "evidence_complete" } else { "advisory" },
        "safe_to_apply": if blocked { "no" } else if all_provided && freshness_status == "fresh" { "review_required" } else { "not_evaluated" },
        "mutated_repository": false,
        "note": "Advisory evidence composition. Missing optional inputs do not block unrelated work; guard and verify_edit failures do block this proposed change."
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(label: &str) -> std::path::PathBuf {
        let id = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("archietect-workflow-{label}-{id}"));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn missing_optional_evidence_is_advisory_not_blocking() {
        let dir = temp_path("advisory");
        let (idx, graph) = scan::scan(&dir);
        let report = workflow_check(&dir, &idx, &graph, "add billing", None, None, None, None);
        assert_eq!(report["decision"], "advisory");
        assert_eq!(report["safe_to_apply"], "not_evaluated");
        assert_eq!(report["checks"]["impact"]["status"], "not_provided");
        assert_eq!(report["mutated_repository"], false);
    }

    #[test]
    fn invalid_edit_is_explicitly_blocked() {
        let dir = temp_path("blocked");
        let (idx, graph) = scan::scan(&dir);
        let report = workflow_check(
            &dir, &idx, &graph, "edit", Some("Thing"), None,
            Some("src/main.rs"), Some("fn broken("),
        );
        assert_eq!(report["checks"]["verify_edit"]["status"], "blocked");
        assert_eq!(report["decision"], "blocked");
    }
}
