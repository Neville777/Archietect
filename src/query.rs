//! The queries — deterministic, evidence-carrying, refusing to bluff.
//!
//! Verdict vocabulary:
//!   ACTIVE        — declared AND observably used. Extend it.
//!   DECLARED_ONLY — the schema asserts it but nothing observed touches it.
//!                   May be scaffolding; confirm before extending or replacing.
//!   UNKNOWN       — only name resemblance. Needs human confirmation.
//!   ABSENT        — no declaration, no usage, no resemblance. Building is
//!                   justified.
//!
//! Canonical selection: the most USED declared concept, ties broken by
//! declared relations. Rows/usage alone never outrank declarations — a
//! declaration is the project speaking; usage is the project acting.

use crate::humanize::age_label;
use crate::model::{names_concept, same_word, Evidence, Index, Tier};
use crate::scoring;
use crate::structural::{routes_for_concept, structural_dependents, symbols_for_concept, StructuralGraph};
use serde_json::{json, Value};
use walkdir::WalkDir;

/// A few lines of real source around a declaration, read fresh from disk.
/// Deterministic (it's the file's own bytes) — not inference, just saving
/// the caller a Read/grep round trip for something Archietect already knows
/// the exact location of.
fn source_snippet(root: &str, file: &str, line: usize, context: usize) -> Option<String> {
    let text = std::fs::read_to_string(std::path::Path::new(root).join(file)).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    if line == 0 || line > lines.len() {
        return None;
    }
    let start = line.saturating_sub(1).saturating_sub(context);
    let end = (line - 1 + context + 1).min(lines.len());
    Some(
        lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, l)| format!("{}: {}", start + i + 1, l))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// Build the answer card for a concept KNOWN to exist by exact name — no
/// searching, no ranking. Used by alias resolution (law-010).
fn concept_card(idx: &Index, graph: &StructuralGraph, name: &str, term: &str) -> Value {
    let c = &idx.concepts[name];
    // Built via Concept::to_resource (resource.rs) — the general shape this
    // schema-layer struct projects onto; see SYSTEM_MEMORY.md. Same evidence
    // this function built inline before, just constructed in one place.
    let evidence: Vec<Evidence> = c.to_resource(name).evidence;
    let used = !c.usage.is_empty();
    let symbols = symbols_for_concept(graph, name);
    let routes = routes_for_concept(graph, name);
    json!({
        "concept": term,
        "verdict": if used { "ACTIVE" } else { "DECLARED_ONLY" },
        "canonical": name,
        "table": c.table,
        "fields": c.fields.iter().take(15).collect::<Vec<_>>(),
        "relations": c.relations,
        "competing": [],
        "used_by_files": c.usage.iter().map(|(f, _)| f).take(10).collect::<Vec<_>>(),
        "first_seen_ms": c.first_seen_ms,
        "first_seen_label": age_label(c.first_seen_ms),
        "last_verified_ms": c.last_verified_ms,
        "last_verified_label": age_label(c.last_verified_ms),
        "evidence": evidence,
        "structural_symbols": symbols.iter().take(10).map(|s| json!({
            "name": s.name, "kind": format!("{:?}", s.kind), "file": s.file, "line": s.line,
        })).collect::<Vec<_>>(),
        "structural_routes": routes.iter().take(10).map(|r| json!({
            "method": r.method, "path": r.path, "handler": r.handler, "file": r.file,
        })).collect::<Vec<_>>(),
        "confidence": if used { "high" } else { "medium — declared but no observed access; may be scaffolding" },
        "recommendation": format!("'{term}' already exists as '{name}'. Extend it; do not create a second implementation."),
    })
}

/// Resolve a declared alias (archietect.toml [aliases]) to its canonical
/// concept card. This is what a name search can never see: "episode" has no
/// table named for it, but the project itself declares episode = stories.
/// DECLARED tier, because the declaration file says so — not an inference.
///
/// Split out from `concept()` so it can be called BEFORE name-token search
/// runs at all (law-011), rather than only as a fallback when name search
/// comes up empty — the ordering that let GameTheoryEngine silently defeat
/// the declared theory=causal_hypotheses alias on TITAN.
fn resolve_alias(idx: &Index, graph: &StructuralGraph, term: &str, alias_key: &str, target: &str) -> Value {
    // LAW-010: an alias target is an EXACT concept name, not a search term.
    // Feeding it back through term matching broke on any multi-token target
    // — TITAN declares theory = "causal_hypotheses" and got UNKNOWN, because
    // no single token of the target matches the whole target. Exact lookup
    // first; term search only as fallback.
    let mut r = if idx.concepts.contains_key(target) {
        concept_card(idx, graph, target, target)
    } else {
        concept(idx, graph, target)
    };
    if r["canonical"].is_null() {
        // A declared alias pointing at nothing is itself a finding.
        return json!({
            "concept": term,
            "verdict": "UNKNOWN",
            "canonical": null,
            "evidence": [Evidence { tier: Tier::Declared,
                what: format!("archietect.toml declares '{alias_key}' = '{target}', but '{target}' is not a declared concept — the ontology file is stale or wrong") }],
            "confidence": "low — declaration exists but points at nothing",
            "recommendation": "Fix archietect.toml: the alias target does not exist in the scanned declarations.",
        });
    }
    r["concept"] = json!(term);
    r["resolved_via"] = json!("alias");
    if let Some(ev) = r["evidence"].as_array_mut() {
        ev.insert(0, serde_json::to_value(Evidence {
            tier: Tier::Declared,
            what: format!("archietect.toml declares '{alias_key}' = '{target}' — the project's own ontology, not an inference"),
        }).unwrap());
    }
    r
}

pub fn concept(idx: &Index, graph: &StructuralGraph, term: &str) -> Value {
    let term = term.trim();

    // LAW-011: declared ontology is checked BEFORE name-token search, full
    // stop — not merely as a fallback when no name matches. scoring.rs's own
    // tier lattice puts DeclaredOntology at tier 1, above ExactOrm/TokenOrm;
    // but the control flow here used to reach alias resolution only when
    // `declared` (name-token matches) came back EMPTY. That silently
    // defeated the ontology whenever an UNRELATED concept happened to share
    // a token with an alias key. Caught live on TITAN: archietect.toml
    // declares theory = "causal_hypotheses", but crates/titan_evolution
    // independently declares GameTheoryEngine — a real struct that
    // token-matches "theory" — and the old order let it win outright,
    // silently, with no error and no signal that the ontology was bypassed.
    if let Some((alias_key, target)) = idx
        .aliases
        .iter()
        .find(|(k, _)| same_word(k, term) || names_concept(k, term))
    {
        return resolve_alias(idx, graph, term, alias_key, target);
    }

    let mut declared: Vec<&String> = idx
        .concepts
        .keys()
        .filter(|n| names_concept(n, term))
        .collect();
    declared.sort_by_key(|n| {
        let c = &idx.concepts[n.as_str()];
        // Ranking is defined in scoring.rs — named constants with documented
        // rationale. Laws 004 and 007 both encode ranking rules; they point
        // here rather than duplicating logic in sort comparators.
        std::cmp::Reverse(scoring::rank(n, c, term))
    });

    if let Some(&canon) = declared.first() {
        let c = &idx.concepts[canon];
        let mut evidence: Vec<Evidence> = c
            .declared_in
            .iter()
            .map(|(f, k)| Evidence {
                tier: Tier::Declared,
                what: format!("{k} declaration in {f}"),
            })
            .collect();
        evidence.extend(c.usage.iter().take(8).map(|(f, k)| Evidence {
            tier: Tier::Used,
            what: format!("{k} access in {f}"),
        }));
        let used = !c.usage.is_empty();
        return json!({
            "concept": term,
            "verdict": if used { "ACTIVE" } else { "DECLARED_ONLY" },
            "canonical": canon,
            // memory, not cache: when this concept FIRST entered the index,
            // and when its evidence was last re-verified against the tree
            "first_seen_ms": c.first_seen_ms,
            "first_seen_label": age_label(c.first_seen_ms),
            "last_verified_ms": c.last_verified_ms,
            "last_verified_label": age_label(c.last_verified_ms),
            "table": c.table,
            "fields": c.fields.iter().take(15).collect::<Vec<_>>(),
            "relations": c.relations,
            "competing": declared.iter().skip(1).take(5).collect::<Vec<_>>(),
            "used_by_files": c.usage.iter().map(|(f, _)| f).take(10).collect::<Vec<_>>(),
            "evidence": evidence,
            "confidence": if used { "high" } else {
                "medium — declared but no observed access; may be scaffolding"
            },
            "recommendation": if used {
                format!("'{term}' already exists as '{canon}'. Extend it; do not create a second implementation.")
            } else {
                format!("'{term}' is declared as '{canon}' but nothing observably uses it. Confirm whether it is scaffolding before extending OR replacing.")
            },
        });
    }

    // Structural tier: no data/schema concept matched, but the term may
    // still be a real symbol — a function, class, or route that legitimately
    // isn't a storage model (a CLI command, a service, a handler). Checked
    // before the NAMED (filename-only) tier below because an actual symbol
    // match is stronger evidence than a bare filename resemblance. This is
    // what lets `archietect concept doctor` (a plain Rust function, not a
    // table) answer with where it's declared instead of a bare ABSENT.
    let mut structural_hits: Vec<&crate::structural::Symbol> = graph
        .symbols
        .values()
        .filter(|s| same_word(&s.name, term) || names_concept(&s.name, term))
        .collect();
    // law-004's lesson applied to this tier too: an EXACT name match must
    // outrank a same-token-family match, never lose to it on alphabetical
    // luck. Found dogfooding a real frontend — querying 'dashboardApi' (a
    // real `export const dashboardApi = {...}`) returned the unrelated
    // 'DashboardPage' function instead, purely because "DashboardPage"
    // sorted first alphabetically among the token-overlap candidates.
    structural_hits.sort_by(|a, b| {
        let exact_a = same_word(&a.name, term);
        let exact_b = same_word(&b.name, term);
        exact_b.cmp(&exact_a).then(a.file.cmp(&b.file)).then(a.name.cmp(&b.name))
    });
    // A route can exist with NO symbol sharing its name at all — e.g. a
    // gRPC `rpc SayHello(...)` has no standalone "SayHello" symbol, only a
    // Route entry. Without this, querying the route's own handler name hit
    // a bare ABSENT purely because the trigger above only ever looked at
    // `graph.symbols`. Found dogfooding a real .proto file.
    if structural_hits.is_empty() {
        let route_hits: Vec<&crate::structural::Route> = graph
            .routes
            .iter()
            .filter(|r| same_word(&r.handler, term) || names_concept(&r.handler, term))
            .take(10)
            .collect();
        if let Some(canon) = route_hits.first().map(|r| r.handler.clone()) {
            return json!({
                "concept": term,
                "verdict": "STRUCTURAL",
                "canonical": canon,
                "evidence": route_hits.iter().map(|r| Evidence {
                    tier: Tier::Declared,
                    what: format!("{} {} route declared in {}", r.method, r.path, r.file),
                }).collect::<Vec<_>>(),
                "routes": route_hits.iter().map(|r| json!({
                    "method": r.method, "path": r.path, "handler": r.handler, "file": r.file,
                })).collect::<Vec<_>>(),
                "confidence": "high — found in source as a real route handler, not a declared data/schema model",
                "recommendation": format!("'{term}' exists in source as a route/RPC handler but is not a schema/storage concept. Schema-concept ranking does not apply."),
            });
        }
    }

    if let Some(canon) = structural_hits.first().map(|s| s.name.clone()) {
        // Cross-reference routes: this tier used to only ever look at
        // `graph.symbols`, never `graph.routes` — so a real route handler
        // (e.g. a Next.js page component) would resolve as a plain
        // Function/Class with no hint that a route even exists. Two ways a
        // route counts as relevant: it's declared in the SAME FILE as a
        // matched symbol (the strong link — e.g. `DashboardPage` and
        // `GET /dashboard` are the same `page.tsx`), or the term itself
        // matches the route's handler name directly.
        let hit_files: std::collections::HashSet<&str> =
            structural_hits.iter().map(|s| s.file.as_str()).collect();
        let linked_routes: Vec<&crate::structural::Route> = graph
            .routes
            .iter()
            .filter(|r| hit_files.contains(r.file.as_str()) || same_word(&r.handler, term) || names_concept(&r.handler, term))
            .take(10)
            .collect();
        return json!({
            "concept": term,
            "verdict": "STRUCTURAL",
            "canonical": canon,
            // Via Symbol::to_resource (resource.rs) — see SYSTEM_MEMORY.md.
            // Each symbol yields exactly one evidence entry, same string as
            // before; construction just moved to one place.
            "evidence": structural_hits.iter().take(10).map(|s| s.to_resource().evidence[0].clone()).collect::<Vec<_>>(),
            "routes": linked_routes.iter().map(|r| json!({
                "method": r.method, "path": r.path, "handler": r.handler, "file": r.file,
            })).collect::<Vec<_>>(),
            // Read fresh from disk at query time — not persisted, not cached,
            // just the file's own text. Same determinism guarantee as every
            // other evidence field; it just saves the caller a round trip.
            "source": structural_hits.iter().take(3).filter_map(|s| {
                source_snippet(&idx.root, &s.file, s.line, 2).map(|excerpt| json!({
                    "file": s.file, "line": s.line, "excerpt": excerpt,
                }))
            }).collect::<Vec<_>>(),
            "confidence": "high — found in source as a real symbol, not a declared data/schema model",
            "recommendation": format!("'{term}' exists in source but is not a schema/storage concept (it's a function, class, route, or similar). Schema-concept ranking does not apply."),
        });
    }

    // NAMED tier: filename resemblance only — and the answer says so.
    let root_path = std::path::Path::new(&idx.root);
    let named: Vec<String> = WalkDir::new(&idx.root)
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir() {
                if matches!(
                    e.file_name().to_str(),
                    Some("node_modules" | ".git" | "target" | "dist" | ".next" | "__pycache__")
                ) {
                    return false;
                }
                // respect the same excludes used during scan
                let rel = e.path().strip_prefix(root_path).unwrap_or(e.path());
                let rel_str = rel.to_string_lossy();
                if idx.excludes.iter().any(|ex| {
                    let ex = ex.trim_end_matches('/');
                    rel_str == ex || rel_str.starts_with(&format!("{ex}/"))
                }) {
                    return false;
                }
            }
            true
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let stem = e.path().file_stem()?.to_str()?.to_string();
            if names_concept(&stem, term) {
                Some(
                    e.path()
                        .strip_prefix(&idx.root)
                        .unwrap_or(e.path())
                        .display()
                        .to_string(),
                )
            } else {
                None
            }
        })
        .take(10)
        .collect();

    if !named.is_empty() {
        return json!({
            "concept": term,
            "verdict": "UNKNOWN",
            "canonical": null,
            "evidence": named.iter().map(|f| Evidence {
                tier: Tier::Named,
                what: format!("filename resemblance only: {f}"),
            }).collect::<Vec<_>>(),
            "confidence": "low — name resemblance is not an architectural fact",
            "recommendation": "Needs human confirmation. Similarly named files exist but no schema declares this concept — inspect them before building anything.",
            "next_action": {
                "type": "ai_investigation",
                "read": named,
                "question": format!("Do any of these files implement or represent '{term}'?"),
                "note": "Any finding from reading these files is provisional. It is not an Archietect fact, is not persisted, and does not change this verdict — that only happens if a human adds an extractor, an alias, or a decision.",
                "escalation": "If this reflects a structural pattern rather than a one-off, propose an extractor or decision via `archietect proposal submit` instead of a one-off finding.",
                "if_universal_defect": "That proposal path is for LOCAL fixes (this project's own archietect.toml, or a new extractor only you run). If instead the actual problem looks like a defect in Archietect's own matching/ranking logic — something that would misfire on any codebase, not just this one — a local fix can't correct that. Report it instead: https://github.com/Neville777/Archietect/issues, with this query and its evidence attached.",
            },
        });
    }
    // Before declaring ABSENT: is part of this repo in a language Archietect
    // has no structural extractor for at all? If so, "no evidence found" does
    // not mean "confirmed absent" — it means "this repo has a blind spot."
    // Handed back as a structured, provisional gap — NOT a fact, and never
    // cached or promoted to a verdict above UNKNOWN by anything in this
    // process. An AI client may investigate the listed files and report
    // findings back to a human; it cannot make this query return ACTIVE.
    // Two distinct blind spots, both genuine: (a) a language on the
    // KNOWN_UNSUPPORTED list — scanned, but deliberately has no extractor —
    // found via the already-scanned file_facts; (b) a language NOBODY has
    // ever classified at all, which never entered file_facts in the first
    // place because scan.rs's walk excluded it before either pass ever saw
    // it — found via a fresh, cheap (extension-only, no content reads) live
    // walk. (b) is what actually caught a synthetic Lua file returning a
    // confident ABSENT instead of this verdict.
    let mut unsupported_files: Vec<String> = graph
        .file_facts
        .keys()
        .filter(|rel| {
            std::path::Path::new(rel)
                .extension()
                .and_then(|x| x.to_str())
                .map(|ext| {
                    let ext = ext.to_lowercase();
                    crate::structural::KNOWN_UNSUPPORTED
                        .iter()
                        .any(|(_, exts)| exts.contains(&ext.as_str()))
                })
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    unsupported_files.extend(
        crate::scan::unclassified_files(root_path, &idx.excludes, 200)
            .into_iter()
            .map(|(rel, _ext)| rel),
    );
    if !unsupported_files.is_empty() {
        unsupported_files.sort();
        unsupported_files.truncate(15);
        return json!({
            "concept": term,
            "verdict": "INSUFFICIENT_COVERAGE",
            "canonical": null,
            "evidence": [],
            "confidence": "unknown — this repository contains files in a language with no structural extractor; absence of evidence there is not evidence of absence",
            "recommendation": format!("No declared, structural, or named evidence for '{term}' — but Archietect cannot see into some of this repo's source at all. This is not a confirmed absence; see next_action."),
            "next_action": {
                "type": "ai_investigation",
                "read": unsupported_files,
                "question": format!("Do any of these files implement or represent '{term}'?"),
                "note": "Any finding from reading these files is provisional. It is not an Archietect fact, is not persisted, and does not change this verdict — that only happens if a human adds an extractor, an alias, or a decision.",
                "escalation": "If this language has no structural extractor at all, propose one via `archietect proposal submit --kind extractor` — it will be validated against the existing laws + invariants suite before anyone applies it.",
                "if_universal_defect": "That's for a missing extractor — a coverage gap, fixable locally. If instead this looks like a defect in Archietect itself (it should have understood this and didn't, in a way that would misfire on any codebase, not just this one), a local fix can't correct that. Report it: https://github.com/Neville777/Archietect/issues, with this query and its evidence attached.",
            },
        });
    }

    json!({
        "concept": term,
        "verdict": "ABSENT",
        "canonical": null,
        "evidence": [],
        "confidence": "high — no declaration, no observed usage, no name resemblance",
        "recommendation": "Genuinely new for this project. Building it is justified.",
    })
}

const STOP: &[&str] = &[
    "want", "need", "make", "build", "better", "improve", "with", "that", "this", "have", "from",
    "into", "would", "should", "could", "will", "more", "less", "system", "support", "please",
    "about", "when", "then", "them", "track", "tracking", "show", "view", "page", "data", "info",
    "some", "every", "feature", "the", "and", "for", "add", "create", "user", "users",
];

pub fn intent(idx: &Index, text: &str) -> Value {
    let mut terms = Vec::new();
    for w in text
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() >= 4 && !STOP.contains(w))
    {
        if !terms.contains(&w.to_string()) {
            terms.push(w.to_string());
        }
    }
    terms.truncate(8);

    let mut extend = Vec::new();
    let mut create = Vec::new();
    let mut needs_confirmation = Vec::new();
    // Pass an empty structural graph — intent only needs schema-layer facts.
    // Structural enrichment is available on concept() directly when needed.
    let empty_graph = StructuralGraph::default();
    for t in &terms {
        let r = concept(idx, &empty_graph, t);
        match r["verdict"].as_str().unwrap_or("") {
            "ACTIVE" | "DECLARED_ONLY" => extend.push(json!({
                "concept": t,
                "canonical": r["canonical"],
                "verdict": r["verdict"],
                "relations": r["relations"],
            })),
            "UNKNOWN" => needs_confirmation.push(json!({
                "concept": t,
                "note": "name-resemblance only — confirm by hand",
            })),
            _ => create.push(t.clone()),
        }
    }
    let summary = if !extend.is_empty() {
        let names: Vec<String> = extend
            .iter()
            .filter_map(|e| e["canonical"].as_str().map(String::from))
            .collect();
        format!(
            "{} concept(s) already exist: extend {}. {}",
            extend.len(),
            names.join(", "),
            if create.is_empty() {
                "Nothing genuinely new required.".to_string()
            } else {
                format!("Only genuinely new: {}.", create.join(", "))
            }
        )
    } else if !create.is_empty() && needs_confirmation.is_empty() {
        "No named concept exists here — this intent is greenfield for this project.".to_string()
    } else {
        "Nothing matched declarations. Either the vocabulary differs from the project's or this is new territory.".to_string()
    };
    json!({
        "intent": text,
        "recognised": terms,
        "extend": extend,
        "create": create,
        "needs_confirmation": needs_confirmation,
        "smallest_correct_change": summary,
    })
}

pub fn impact(idx: &Index, graph: &StructuralGraph, term: &str) -> Value {
    // law-010 generalized: exact key first (see owner())
    let r = if idx.concepts.contains_key(term) {
        concept_card(idx, graph, term, term)
    } else {
        concept(idx, graph, term)
    };
    let Some(canon) = r["canonical"].as_str().map(String::from) else {
        return json!({
            "target": term,
            "impact": "unknown — concept not declared in this project",
            "detail": r,
        });
    };
    // `canon` is a schema-layer concept key ONLY when the verdict above was
    // DECLARED_ONLY/HAS_STORAGE/etc — a STRUCTURAL verdict (a class/function
    // that isn't a schema model, e.g. `GovernanceClient`) also sets
    // `canonical`, but that name was never inserted into `idx.concepts` at
    // all. Blindly indexing here panicked on exactly that real case
    // (`archietect impact GovernanceClient` on a real repo) — a
    // structural-only concept still has real impact data via
    // `structural_dependents` below, it just has no schema-layer
    // usage/relations to report, which is a fact to state, not a crash.
    let mut files: Vec<&String> = Vec::new();
    let mut dependents: Vec<&String> = Vec::new();
    if let Some(c) = idx.concepts.get(&canon) {
        files = c.usage.iter().map(|(f, _)| f).collect();
        files.sort();
        files.dedup();
        dependents = idx
            .concepts
            .iter()
            .filter(|(_, other)| other.relations.iter().any(|r| r == &canon))
            .map(|(n, _)| n)
            .collect();
    }
    let structural_dependents = structural_dependents(graph, &canon, 3);
    json!({
        "target": canon,
        "severity": if files.len() > 8 || dependents.len() > 3 || structural_dependents.len() > 5 {
            "HIGH — widely used and other models declare relations to it"
        } else if !files.is_empty() || !dependents.is_empty() || !structural_dependents.is_empty() {
            "MODERATE — several touchpoints"
        } else {
            "NONE OBSERVED — declared but nothing seen touching it"
        },
        "used_by_files": files.iter().take(20).collect::<Vec<_>>(),
        "declared_dependents": dependents,
        "structural_dependents": structural_dependents.iter().take(20).map(|d| json!({
            "file": d.file, "depth": d.depth, "via_symbols": d.via_symbols,
        })).collect::<Vec<_>>(),
        "evidence_note": "used_by = observed ORM/SQL access (USED tier); dependents = schema-declared relations (DECLARED tier); structural_dependents = files that import an owner of this concept, via the structural graph (transitive, capped at depth 3).",
    })
}

/// `Import::relationship`'s query surface: for one file, every exactly-
/// resolved import edge in and out of it. Deliberately NOT folded into
/// `status` — a full import graph can be hundreds or thousands of edges for
/// a real repo, and `status` already accumulates weight from every domain
/// section; dumping the whole graph into a command that runs on every
/// invocation would reproduce the exact token-cost regression this session
/// already found and fixed once (output shaping, `--only`/`--compact`).
/// This is a separate, one-file-at-a-time query instead, same shape as
/// `impact`.
pub fn imports(graph: &StructuralGraph, file: &str) -> Value {
    let known_files: std::collections::BTreeSet<String> = graph.file_facts.keys().cloned().collect();

    let outgoing: Vec<Value> = graph
        .imports
        .iter()
        .filter(|imp| imp.from_file == file)
        .filter_map(|imp| imp.relationship(&known_files))
        .map(|rel| json!({ "to": rel.to.0, "kind": rel.kind, "evidence": rel.evidence }))
        .collect();

    let incoming: Vec<Value> = graph
        .imports
        .iter()
        .filter_map(|imp| imp.relationship(&known_files).map(|rel| (imp, rel)))
        .filter(|(_, rel)| rel.to.0 == file)
        .map(|(imp, rel)| json!({ "from": imp.from_file, "kind": rel.kind, "evidence": rel.evidence }))
        .collect();

    json!({
        "file": file,
        "known_to_this_scan": known_files.contains(file),
        "imports": outgoing,
        "imported_by": incoming,
        "note": "Only exact, unambiguous relative-import resolutions are reported — see Import::relationship in structural.rs. A file with real imports that don't appear here either targets an external package (out of scope: no reliable exact-resolution strategy without a language-specific package resolver) or resolved ambiguously (more than one scanned file matched, so nothing is claimed).",
    })
}

pub fn status(idx: &Index, graph: &crate::structural::StructuralGraph) -> Value {
    let used = idx.concepts.values().filter(|c| !c.usage.is_empty()).count();
    let dead: Vec<&String> = idx
        .concepts
        .iter()
        .filter(|(_, c)| {
            c.usage.is_empty() && c.declared_in.iter().any(|(_, k)| k != "prisma-enum")
        })
        .map(|(n, _)| n)
        .take(25)
        .collect();
    let mut relationships = same_project_relationships(idx);
    relationships.extend(built_from_relationships(idx));
    json!({
        "root": idx.root,
        "files_scanned": idx.files_scanned,
        "declaration_files": idx.declaration_files,
        "concepts_declared": idx.concepts.len(),
        "concepts_with_observed_usage": used,
        "declared_but_never_observed_in_use": dead,
        "structural_coverage": crate::structural::coverage_report(idx, graph),
        "git": git_status_section(idx),
        "docker": docker_status_section(idx),
        "relationships": relationships,
        "note": "'never observed in use' is evidence of absence at USED tier only — access styles v0 doesn't parse (raw drivers, GraphQL resolvers, services in other repos) are invisible. Stated so it cannot be mistaken for proof of death. See structural_coverage for which languages/frameworks in THIS repo Archietect can actually see structurally.",
    })
}

/// The first real cross-domain identity link — see SYSTEM_MEMORY.md's
/// "Identity is a link, and the mechanism for it already exists". This is
/// deliberately the ONE safe case that needs no name-matching at all: the
/// git domain's `git_repository` resource and the code domain's resources
/// were extracted from the literally identical root path, in this same
/// `status()` call. That shared root IS the declared, checkable fact — not
/// a comparison of the two domains' names/identities against each other.
///
/// Returns nothing when there's no git repository resource (git disabled,
/// or no `.git` here) or when the code domain found no files at this root
/// (an empty directory isn't "a codebase to link against"). Recomputes git
/// resources independently from `git_status_section` rather than sharing
/// its output — this keeps that function's existing shape/tests completely
/// untouched, at the cost of scanning `.git` twice per `status()` call,
/// which is cheap (two small file reads, no subprocess).
fn same_project_relationships(idx: &Index) -> Vec<crate::resource::Relationship> {
    let root = std::path::Path::new(&idx.root);
    let cfg = crate::permissions::default_global_config_path()
        .and_then(|p| crate::permissions::load(&p, root))
        .unwrap_or_default();
    if !crate::permissions::domain_allowed(&cfg, "git") {
        return Vec::new();
    }
    let git_resources = crate::git_domain::scan_if_allowed(&cfg, root);
    let Some(repo) = git_resources.iter().find(|r| r.kind == "git_repository") else {
        return Vec::new();
    };
    if idx.files_scanned == 0 {
        return Vec::new();
    }
    vec![crate::resource::Relationship {
        from: repo.id.clone(),
        kind: "same_project_as".to_string(),
        // The code domain has no single "whole codebase" resource today —
        // its resources are per-concept/per-symbol. `idx.root` itself is
        // the identity here: not a name, the actual shared location both
        // domains were scanned from.
        to: crate::resource::Identity(idx.root.clone()),
        evidence: Evidence {
            tier: Tier::Declared,
            what: format!(
                "'{}' (git domain) and the code resources scanned at '{}' were extracted from the same root path — a shared location, not a name match",
                repo.id.0, idx.root
            ),
        },
    }]
}

/// The second real cross-domain identity link — one step more specific than
/// `same_project_relationships`'s bare same-root co-location. A compose
/// service's `build.context` (docker_domain.rs, captured verbatim, no
/// resolution attempted there — see its own doc) names a REAL FILESYSTEM
/// PATH, not an identifier: whether it resolves to a genuine, existing
/// directory under this project's root is a structural fact check, exactly
/// like `Import::relationship` (structural.rs) resolving a relative import
/// path — not a comparison of the service's name, image tag, or any other
/// string against anything. No `names_concept`/string-equality of any kind
/// appears in this function; only path resolution and existence checks.
///
/// Uses real filesystem access (`canonicalize`) rather than the lexical-only
/// resolution `Import::relationship` uses for module specifiers — correct
/// here because a compose build context IS a real path on disk today, not a
/// virtual module specifier resolved by a bundler/runtime.
///
/// Emits nothing (silence, never a guess) when: no `build_context` is
/// declared; the context doesn't exist as a real directory; the context
/// resolves to a path OUTSIDE this project's root (`../elsewhere` — never
/// claim identity across an out-of-scope path); or the context IS the
/// project's own root (`context: .` or equivalent) — that trivial case is
/// already fully covered by `same_project_relationships` above, so emitting
/// a second, redundant relationship for it would be noise, not a new fact.
fn built_from_relationships(idx: &Index) -> Vec<crate::resource::Relationship> {
    let root = std::path::Path::new(&idx.root);
    let Ok(canon_root) = root.canonicalize() else { return Vec::new() };
    let cfg = crate::permissions::default_global_config_path()
        .and_then(|p| crate::permissions::load(&p, root))
        .unwrap_or_default();
    if !crate::permissions::domain_allowed(&cfg, "docker") {
        return Vec::new();
    }
    let resources = crate::docker_domain::scan_if_allowed(&cfg, root);
    let mut rels = Vec::new();
    for r in resources.iter().filter(|r| r.kind == "docker_service") {
        let Some(context) = r.attributes.get("build_context") else { continue };
        let Ok(canon_candidate) = root.join(context).canonicalize() else { continue };
        if !canon_candidate.starts_with(&canon_root) {
            continue; // escapes root — never claim identity across it
        }
        if canon_candidate == canon_root {
            continue; // trivial "this is the project root" — same_project_as already covers it
        }
        if !canon_candidate.is_dir() {
            continue;
        }
        let Ok(relative) = canon_candidate.strip_prefix(&canon_root) else { continue };
        let relative_display = relative.display().to_string();
        rels.push(crate::resource::Relationship {
            from: r.id.clone(),
            kind: "built_from".to_string(),
            to: crate::resource::Identity(relative_display.clone()),
            evidence: Evidence {
                tier: Tier::Declared,
                what: format!(
                    "'{}' declares build.context '{}' in {}, which resolves to the real, existing directory '{}' under this project's root",
                    r.id.0, context, r.location.file, relative_display
                ),
            },
        });
    }
    rels
}

/// The first real (non-test) call site for `git_domain`'s gated scan — see
/// SYSTEM_MEMORY.md's phase 3 report, which left this domain built but with
/// nothing surfacing it. Goes through `scan_if_allowed`, never `scan`
/// directly, so a disabled git domain (via `[domains] git = "disabled"` in
/// either config layer) is honestly reported as disabled rather than the
/// section silently vanishing — vanishing would be indistinguishable from
/// "this isn't a git repository", a different fact entirely. Fails open on
/// a missing/unreadable global permission config the same way
/// `scan_with_prior` already fails open on a missing `archietect.toml`
/// elsewhere in this codebase — a config problem here should not take down
/// `status`, the single most load-bearing command in the CLI.
fn git_status_section(idx: &Index) -> Value {
    let root = std::path::Path::new(&idx.root);
    let cfg = crate::permissions::default_global_config_path()
        .and_then(|p| crate::permissions::load(&p, root))
        .unwrap_or_default();
    if !crate::permissions::domain_allowed(&cfg, "git") {
        return json!({
            "enabled": false,
            "resources": [],
            "note": "the 'git' domain is disabled by permission config — see `archietect permissions`",
        });
    }
    let resources = crate::git_domain::scan_if_allowed(&cfg, root);
    json!({
        "enabled": true,
        "resources": resources,
    })
}

/// Second domain wired the same way `git_status_section` is — see that
/// function's doc for the honest-disabled-reporting and fail-open rationale,
/// both identical here. `docker` is NOT in `DEFAULT_ENABLED_DOMAINS` (unlike
/// git), so with zero config anywhere this reports disabled by default,
/// proven by a test in docker_domain.rs.
fn docker_status_section(idx: &Index) -> Value {
    let root = std::path::Path::new(&idx.root);
    let cfg = crate::permissions::default_global_config_path()
        .and_then(|p| crate::permissions::load(&p, root))
        .unwrap_or_default();
    if !crate::permissions::domain_allowed(&cfg, "docker") {
        return json!({
            "enabled": false,
            "resources": [],
            "note": "the 'docker' domain is disabled by permission config — see `archietect permissions`",
        });
    }
    let resources = crate::docker_domain::scan_if_allowed(&cfg, root);
    json!({
        "enabled": true,
        "resources": resources,
    })
}

/// THE LAW — reject CREATE TABLE for a concept that already has a canonical
/// implementation. Ported from the system this engine was extracted from,
/// where it gates autonomously generated patches. Fails OPEN on anything it
/// cannot parse: a guard that blocks all work on a hiccup costs more than the
/// duplication it prevents.
pub fn guard(idx: &Index, sql: &str) -> Value {
    let re = regex::RegexBuilder::new(
        r#"create\s+table\s+(?:if\s+not\s+exists\s+)?["'`]?(\w+)"#,
    )
    .case_insensitive(true)
    .build()
    .unwrap();
    let proposed: Vec<String> = re
        .captures_iter(sql)
        .map(|c| c[1].to_string())
        .collect();
    if proposed.is_empty() {
        return json!({
            "allowed": true,
            "reason": "no CREATE TABLE in this text",
            "findings": [],
        });
    }
    let mut findings = Vec::new();
    let mut blocked = Vec::new();
    for t in &proposed {
        // check the CONCEPT the table names, not the literal string —
        // `episodes` must collide with a declared `Story`-like model too.
        let head = t
            .trim_end_matches('s')
            .rsplit('_')
            .next()
            .unwrap_or(t)
            .to_string();
        let r = concept(idx, &StructuralGraph::default(), &head);
        let verdict = r["verdict"].as_str().unwrap_or("ABSENT");
        let canonical = r["canonical"].as_str().unwrap_or("").to_string();
        // The ONLY exemption is re-declaring the canonical's own storage table,
        // matched EXACTLY (case-insensitive). Fuzzy matching here let
        // `CREATE TABLE ghosts` through while model `Ghost` was ACTIVE —
        // caught by the first end-to-end test, kept as a law: near-names are
        // exactly what duplicates look like, so near-names must block.
        let canonical_table = idx
            .concepts
            .get(&canonical)
            .and_then(|c| c.table.as_deref())
            .unwrap_or(&canonical);
        if matches!(verdict, "ACTIVE" | "DECLARED_ONLY")
            && !canonical.is_empty()
            && !t.eq_ignore_ascii_case(canonical_table)
            && !t.eq_ignore_ascii_case(&canonical)
        {
            // Cite the governing DECISION when one is declared. "The table
            // already exists" states a fact; the decision states the REASONING
            // and the alternatives already considered — which is what stops
            // the same proposal returning next month under a different name.
            let cite = idx
                .decisions
                .iter()
                .find(|d| d.links.iter().any(|l| same_word(l, &head) || l.eq_ignore_ascii_case(t) || l.eq_ignore_ascii_case(&canonical)))
                .map(|d| format!(
                    " DECLARED DECISION ('{}'): {} Alternatives already considered and rejected: {}.",
                    d.id, d.because, d.rejected.join("; ")
                ))
                .unwrap_or_default();
            blocked.push(format!(
                "CREATE TABLE {t} rejected — '{head}' is already {verdict}, canonically implemented as '{canonical}'. Extend {canonical} instead.{cite}"
            ));
        }
        findings.push(json!({
            "proposed_table": t,
            "concept_checked": head,
            "verdict": verdict,
            "canonical": r["canonical"],
        }));
    }
    json!({
        "allowed": blocked.is_empty(),
        "reason": if blocked.is_empty() {
            format!("{} proposed table(s) check out as new", proposed.len())
        } else {
            blocked.join(" | ")
        },
        "findings": findings,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// The intern's interface — doctor, tour, duplicates, owner.
// No AI-generated prose anywhere: every line is derived from concepts,
// decisions, aliases, usage, and history. A hallucination-free onboarding is
// only possible BECAUSE the engine refuses to invent facts.
// ─────────────────────────────────────────────────────────────────────────────

fn top_segment(path: &str) -> String {
    // In a monorepo the first segment is a CONTAINER, not an owner — 'crates'
    // owns nothing; 'crates/titan_knowledge' is the answer a human wants.
    // The container list is ecosystem convention (like SKIP_DIRS), not a guess.
    const CONTAINERS: &[&str] = &["crates", "packages", "apps", "bin", "services", "libs", "modules"];
    let mut it = path.split('/');
    let first = it.next().unwrap_or("root");
    if CONTAINERS.contains(&first) {
        if let Some(second) = it.next() {
            if second.contains('.') {
                return first.to_string(); // a file directly in the container
            }
            return format!("{first}/{second}");
        }
    }
    first.to_string()
}

/// Repository summary for someone who just cloned it.
pub fn doctor(idx: &Index, graph: &crate::structural::StructuralGraph, root: &std::path::Path) -> Value {
    // Domains = where declarations LIVE (top-level directories) — derived from
    // the tree's own organisation, not from a curated list.
    let mut domains: std::collections::BTreeMap<String, usize> = Default::default();
    for (f, _) in &idx.declaration_files {
        *domains.entry(top_segment(f)).or_default() += 1;
    }
    let mut top: Vec<(&String, usize)> =
        idx.concepts.iter().map(|(n, c)| (n, c.usage.len())).collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    let unused: Vec<&String> = idx
        .concepts
        .iter()
        .filter(|(_, c)| c.usage.is_empty())
        .map(|(n, _)| n)
        .take(15)
        .collect();
    let recent = crate::store::read_history(root, None, 10);
    json!({
        "domains": domains,
        "top_concepts": top.iter().take(10).map(|(n, u)| json!({ "concept": n, "observed_uses": u })).collect::<Vec<_>>(),
        "declared_but_never_observed_in_use": unused,
        "recent_architectural_changes": recent,
        "things_to_read": idx.decisions.iter().map(|d| json!({
            "id": d.id, "decision": d.decision,
        })).collect::<Vec<_>>(),
        "structural_coverage": crate::structural::coverage_report(idx, graph),
        "counts": {
            "concepts": idx.concepts.len(),
            "files_scanned": idx.files_scanned,
            "declared_decisions": idx.decisions.len(),
            "declared_aliases": idx.aliases.len(),
        },
        "note": "Everything above is derived from declarations, usage, decisions and the timeline — nothing is generated prose. 'never observed in use' is evidence of absence at the USED tier only; verify before treating it as dead.",
    })
}

/// The onboarding tour. Common mistakes come from the ontology itself: every
/// alias is a "don't create X" waiting to happen, and every decision's
/// rejected list is literally what the next person is about to propose.
pub fn tour(idx: &Index, graph: &crate::structural::StructuralGraph) -> Value {
    let mut top: Vec<(&String, usize)> =
        idx.concepts.iter().map(|(n, c)| (n, c.usage.len())).collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));

    let mut mistakes: Vec<String> = idx
        .aliases
        .iter()
        .map(|(alias, target)| format!(
            "Don't create '{alias}' — '{target}' already owns that responsibility (declared alias)."
        ))
        .collect();
    for d in &idx.decisions {
        for r in &d.rejected {
            mistakes.push(format!(
                "Don't build '{r}' — considered and rejected in decision '{}': {}",
                d.id, d.decision
            ));
        }
    }
    // Reading time: decisions at ~200 words/min — arithmetic, not a guess.
    let words: usize = idx
        .decisions
        .iter()
        .map(|d| d.decision.split_whitespace().count() + d.because.split_whitespace().count())
        .sum();
    json!({
        "important_concepts": top.iter().take(8).map(|(n, _)| n).collect::<Vec<_>>(),
        "probably_ignorable": idx.concepts.iter()
            .filter(|(_, c)| c.usage.is_empty())
            .map(|(n, _)| n).take(10).collect::<Vec<_>>(),
        "common_mistakes": mistakes,
        "decisions_to_read": idx.decisions.iter().map(|d| &d.id).collect::<Vec<_>>(),
        "estimated_reading_minutes": (words / 200).max(1),
        "structural_coverage": crate::structural::coverage_report(idx, graph),
        "note": "Derived entirely from declarations, usage, aliases and decisions — no generated prose, nothing to hallucinate. 'probably_ignorable' means no observed use at the USED tier; confirm before deleting anything.",
    })
}

/// Suspected duplicate concepts: live pairs sharing a name token. Evidence of
/// RISK, not proof — stated as such.
pub fn duplicates(idx: &Index) -> Value {
    // Restricted to STORAGE-bearing concepts. Found by dogfooding on TITAN
    // (3,838 concepts once the rust pub-struct extractor landed): this loop
    // is O(n^2) name-token comparisons, and over the full concept set that
    // was 23 SECONDS for one call — bare `archietect` (which calls this)
    // would be unusably slow on any large Rust codebase, the opposite of
    // the git-status instant-glance this tool exists to be. It was also
    // pure noise: "AIAllocationSuggestion vs RustcSuggestion" sharing the
    // token "Suggestion" has zero architectural meaning. Storage duplication
    // (two tables for one concept) has a real cost — a migration, a merge.
    // Two unrelated in-memory structs sharing an English suffix do not.
    let names: Vec<&String> = idx
        .concepts
        .iter()
        .filter(|(_, c)| c.table.is_some())
        .map(|(n, _)| n)
        .collect();
    let mut pairs = Vec::new();
    let mut needs_alias = Vec::new();
    for i in 0..names.len() {
        for j in (i + 1)..names.len() {
            let (a, b) = (names[i], names[j]);
            let shared: Vec<String> = crate::model::name_tokens(a)
                .into_iter()
                .filter(|t| t.len() >= 5 && names_concept(b, t))
                .collect();
            if !shared.is_empty() {
                let (ca, cb) = (&idx.concepts[a.as_str()], &idx.concepts[b.as_str()]);
                let sql_only = |c: &crate::model::Concept| c.declared_in.iter().all(|(_, k)| k == "sql");
                let orm = |c: &crate::model::Concept| c.declared_in.iter().any(|(_, k)| k != "sql");
                // An ORM model beside an sql-tier table sharing its name is
                // PROBABLY one concept the merge law cannot fold, because the
                // model declares no table name (we refuse to run inflection
                // engines). That is not a duplicate — it is a missing link,
                // and the fix is a one-line alias declaration.
                let same_concept_unlinked =
                    (sql_only(ca) && orm(cb)) || (sql_only(cb) && orm(ca));
                let entry = json!({
                    "concepts": [a, b],
                    "shared_token": shared[0],
                    "declared_in": [ca.declared_in.first(), cb.declared_in.first()],
                });
                if same_concept_unlinked {
                    needs_alias.push(entry);
                } else {
                    pairs.push(entry);
                }
            }
        }
    }
    pairs.truncate(40);
    needs_alias.truncate(40);
    json!({
        "suspected_duplicates": pairs,
        "likely_same_concept_needs_alias": needs_alias,
        "note": "suspected_duplicates: name-token overlap is evidence of RISK, not proof — related concepts legitimately share vocabulary (Article/ArticleComment); the pairs worth investigating are the ones that surprise you. likely_same_concept_needs_alias: an ORM model beside an sql-tier table sharing its name is probably ONE concept the merge law cannot fold without a declared table name — declare the mapping in archietect.toml [aliases] to link them.",
    })
}

/// Who owns a concept — the directory that DECLARES it, then the directories
/// that use it. Declarations outweigh usage 2:1: maintaining the contract is
/// ownership; calling it is only interest.
pub fn owner(idx: &Index, graph: &StructuralGraph, term: &str) -> Value {
    // LAW-010, generalized: a known concept name is an EXACT key, not a
    // search term. plan() passed canonical names back through term search
    // and multi-token names matched nothing — the alias bug's twin.
    //
    // Two real bugs fixed here together, found by auditing every caller of
    // the same pattern after `impact` was fixed for the first one
    // (idx.concepts[&canon] panicking on a STRUCTURAL-only canonical name
    // that was never inserted into the schema-layer map — see
    // impact_structural_only_tests). This function had BOTH bugs at once,
    // and the first one was masking the second:
    //   1. `StructuralGraph::default()` was passed to concept()/concept_card
    //      instead of the REAL graph this function receives — so the
    //      structural-symbol search inside concept() always ran against an
    //      EMPTY graph. `owner <any-structural-only-symbol>` silently
    //      returned ABSENT/owner:null for something that plainly exists —
    //      not a crash, a confident WRONG ANSWER, worse than a panic for a
    //      tool whose entire premise is never doing that. Fixed by actually
    //      using the `graph` parameter.
    //   2. With (1) fixed, a structural-only canonical name can genuinely
    //      reach this function now (it couldn't before, because the empty
    //      graph meant concept() could never RETURN a STRUCTURAL verdict
    //      here) — so `idx.concepts[&canon]` needed the same fix impact()
    //      already got: look it up in the structural graph instead of
    //      assuming a schema-layer entry exists.
    let r = if idx.concepts.contains_key(term) {
        concept_card(idx, graph, term, term)
    } else {
        concept(idx, graph, term)
    };
    let Some(canon) = r["canonical"].as_str().map(String::from) else {
        return json!({ "target": term, "owner": null, "detail": r });
    };
    let Some(c) = idx.concepts.get(&canon) else {
        // Structural-only: ownership is simply its one real declaration
        // site — no schema-layer declared_in/usage to weigh against
        // anything else, so there is nothing to rank.
        let sym = graph.symbols.values().find(|s| s.name == canon);
        let owner_dir = sym.map(|s| top_segment(&s.file));
        return json!({
            "target": canon,
            "owner_directory": owner_dir,
            "because": owner_dir.as_ref().map(|d| format!(
                "'{d}' declares this structural symbol — its only known declaration site"
            )),
            "declared_in": sym.map(|s| vec![json!({ "file": s.file, "kind": format!("{:?}", s.kind) })]).unwrap_or_default(),
            "ranked_directories": owner_dir.map(|d| vec![json!({ "dir": d, "weight": 1 })]).unwrap_or_default(),
        });
    };
    // Ownership comes from DECLARING directories ONLY — found on TITAN:
    // three readers outvoted the single declaring directory under a weighted
    // sum, contradicting the stated principle. Interest must never outvote
    // the contract, at any count. Usage breaks ties among declarers and
    // ranks the interested parties, nothing more.
    let mut decl_score: std::collections::BTreeMap<String, usize> = Default::default();
    for (f, _) in &c.declared_in {
        *decl_score.entry(top_segment(f)).or_default() += 1;
    }
    let mut use_score: std::collections::BTreeMap<String, usize> = Default::default();
    for (f, _) in &c.usage {
        *use_score.entry(top_segment(f)).or_default() += 1;
    }
    let mut owners: Vec<(String, usize, usize)> = decl_score
        .iter()
        .map(|(d, n)| (d.clone(), *n, *use_score.get(d).unwrap_or(&0)))
        .collect();
    owners.sort_by(|a, b| (b.1, b.2).cmp(&(a.1, a.2)));
    let mut ranked: Vec<(String, usize)> = owners.iter().map(|(d, n, u)| (d.clone(), n * 2 + u)).collect();
    for (d, u) in &use_score {
        if !decl_score.contains_key(d) {
            ranked.push((d.clone(), *u));
        }
    }
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    let ranked_owner = owners.first().map(|(d, ..)| d.clone());
    json!({
        "target": canon,
        "owner_directory": ranked_owner.clone(),
        "because": ranked_owner.as_ref().map(|d| format!(
            "'{d}' holds the declaration(s) — maintaining the contract is ownership; calling it is only interest"
        )),
        "declared_in": c.declared_in,
        "ranked_directories": ranked.iter().take(6).map(|(d, s)| json!({ "dir": d, "weight": s })).collect::<Vec<_>>(),
    })
}

/// CI gate: check a unified diff's ADDED lines for architecture violations.
///
/// `git diff main... | archietect ci --root .` — fails the pipeline when a
/// patch introduces storage for a concept that already has a canonical
/// implementation. Two severities, honestly separated:
///
///   violations — CREATE TABLE colliding with an existing concept (the
///                guard's verdict; always fails)
///   warnings   — a new ORM declaration whose name collides with an existing
///                canonical (name evidence only — fails only with --strict,
///                because related concepts legitimately share vocabulary)
pub fn ci(idx: &Index, diff: &str, strict: bool) -> Value {
    // only lines the patch ADDS — removing architecture is not this gate's business
    let added: String = diff
        .lines()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .map(|l| &l[1..])
        .collect::<Vec<_>>()
        .join("\n");

    let g = guard(idx, &added);
    let violations: Vec<Value> = if g["allowed"] == false {
        vec![json!({ "kind": "duplicate_storage", "reason": g["reason"], "findings": g["findings"] })]
    } else {
        Vec::new()
    };

    // new ORM-style declarations in the added lines, name-checked
    let decl_re = regex::Regex::new(
        r"(?m)^\s*(?:model\s+(\w+)\s*\{|export\s+class\s+(\w+)|class\s+(\w+)\s*\([^)]*Model[^)]*\))",
    )
    .unwrap();
    let mut warnings = Vec::new();
    for cap in decl_re.captures_iter(&added) {
        let name = cap.get(1).or(cap.get(2)).or(cap.get(3)).map(|m| m.as_str()).unwrap_or("");
        if name.len() < 4 || idx.concepts.contains_key(name) {
            continue; // extending an existing concept is the GOAL, not a finding
        }
        for tok in crate::model::name_tokens(name) {
            // LAW-013: a shared generic architectural-role word (executor,
            // manager, handler, ...) is never by itself collision evidence.
            if tok.len() < 4 || crate::watch::GENERIC_ROLE_TOKENS.contains(&tok.to_lowercase().as_str()) {
                continue;
            }
            if let Some(existing) = idx.concepts.keys().find(|c| names_concept(c, &tok)) {
                warnings.push(json!({
                    "kind": "possible_duplicate_concept",
                    "new_declaration": name,
                    "collides_with": existing,
                    "via_token": tok,
                    "advice": format!("'{existing}' may already cover this — run `archietect concept {tok}` before merging"),
                }));
                break;
            }
        }
    }

    let fail = !violations.is_empty() || (strict && !warnings.is_empty());
    json!({
        "pass": !fail,
        "violations": violations,
        "warnings": warnings,
        "strict": strict,
        "note": "violations = CREATE TABLE colliding with an existing canonical (always fails). warnings = new declaration whose NAME collides (name evidence only; fails only under --strict, because related concepts legitimately share vocabulary).",
    })
}

/// The glance — bare `archietect`, the git-status of architecture. A pure
/// COMPOSITION of existing queries (freshness, drift, ontology, timeline)
/// plus suggestions DERIVED from the facts. Deliberately no "health: 92/100":
/// a composite score nobody measured is an unmeasured number wearing a
/// measured one's clothes, and it would teach readers to distrust the real
/// numbers beside it. Facts and derived suggestions only.
/// The one freshness notion in this codebase, shared by `glance` and
/// `register`: is there a persisted index at all. Kept as a function so the
/// two can never drift into different wordings for the same fact.
pub fn index_freshness(root: &std::path::Path) -> &'static str {
    if root.join("archietect.db").exists() {
        "current (incremental)"
    } else {
        "not persisted (this scan ran in-memory)"
    }
}

pub fn glance(idx: &Index, graph: &StructuralGraph, root: &std::path::Path) -> Value {
    let db = root.join("archietect.db");
    let dup = duplicates(idx);
    let needs_alias = dup["likely_same_concept_needs_alias"].as_array().map(|a| a.len()).unwrap_or(0);

    // stale aliases — ontology pointing at nothing
    let stale: Vec<&String> = idx
        .aliases
        .iter()
        .filter(|(_, target)| {
            !idx.concepts.contains_key(*target)
                && !idx.concepts.keys().any(|n| names_concept(n, target.trim_end_matches('s')))
        })
        .map(|(k, _)| k)
        .collect();

    // SUGGESTIONS, each derived from a fact:
    let mut suggestions: Vec<String> = Vec::new();
    // (a) STORAGE concept families sharing a token with no governing decision
    //     — "four ledgers exist and nothing records why". Restricted to
    //     concepts with a declared TABLE, found by dogfooding on TITAN: once
    //     the rust pub-struct extractor made every domain type a "concept",
    //     this drowned in 294 *Config structs, 108 *Result, 93 *Response —
    //     universal Rust naming conventions with ZERO duplication cost
    //     (every module having its own Config is normal; every module having
    //     its own ledger table is not). The "invites an Nth" reasoning is
    //     specifically about STORAGE — another table, another migration —
    //     which is exactly what table.is_some() identifies.
    let mut fam: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (name, _) in idx.concepts.iter().filter(|(_, c)| c.table.is_some()) {
        for tok in crate::model::name_tokens(name) {
            // LAW-013: same generic-role-word exemption as the CI guard and
            // the watch daemon's diff — a shared role word never counts.
            if tok.len() >= 5 && !crate::watch::GENERIC_ROLE_TOKENS.contains(&tok.to_lowercase().as_str()) {
                fam.entry(tok.to_lowercase()).or_default().push(name.clone());
            }
        }
    }
    // biggest families first — truncating alphabetically buried a 4-member
    // ledger family under five 3-member ones on the very first TITAN run
    let mut families: Vec<(&String, &Vec<String>)> = fam
        .iter()
        .filter(|(tok, members)| {
            members.len() >= 3
                && !idx.decisions.iter().any(|d| d.links.iter().any(|l| same_word(l, tok)))
        })
        .collect();
    families.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
    for (tok, members) in families.iter().take(5) {
        suggestions.push(format!(
            "Record why {} '{}' concepts exist ({}) — a family this size with no governing decision invites a {}th",
            members.len(), tok, members.join(", "), members.len() + 1
        ));
    }
    if needs_alias > 0 {
        suggestions.push(format!(
            "{needs_alias} model↔table pair(s) look like ONE concept the merge law cannot fold — declare the mapping in archietect.toml [aliases]"
        ));
    }
    for k in &stale {
        suggestions.push(format!("Fix stale alias '{k}' in archietect.toml — it points at a concept that no longer exists"));
    }

    // No persisted index is normal on a first run, not an error — the scan
    // just happened in memory. Treated as onboarding (a next step, not a
    // warning glyph) the way `git init` invites rather than scolds.
    let persisted = db.exists();
    json!({
        "repository": root.display().to_string(),
        "status": {
            "index": index_freshness(root),
            "concepts": idx.concepts.len(),
            "duplicate_storage_risks": needs_alias,
            "ontology_warnings": stale.len(),
            "declared_decisions": idx.decisions.len(),
        },
        "onboarding": if persisted { Value::Null } else {
            json!("Run `archietect init` to persist this index (instant lookups, no rescan), or `archietect watch` to keep it continuously warm and start recording architectural history.")
        },
        "recent_changes": crate::store::read_history(root, None, 5),
        "suggestions": suggestions,
        "structural_coverage": crate::structural::coverage_report(idx, graph),
        "note": "No health score, deliberately: a composite nobody measured is an unmeasured number wearing a measured one's clothes. Every line above is a fact or derived from one.",
    })
}

/// `archietect plan` — the agent's one-call architectural plan. PURE
/// COMPOSITION: intent resolution, then per-concept owner, impact, governing
/// decisions and duplicate risks, stitched into one answer. No new semantics,
/// no AI, no heuristics beyond the queries it composes — it exists because an
/// agent that needs five tool calls to assemble context will sometimes skip
/// two of them, and the skipped ones are always the ones that mattered.
pub fn plan(idx: &Index, graph: &StructuralGraph, text: &str) -> Value {
    let it = intent(idx, text);
    let mut planned = Vec::new();
    for e in it["extend"].as_array().cloned().unwrap_or_default().iter().take(3) {
        let Some(canon) = e["canonical"].as_str() else { continue };
        let own = owner(idx, graph, canon);
        let imp = impact(idx, graph, canon);
        // governing decisions: any declared decision linking this concept
        let decisions: Vec<Value> = idx
            .decisions
            .iter()
            .filter(|d| {
                d.links.iter().any(|l| {
                    l.eq_ignore_ascii_case(canon) || names_concept(canon, l)
                })
            })
            .map(|d| json!({ "id": d.id, "decision": d.decision, "rejected": d.rejected }))
            .collect();
        let c = &idx.concepts[canon];
        planned.push(json!({
            "concept": e["concept"],
            "canonical": canon,
            "canonical_location": own["owner_directory"],
            "related": c.relations,
            "existing_decisions": decisions,
            "impact_if_changed": imp["severity"],
            "affected_files": imp["used_by_files"],
        }));
    }
    let create = it["create"].as_array().cloned().unwrap_or_default();
    json!({
        "intent": text,
        "extend": planned,
        "genuinely_new": create,
        "needs_confirmation": it["needs_confirmation"],
        "recommendation": it["smallest_correct_change"],
        "conflicts": if planned.is_empty() && create.is_empty() {
            json!("nothing recognised — vocabulary may differ from the project's")
        } else {
            json!("none detected at plan time — run `archietect guard` on the actual patch before applying")
        },
        "note": "Pure composition of intent/owner/impact/decisions — one call instead of five, because the skipped calls are always the ones that mattered. Deterministic; the guard still rules on the final patch.",
    })
}

#[cfg(test)]
mod status_git_section_tests {
    use super::*;

    /// `status` against THIS repository's own real .git — a real fixture,
    /// not synthetic, matching this codebase's own testing preference.
    #[test]
    fn status_includes_git_resources_for_this_real_repo() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let (idx, graph) = crate::scan::scan(&root);
        let out = status(&idx, &graph);
        assert_eq!(out["git"]["enabled"], true);
        let resources = out["git"]["resources"].as_array().expect("resources must be an array");
        assert!(
            resources.iter().any(|r| r["kind"] == "git_repository"),
            "expected a git_repository resource in status's git section, got: {resources:?}"
        );
        assert!(
            resources.iter().any(|r| r["kind"] == "git_branch"),
            "expected a git_branch resource (this repo has a checked-out branch)"
        );
    }

    /// A project-level `[domains] git = "disabled"` override must make the
    /// git section honestly report itself disabled, not silently vanish or
    /// fall back to `scan`'s ungated output.
    #[test]
    fn status_git_section_honestly_disabled_via_project_config() {
        let tmp = std::env::temp_dir()
            .join(format!("archietect-status-git-disabled-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::process::Command::new("git").arg("init").arg("-q").current_dir(&tmp).status().unwrap();
        std::fs::write(tmp.join("archietect.toml"), "[domains]\ngit = \"disabled\"\n").unwrap();

        let (idx, graph) = crate::scan::scan(&tmp);
        let out = status(&idx, &graph);
        assert_eq!(out["git"]["enabled"], false);
        assert_eq!(out["git"]["resources"].as_array().unwrap().len(), 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

#[cfg(test)]
mod same_project_relationship_tests {
    use super::*;

    /// A real project (tempdir, real `git init`, a real Prisma schema so
    /// `files_scanned` is genuinely > 0) must produce a `same_project_as`
    /// relationship whose evidence text names the actual shared root path —
    /// proving this is real co-location evidence, not a decorative constant.
    #[test]
    fn status_links_git_repository_to_code_by_shared_root() {
        let tmp = std::env::temp_dir()
            .join(format!("archietect-same-project-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::process::Command::new("git").arg("init").arg("-q").current_dir(&tmp).status().unwrap();
        std::fs::write(
            tmp.join("schema.prisma"),
            "model Widget {\n  id   Int    @id @default(autoincrement())\n  name String\n}\n",
        )
        .unwrap();

        let (idx, graph) = crate::scan::scan(&tmp);
        assert!(idx.files_scanned > 0, "sanity: the fixture must have real scanned files");
        let out = status(&idx, &graph);

        let rels = out["relationships"].as_array().expect("relationships must be an array");
        assert_eq!(rels.len(), 1, "expected exactly one same_project_as relationship, got: {rels:?}");
        let rel = &rels[0];
        assert_eq!(rel["kind"], "same_project_as");
        assert_eq!(rel["evidence"]["tier"], "Declared");
        let root_str = tmp.display().to_string();
        let what = rel["evidence"]["what"].as_str().unwrap();
        assert!(
            what.contains(&root_str),
            "evidence text must cite the actual shared root path {root_str}, got: {what}"
        );
        assert!(what.contains("same root path"), "evidence text must state the actual reason, got: {what}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A project with real code but no `.git` at all must produce NO
    /// relationship — there is nothing on the git side to link from.
    #[test]
    fn no_relationship_when_no_git_repository() {
        let tmp = std::env::temp_dir()
            .join(format!("archietect-same-project-nogit-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join("schema.prisma"),
            "model Widget {\n  id   Int    @id @default(autoincrement())\n  name String\n}\n",
        )
        .unwrap();

        let (idx, graph) = crate::scan::scan(&tmp);
        assert!(idx.files_scanned > 0, "sanity: the fixture must have real scanned files");
        let out = status(&idx, &graph);

        let rels = out["relationships"].as_array().expect("relationships must be an array");
        assert!(rels.is_empty(), "no .git directory means no same_project_as relationship, got: {rels:?}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// This repository's own real status output — proves the relationship
    /// fires end-to-end against a real, non-synthetic project.
    #[test]
    fn this_repo_has_a_same_project_relationship() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let (idx, graph) = crate::scan::scan(&root);
        let out = status(&idx, &graph);
        let rels = out["relationships"].as_array().expect("relationships must be an array");
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0]["kind"], "same_project_as");
    }
}

#[cfg(test)]
mod impact_structural_only_tests {
    use super::*;

    /// `impact()` on a STRUCTURAL-only concept (a real class/function that
    /// is not a schema model) must not panic. `concept()`'s STRUCTURAL
    /// branch sets `"canonical"` to a name that was never inserted into
    /// `idx.concepts` at all — found live against a real repository
    /// (`archietect impact GovernanceClient` on ghosttrack-monorepo), where
    /// `impact` unconditionally indexed `idx.concepts[&canon]` and panicked
    /// with "no entry found for key".
    #[test]
    fn impact_on_structural_only_concept_does_not_panic() {
        let tmp = std::env::temp_dir()
            .join(format!("archietect-impact-structural-only-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join("service.ts"),
            "export class GovernanceClient {\n  doSomething() {}\n}\n",
        )
        .unwrap();

        let (idx, graph) = crate::scan::scan(&tmp);
        assert!(!idx.concepts.contains_key("GovernanceClient"), "sanity: must be structural-only, not a schema concept");

        let out = impact(&idx, &graph, "GovernanceClient");
        assert_eq!(out["target"], "GovernanceClient");
        assert!(out["used_by_files"].as_array().unwrap().is_empty());
        assert!(out["declared_dependents"].as_array().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

#[cfg(test)]
mod owner_structural_only_tests {
    use super::*;

    /// `owner()` had TWO bugs at once, found by auditing every caller of the
    /// same `idx.concepts[&canon]` pattern after `impact` was fixed for the
    /// first instance of it: (1) it called concept()/concept_card with
    /// `StructuralGraph::default()` instead of the real graph it receives,
    /// so a structural-only symbol could never be found at all — a
    /// confident WRONG ANSWER (owner: null), not a crash, worse for a tool
    /// whose whole premise is never doing that; (2) once the real graph is
    /// used, a structural-only canonical name reaches `idx.concepts[&canon]`
    /// exactly like impact() did, and needed the identical fix. This test
    /// pins both: the real declaring directory must be found and reported.
    #[test]
    fn owner_on_structural_only_concept_finds_its_real_declaring_directory() {
        let tmp = std::env::temp_dir()
            .join(format!("archietect-owner-structural-only-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("services")).unwrap();
        std::fs::write(
            tmp.join("services").join("governance-client.ts"),
            "export class GovernanceClient {\n  evaluate() {}\n}\n",
        )
        .unwrap();

        let (idx, graph) = crate::scan::scan(&tmp);
        assert!(!idx.concepts.contains_key("GovernanceClient"), "sanity: must be structural-only, not a schema concept");

        let out = owner(&idx, &graph, "GovernanceClient");
        assert_eq!(out["target"], "GovernanceClient");
        assert_eq!(
            out["owner_directory"], "services",
            "must find the real declaring directory, not silently report null, got: {out}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A normal schema-layer concept's ownership must be completely
    /// unaffected by this fix — same ranked-directory behavior as before.
    #[test]
    fn owner_on_schema_concept_is_unaffected() {
        let tmp = std::env::temp_dir()
            .join(format!("archietect-owner-schema-unaffected-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join("schema.prisma"),
            "model Widget {\n  id Int @id\n  name String\n}\n",
        )
        .unwrap();

        let (idx, graph) = crate::scan::scan(&tmp);
        let out = owner(&idx, &graph, "Widget");
        assert_eq!(out["owner_directory"], "schema.prisma");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

#[cfg(test)]
mod built_from_relationship_tests {
    use super::*;

    fn tmp_project(label: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir()
            .join(format!("archietect-built-from-test-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// A real docker-compose service whose build.context names a real,
    /// existing, DISTINCT subdirectory must produce a `built_from`
    /// relationship, alongside the unaffected `same_project_as` link.
    #[test]
    fn built_from_relationship_appears_for_a_real_distinct_build_context() {
        let root = tmp_project("real-context");
        std::fs::write(root.join("schema.prisma"), "model Widget {\n  id Int @id\n}\n").unwrap();
        std::fs::create_dir_all(root.join("backend")).unwrap();
        std::fs::write(
            root.join("docker-compose.yml"),
            "services:\n  api:\n    build:\n      context: ./backend\n",
        )
        .unwrap();
        std::fs::write(root.join("archietect.toml"), "[domains]\ndocker = \"enabled\"\n").unwrap();

        let (idx, graph) = crate::scan::scan(&root);
        let out = status(&idx, &graph);
        let rels = out["relationships"].as_array().expect("relationships must be an array");

        let same_project = rels.iter().find(|r| r["kind"] == "same_project_as");
        assert!(same_project.is_none(), "sanity: no git repo here, so same_project_as legitimately absent — this fixture isolates built_from");

        let built_from = rels.iter().find(|r| r["kind"] == "built_from").expect("expected a built_from relationship");
        assert_eq!(built_from["to"], "backend");
        assert_eq!(built_from["evidence"]["tier"], "Declared");
        let what = built_from["evidence"]["what"].as_str().unwrap();
        assert!(what.contains("./backend"), "evidence must cite the literal declared context, got: {what}");
        assert!(what.contains("backend"), "evidence must cite the resolved directory, got: {what}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A build context that does not exist on disk must produce NO
    /// relationship — silence, never a guess.
    #[test]
    fn no_relationship_when_build_context_does_not_exist() {
        let root = tmp_project("missing-context");
        std::fs::write(root.join("schema.prisma"), "model Widget {\n  id Int @id\n}\n").unwrap();
        std::fs::write(
            root.join("docker-compose.yml"),
            "services:\n  api:\n    build:\n      context: ./does-not-exist\n",
        )
        .unwrap();
        std::fs::write(root.join("archietect.toml"), "[domains]\ndocker = \"enabled\"\n").unwrap();

        let (idx, graph) = crate::scan::scan(&root);
        let out = status(&idx, &graph);
        let rels = out["relationships"].as_array().expect("relationships must be an array");
        assert!(
            rels.iter().all(|r| r["kind"] != "built_from"),
            "a non-existent build context must never produce a relationship, got: {rels:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// `context: .` (the trivial "this service builds from the project's
    /// own root" case) must NOT produce a built_from relationship — that
    /// fact is already fully covered by same_project_as; a second,
    /// redundant relationship for it would be noise, not a new fact.
    #[test]
    fn no_built_from_relationship_for_trivial_root_context() {
        let root = tmp_project("root-context");
        std::fs::write(root.join("schema.prisma"), "model Widget {\n  id Int @id\n}\n").unwrap();
        std::fs::write(
            root.join("docker-compose.yml"),
            "services:\n  api:\n    build:\n      context: .\n",
        )
        .unwrap();
        std::fs::write(root.join("archietect.toml"), "[domains]\ndocker = \"enabled\"\n").unwrap();

        let (idx, graph) = crate::scan::scan(&root);
        let out = status(&idx, &graph);
        let rels = out["relationships"].as_array().expect("relationships must be an array");
        assert!(
            rels.iter().all(|r| r["kind"] != "built_from"),
            "context: . (the project's own root) must not produce a redundant built_from relationship, got: {rels:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// docker disabled (default, no config) must never produce a
    /// built_from relationship, even with a real, resolvable build context
    /// on disk — matches docker_domain.rs's own default-disabled contract.
    #[test]
    fn no_built_from_relationship_when_docker_domain_disabled() {
        let root = tmp_project("docker-disabled");
        std::fs::write(root.join("schema.prisma"), "model Widget {\n  id Int @id\n}\n").unwrap();
        std::fs::create_dir_all(root.join("backend")).unwrap();
        std::fs::write(
            root.join("docker-compose.yml"),
            "services:\n  api:\n    build:\n      context: ./backend\n",
        )
        .unwrap();
        // No archietect.toml at all — docker defaults to disabled.

        let (idx, graph) = crate::scan::scan(&root);
        let out = status(&idx, &graph);
        let rels = out["relationships"].as_array().expect("relationships must be an array");
        assert!(
            rels.iter().all(|r| r["kind"] != "built_from"),
            "docker disabled by default must yield no built_from relationship, got: {rels:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
