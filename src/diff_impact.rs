//! Change impact and static/runtime contradiction reports.
//!
//! These reports deliberately distinguish facts observed in the repository
//! from facts observed at runtime.  A changed file is not itself a broken
//! route, and a reachable route is not proof that every declared handler
//! works.  The report keeps those two evidence sources separate and only
//! emits a contradiction when both sides actually contain comparable facts.

use crate::model::Index;
use crate::structural::StructuralGraph;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub status: String,
    pub path: String,
    pub old_path: Option<String>,
}

fn changed_files(root: &Path, base: &str) -> Result<Vec<ChangedFile>, String> {
    let output = Command::new("git")
        .args(["diff", "--name-status", "-z", base, "--"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("could not run git diff: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git diff failed for base {base:?}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let fields: Vec<&[u8]> = output
        .stdout
        .split(|b| *b == 0)
        .filter(|v| !v.is_empty())
        .collect();
    let mut result = Vec::new();
    let mut i = 0;
    while i < fields.len() {
        let status = String::from_utf8_lossy(fields[i]).to_string();
        i += 1;
        let status_code = status.chars().next().unwrap_or('?');
        let path = String::from_utf8_lossy(fields.get(i).copied().unwrap_or_default()).to_string();
        i += 1;
        let (old_path, final_path) = if status_code == 'R' || status_code == 'C' {
            let new_path =
                String::from_utf8_lossy(fields.get(i).copied().unwrap_or_default()).to_string();
            i += 1;
            (Some(path), new_path)
        } else {
            (None, path)
        };
        result.push(ChangedFile {
            status,
            path: final_path,
            old_path,
        });
    }
    // `git diff` excludes untracked files. Include ordinary untracked files
    // separately so a new route or symbol is not silently omitted. Ignored
    // files remain outside this architectural boundary.
    let untracked = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("could not list untracked files: {e}"))?;
    if !untracked.status.success() {
        return Err(format!(
            "git untracked-file listing failed: {}",
            String::from_utf8_lossy(&untracked.stderr).trim()
        ));
    }
    let existing: BTreeSet<String> = result.iter().map(|f| f.path.clone()).collect();
    for field in untracked
        .stdout
        .split(|b| *b == 0)
        .filter(|v| !v.is_empty())
    {
        let path = String::from_utf8_lossy(field).to_string();
        if !existing.contains(&path) {
            result.push(ChangedFile {
                status: "??".into(),
                path,
                old_path: None,
            });
        }
    }
    result.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}

/// Build a deterministic impact report for the current worktree relative to
/// `base`. Tracked changes come from `git diff`; ordinary untracked files are
/// added from `git ls-files --others`. Ignored files and runtime behavior
/// remain outside this static report.
pub fn impact_report(root: &Path, base: &str, idx: &Index, graph: &StructuralGraph) -> Value {
    let files = match changed_files(root, base) {
        Ok(files) => files,
        Err(error) => {
            return json!({
                "evidence": "DECLARED+USED",
                "base": base,
                "error": error,
                "files": [],
                "complete": false,
            })
        }
    };
    let changed: BTreeSet<String> = files.iter().map(|f| f.path.clone()).collect();
    let mut concepts = BTreeMap::<String, BTreeSet<String>>::new();
    for (name, concept) in &idx.concepts {
        for (file, _) in concept.declared_in.iter().chain(concept.usage.iter()) {
            if changed.contains(file) {
                concepts
                    .entry(name.clone())
                    .or_default()
                    .insert(file.clone());
            }
        }
    }
    let symbols: Vec<Value> = graph
        .symbols
        .values()
        .filter(|s| changed.contains(&s.file))
        .map(|s| json!({"name": s.name, "kind": format!("{:?}", s.kind), "file": s.file, "line": s.line, "linked_concept": s.linked_concept}))
        .collect();
    let routes: Vec<Value> = graph
        .routes
        .iter()
        .filter(|r| changed.contains(&r.file))
        .map(|r| json!({"method": r.method, "path": r.path, "handler": r.handler, "file": r.file}))
        .collect();
    let importers: BTreeSet<String> = graph
        .imports
        .iter()
        .filter(|edge| changed.contains(&edge.to_module) || changed.contains(&edge.from_file))
        .map(|edge| edge.from_file.clone())
        .collect();
    json!({
        "evidence": ["DECLARED", "USED"],
        "base": base,
        "files": files.iter().map(|f| json!({"status": f.status, "path": f.path, "old_path": f.old_path})).collect::<Vec<_>>(),
        "changed_file_count": files.len(),
        "affected_concepts": concepts.into_iter().map(|(name, files)| json!({"concept": name, "files": files.into_iter().collect::<Vec<_>>()})).collect::<Vec<_>>(),
        "changed_symbols": symbols,
        "changed_routes": routes,
        "affected_importers": importers,
        "complete": false,
        "untracked_files_included": files.iter().filter(|f| f.status == "??").count(),
        "limitations": ["Ignored files are not included; use an explicit scan if they are relevant.", "Impact is static and does not prove runtime behavior, database migration application, or browser behavior."],
    })
}

/// Compare runtime HTTP observations (the JSON returned by runtime probes)
/// with statically declared routes. Only a matching route with a failing
/// status produces a contradiction; missing runtime observations remain
/// `UNVERIFIED`, not failures.
pub fn contradictions(graph: &StructuralGraph, runtime: &[Value]) -> Value {
    let mut findings = Vec::new();
    let mut checked = 0usize;
    for observation in runtime {
        let Some(path) = observation["path"].as_str() else {
            continue;
        };
        let Some(status) = observation["status"].as_u64() else {
            continue;
        };
        let matching: Vec<_> = graph
            .routes
            .iter()
            .filter(|r| route_matches(&r.path, path))
            .collect();
        if matching.is_empty() {
            continue;
        }
        checked += 1;
        if status >= 400 {
            for route in matching {
                findings.push(json!({
                    "kind": "static_runtime_contradiction",
                    "verdict": "CONTRADICTED",
                    "static": {"method": route.method, "path": route.path, "handler": route.handler, "file": route.file, "evidence": "DECLARED"},
                    "runtime": {"path": path, "status": status, "evidence": observation["evidence"].as_str().unwrap_or("RUNTIME")},
                    "reason": "A statically declared route was observed returning a failing HTTP status.",
                }));
            }
        }
    }
    json!({
        "evidence": ["DECLARED", "RUNTIME"],
        "checked_observations": checked,
        "contradictions": findings,
        "unverified_routes": graph.routes.len().saturating_sub(checked),
        "note": "Static declarations are not runtime health proof; routes without a matching runtime observation remain unverified.",
    })
}

fn route_matches(declared: &str, observed: &str) -> bool {
    fn normalize(path: &str) -> &str {
        path.split('?').next().unwrap_or(path).trim_end_matches('/')
    }
    let d = normalize(declared);
    let o = normalize(observed);
    if d == o {
        return true;
    }
    let ds: Vec<_> = d.split('/').filter(|s| !s.is_empty()).collect();
    let os: Vec<_> = o.split('/').filter(|s| !s.is_empty()).collect();
    ds.len() == os.len()
        && ds
            .iter()
            .zip(os.iter())
            .all(|(a, b)| a.starts_with(':') || *a == *b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structural::{Route, StructuralGraph};

    #[test]
    fn impact_includes_untracked_files_without_claiming_complete_coverage() {
        let root = std::env::temp_dir().join(format!(
            "archietect-impact-untracked-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        std::fs::write(root.join("tracked.rs"), "pub struct Tracked;\n").unwrap();
        git(&["add", "tracked.rs"]);
        git(&[
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=Test",
            "commit",
            "-qm",
            "base",
        ]);
        std::fs::write(root.join("new.rs"), "pub struct NewThing;\n").unwrap();
        let files = changed_files(&root, "HEAD").unwrap();
        assert!(
            files.iter().any(|f| f.path == "new.rs" && f.status == "??"),
            "{files:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn route_contradiction_is_not_inferred_without_runtime_evidence() {
        let graph = StructuralGraph {
            routes: vec![Route {
                method: "GET".into(),
                path: "/health".into(),
                handler: "health".into(),
                file: "src/routes.rs".into(),
            }],
            ..Default::default()
        };
        let out = contradictions(&graph, &[]);
        assert_eq!(out["contradictions"].as_array().unwrap().len(), 0);
        assert_eq!(out["unverified_routes"], 1);
    }

    #[test]
    fn failing_runtime_observation_contradicts_declared_route() {
        let graph = StructuralGraph {
            routes: vec![Route {
                method: "GET".into(),
                path: "/health".into(),
                handler: "health".into(),
                file: "src/routes.rs".into(),
            }],
            ..Default::default()
        };
        let out = contradictions(
            &graph,
            &[json!({"evidence":"RUNTIME", "path":"/health", "status":500})],
        );
        assert_eq!(out["contradictions"][0]["verdict"], "CONTRADICTED");
    }
}
