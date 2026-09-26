//! Unified, read-only context composition for one architectural term.

use serde_json::{json, Value};
use std::path::Path;

pub fn for_term(
    root: &Path,
    idx: &crate::model::Index,
    graph: &crate::structural::StructuralGraph,
    term: &str,
) -> Value {
    let concept = crate::query::concept(idx, graph, term);
    let owner = crate::query::owner(idx, graph, term);
    let impact = crate::query::impact(idx, graph, term);
    let plan = crate::query::plan(idx, graph, term);
    let status = crate::query::status(idx, graph);
    let register = crate::register::register(idx, graph, root);
    let history = crate::store::read_history(root, Some(term), 10);
    let verdict = concept
        .get("verdict")
        .and_then(Value::as_str)
        .unwrap_or("UNKNOWN");
    let unknowns = json!({
        "coverage": status.get("structural_coverage").cloned().unwrap_or(Value::Null),
        "not_known": register.get("not_known").cloned().unwrap_or_else(|| json!([])),
        "not_known_count": register.get("not_known_count").cloned().unwrap_or_else(|| json!(0)),
        "freshness": status.get("freshness").cloned().unwrap_or(Value::Null),
        "note": "Unknown and unsupported evidence is retained here; it is never converted into absence."
    });
    let decisions = plan
        .get("extend")
        .and_then(Value::as_array)
        .map(|items| {
            Value::Array(
                items
                    .iter()
                    .flat_map(|item| {
                        item.get("existing_decisions")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default()
                    })
                    .collect(),
            )
        })
        .unwrap_or_else(|| json!([]));
    json!({"kind":"architectural_context","term":term,"verdict":verdict,"concept":concept,"owner":owner,"impact":impact,"plan":plan,"decisions":decisions,"history":history,"unknowns":unknowns,"evidence":{"freshness":status.get("freshness").cloned().unwrap_or(Value::Null),"structural_coverage":status.get("structural_coverage").cloned().unwrap_or(Value::Null),"source":"local index, deterministic query composition"},"note":"One read-only context packet composed from existing deterministic queries. Run guard on the actual patch before applying changes."})
}

#[cfg(test)]
mod tests {
    #[test]
    fn context_preserves_unknown_boundary() {
        let root = std::env::temp_dir().join(format!("archietect-context-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("lib.rs"), "pub struct Widget;\n").unwrap();
        let (idx, graph) = crate::scan::scan(&root);
        let out = super::for_term(&root, &idx, &graph, "missing-concept");
        assert_eq!(out["kind"], "architectural_context");
        assert!(out["unknowns"].get("freshness").is_some());
    }
}
