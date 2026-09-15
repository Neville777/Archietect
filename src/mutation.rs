//! Observed changes between the two contents of a patch. Extractors own syntax;
//! this module compares their established objects and evaluates decision links.
//! Only changed files are extracted. Missing evidence is an Unknown mutation.
use crate::{model::Index, structural::StructuralGraph};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MutationKind { Added, Removed, Modified, Renamed, RelationshipAdded, RelationshipRemoved, Unknown }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mutation {
    pub resource: String,
    pub file: String,
    pub object: String,
    pub kind: MutationKind,
    pub before: Option<Value>,
    pub after: Option<Value>,
    pub evidence: Vec<String>,
    pub dependent_files: Vec<String>,
    pub decision_required: bool,
    pub governing_decisions: Vec<String>,
    pub supplied_decisions: Vec<String>,
}

fn unknown(file: &str, reason: String) -> Mutation {
    Mutation { resource: file.into(), file: file.into(), object: "coverage".into(), kind: MutationKind::Unknown,
        before: None, after: None, evidence: vec![reason], dependent_files: vec![], decision_required: true,
        governing_decisions: vec![], supplied_decisions: vec![] }
}

fn documentation(path: &str) -> bool {
    matches!(Path::new(path).extension().and_then(|s| s.to_str()), Some("md" | "txt" | "rst" | "png" | "jpg" | "jpeg" | "svg" | "ico" | "icns"))
}
fn test_source(path: &str) -> bool {
    path.split('/').any(|p| matches!(p, "test" | "tests" | "__tests__" | "fixtures"))
        || path.contains(".test.") || path.contains(".spec.")
}
fn protected(idx: &Index, path: &str) -> bool {
    idx.decision_required_paths.iter().any(|p| {
        let p = p.trim_end_matches('/');
        p == "." || path == p || path.strip_prefix(p).is_some_and(|s| s.starts_with('/'))
    }) && !idx.excludes.iter().any(|p| path == p.trim_end_matches('/') || path.starts_with(&format!("{}/", p.trim_end_matches('/'))))
}

/// File-level dependents with exact import resolution only. This is a lower
/// bound from the supplied index, not an assertion about every possible caller.
fn dependents(graph: &StructuralGraph, file: &str) -> Vec<String> {
    let known: BTreeSet<String> = graph.file_facts.keys().cloned().chain(graph.symbols.values().map(|s| s.file.clone())).collect();
    let edges: Vec<_> = graph.imports.iter().filter_map(|i| i.relationship(&known, &graph.workspace_packages)).collect();
    let mut seen = BTreeSet::from([file.to_string()]);
    for _ in 0..3 {
        let next: Vec<_> = edges.iter().filter(|e| seen.contains(&e.to.0)).map(|e| e.from.0.clone()).collect();
        let old = seen.len();
        seen.extend(next);
        if seen.len() == old { break; }
    }
    seen.remove(file);
    seen.into_iter().collect()
}

fn objects(path: &str, text: &str) -> (BTreeMap<String, (String, Value)>, Vec<String>) {
    if text.is_empty() { return (BTreeMap::new(), vec![]); }
    let (facts, uncertainty) = crate::structural::extract_snapshot(path, text);
    let sources = crate::structural::symbol_sources(path, text);
    let mut uncertainty = uncertainty;
    let cfg_guarded = rust_cfg_guarded_duplicates(path, text);
    let mut seen_symbols = BTreeSet::new();
    for symbol in &facts.symbols {
        if !seen_symbols.insert(symbol.name.clone()) && !cfg_guarded.contains(&symbol.name) {
            uncertainty.push(format!("Duplicate symbol identity '{}' in {path}", symbol.name));
        }
    }
    // The production graph keys symbols by file/name, so a duplicate
    // declaration can be collapsed before it reaches the graph. Detect the
    // common declaration forms here and preserve that ambiguity as UNKNOWN.
    let declaration_names = regex::Regex::new(r"(?m)^(?:pub\s+)?(?:async\s+)?(?:fn|class|interface|struct|def|func)\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    let mut declared = BTreeSet::new();
    for capture in declaration_names.captures_iter(text) {
        let name = capture[1].to_string();
        if !declared.insert(name.clone()) && !cfg_guarded.contains(&name) {
            uncertainty.push(format!("Duplicate declaration identity '{}' in {path}", name));
        }
    }
    let (declarations, _) = crate::scan::extract_declarations(Path::new(path), text);
    let mut objects = BTreeMap::new();
    for d in declarations {
        let key = format!("concept::{}", d.name);
        let mut fields = d.fields; fields.sort();
        let mut relations = d.relations; relations.sort();
        objects.insert(key, (d.name.clone(), json!({"name": d.name, "kind": d.kind, "table": d.table, "fields": fields, "relations": relations})));
    }
    for s in facts.symbols {
        let content = sources.get(&s.name);
        objects.insert(format!("symbol::{}", s.name), (s.name.clone(), json!({"name": s.name, "kind": s.kind, "source": content, "observation_source": s.observation_source})));
    }
    for i in facts.imports {
        let mut names = i.names; names.sort();
        let value = json!({"module": i.to_module, "names": names});
        objects.insert(format!("relationship::{value}"), (path.into(), value));
    }
    for r in facts.routes {
        let value = serde_json::to_value(r).unwrap();
        objects.insert(format!("route::{value}"), (path.into(), value));
    }
    (objects, uncertainty)
}

/// Rust permits complementary platform implementations under `#[cfg(...)]`.
/// They share a source-level name but only one exists in a compiled target, so
/// treating that pair as an ambiguous duplicate makes ordinary cross-platform
/// maintenance impossible. This only suppresses a duplicate when *every*
/// declaration of that name is directly cfg-guarded; an ordinary duplicate
/// remains UNKNOWN and therefore fails closed.
fn rust_cfg_guarded_duplicates(path: &str, text: &str) -> BTreeSet<String> {
    if !path.ends_with(".rs") { return BTreeSet::new(); }
    let re = regex::Regex::new(r"^(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    let mut pending_cfg = false;
    let mut flags: BTreeMap<String, Vec<bool>> = BTreeMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("#[cfg(") { pending_cfg = true; continue; }
        if let Some(capture) = re.captures(trimmed) {
            flags.entry(capture[1].to_string()).or_default().push(pending_cfg);
            pending_cfg = false;
        } else if !trimmed.is_empty() && !trimmed.starts_with("#[") && !trimmed.starts_with("///") {
            pending_cfg = false;
        }
    }
    flags.into_iter().filter_map(|(name, declarations)| {
        (declarations.len() > 1 && declarations.iter().all(|guarded| *guarded)).then_some(name)
    }).collect()
}

fn compare(change: &crate::patch::FileChange, graph: &StructuralGraph) -> Vec<Mutation> {
    let path = change.after_path.as_deref().or(change.before_path.as_deref()).unwrap_or("unknown");
    if documentation(path) || path == "archietect.toml" { return vec![]; }
    let before_path = change.before_path.as_deref().unwrap_or(path);
    let (before, b_unknown) = objects(before_path, &change.before);
    let (after, a_unknown) = objects(path, &change.after);
    let mut mutations = Vec::new();
    let deps = dependents(graph, before_path);
    for reason in b_unknown.into_iter().chain(a_unknown).collect::<BTreeSet<_>>() {
        mutations.push(unknown(path, reason));
    }
    for key in before.keys().chain(after.keys()).collect::<BTreeSet<_>>() {
        let b = before.get(key); let a = after.get(key);
        if b == a {
            if change.before_path != change.after_path {
                let (name, value) = a.or(b).expect("equal object exists");
                let object = key.split("::").next().unwrap();
                let required = object == "concept" || deps.len() >= 10;
                mutations.push(Mutation {
                    resource: if object == "concept" { name.clone() } else { format!("{path}::{name}") },
                    file: path.into(), object: object.into(), kind: MutationKind::Renamed,
                    before: Some(value.clone()), after: Some(value.clone()),
                    evidence: vec![format!("Verified unchanged object moved from '{}' to '{}'", before_path, path), "Dependent files are exact resolved imports from the supplied index, capped at three hops".into()],
                    dependent_files: deps.clone(), decision_required: required,
                    governing_decisions: vec![], supplied_decisions: vec![],
                });
            }
            continue;
        }
        let (name, _) = a.or(b).unwrap();
        let object = key.split("::").next().unwrap();
        let relation = object == "relationship" || object == "route";
        let kind = match (b, a, relation) {
            (None, _, true) => MutationKind::RelationshipAdded,
            (_, None, true) => MutationKind::RelationshipRemoved,
            (None, _, _) => MutationKind::Added,
            (_, None, _) => MutationKind::Removed,
            // A pure Git move preserves the blob, so the observed object is
            // still the same architectural object under a new owning path.
            _ if change.before_path != change.after_path => MutationKind::Renamed,
            _ => MutationKind::Modified,
        };
        let required = object == "concept" || deps.len() >= 10;
        mutations.push(Mutation { resource: if object == "concept" { name.clone() } else { format!("{path}::{name}") }, file: path.into(), object: object.into(), kind: kind.clone(),
            before: b.map(|(_, v)| v.clone()), after: a.map(|(_, v)| v.clone()),
            evidence: if kind == MutationKind::Renamed {
                vec![format!("Verified unchanged object moved from '{}' to '{}'", before_path, path), "Dependent files are exact resolved imports from the supplied index, capped at three hops".into()]
            } else {
                vec!["Compared extractor observations of verified patch contents".into(), "Dependent files are exact resolved imports from the supplied index, capped at three hops".into()]
            },
            dependent_files: deps.clone(), decision_required: required,
            governing_decisions: vec![], supplied_decisions: vec![] });
    }
    if change.before_path != change.after_path && before.is_empty() && after.is_empty() {
        mutations.push(unknown(path, "File moved but no supported structural object established identity continuity".into()));
    }
    mutations
}

/// Shared CLI/MCP gate. A malformed patch never becomes an empty safe change.
pub fn evaluate(idx: &Index, graph: &StructuralGraph, diff: &str) -> Value {
    let enabled = !idx.decision_required_paths.is_empty();
    let changes = match crate::patch::materialize(Path::new(&idx.root), diff) {
        Ok(c) => c,
        Err(reason) => return json!({"status": "UNKNOWN", "enforcement_level": idx.enforcement, "mutations": [unknown("patch", reason)], "violations": if enabled {vec![json!({"kind":"unknown_structural_mutation", "reason":"Cannot establish patch contents", "next_command":"git diff --full-index --no-ext-diff --no-textconv"})]} else {vec![]}}),
    };
    let mut mutations: Vec<_> = changes.iter().flat_map(|c| compare(c, graph)).collect();
    for m in &mut mutations {
        m.decision_required &= enabled && protected(idx, &m.file) && !test_source(&m.file);
        m.governing_decisions = idx.decisions.iter().filter(|d| d.links.iter().any(|l| l == &m.resource || m.resource.ends_with(&format!("::{l}")))).map(|d| d.id.clone()).collect();
    }
    let targets: Vec<String> = mutations.iter().filter(|m| m.decision_required && m.kind != MutationKind::Unknown).map(|m| m.resource.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    let config = changes.iter().find(|c| c.after_path.as_deref() == Some("archietect.toml"));
    let supplied = config.map(|c| crate::mutation_policy::applicable_decisions(&c.before, &c.after, &targets, &idx.aliases)).unwrap_or(Ok(BTreeMap::new()));
    let mut violations = vec![];
    let supplied = match supplied { Ok(s) => s, Err(e) => { if enabled { violations.push(json!({"kind":"invalid_decision_update", "reason":e})); } BTreeMap::new() } };
    for m in &mut mutations {
        m.supplied_decisions = supplied.get(&m.resource).cloned().unwrap_or_default();
        if m.decision_required && (m.kind == MutationKind::Unknown || m.supplied_decisions.is_empty()) {
            violations.push(json!({"kind": if m.kind == MutationKind::Unknown {"unknown_structural_mutation"} else {"missing_architectural_decision"},
                "resource": m.resource, "file": m.file, "mutation": m.kind, "evidence":m.evidence,
                "governing_decisions": m.governing_decisions, "required": "A changed decision in archietect.toml linked to this exact resource", "next_command":format!("archietect plan '{}'", m.resource)}));
        }
    }
    json!({"status":if mutations.iter().any(|m| m.kind == MutationKind::Unknown) {"UNKNOWN"} else {"OBSERVED"}, "enforcement_level":idx.enforcement, "mutations":mutations, "violations":violations})
}
