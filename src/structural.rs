//! Structural graph — what the repository *contains*, extracted deterministically.
//!
//! This is Layer 1 of the three-layer architecture:
//!
//! ```text
//! Structural Graph   — what files/symbols/routes/events exist and how they relate
//! Semantic Layer     — what things appear to represent (concept identity, aliases)
//! Architecture Memory — what humans have decided (decisions, constraints, history)
//! ```
//!
//! ## Extraction principle
//!
//! Everything here is OBSERVED, not inferred. A symbol either exists in a file
//! or it does not. A file either imports another or it does not. An HTTP route
//! is either declared or it is not.
//!
//! Confidence is structural, not probabilistic:
//!   - `SymbolKind::Class`   — a class/struct/interface keyword was found
//!   - `SymbolKind::Function` — a function/method keyword was found
//!   - `SymbolKind::Route`   — a route decorator/call was found
//!   - `SymbolKind::Event`   — an event/message was published or handled
//!
//! ## What is NOT here
//!
//! - Concept identity ("OrderService represents the Order domain concept") →
//!   that is the semantic layer's job, done in query.rs
//! - Whether a symbol should be merged with another → semantic layer
//! - Human decisions about any of the above → architecture memory (store.rs)
//!
//! ## Caching
//!
//! The structural graph is cached per-file exactly like declaration facts:
//! (size, mtime, extractor version). A changed file is re-extracted; an
//! unchanged file reuses its prior `StructuralFileFacts`. The concept-set
//! dependency rule does NOT apply here — structural symbols are independent
//! of the schema extraction pass.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ── Types ────────────────────────────────────────────────────────────────────

/// The kind of a structural symbol. Determines how it participates in
/// concept matching and impact traversal.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SymbolKind {
    /// A class, struct, or interface declaration.
    Class,
    /// A function or method. Not at the top of a class (those are methods
    /// of their containing class). Top-level only — too noisy otherwise.
    Function,
    /// An interface or trait declaration.
    Interface,
    /// An event or message that is published or subscribed to.
    Event,
    /// An HTTP route handler (GET /foo, POST /foo/:id, etc.).
    Route,
}

/// One structural symbol extracted from a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    /// The declared name exactly as it appears in source.
    pub name: String,
    pub kind: SymbolKind,
    /// Repository-relative path of the file containing this symbol.
    pub file: String,
    /// The concept name this symbol is linked to, if the semantic layer has
    /// established a link. None until query.rs populates it after extraction.
    ///
    /// Kept Option<String> rather than a direct &Concept reference so the
    /// struct remains owned and serialisable — the link is a name, not a
    /// borrow. Populated by `link_to_concepts()` in this module.
    #[serde(default)]
    pub linked_concept: Option<String>,
    /// 1-indexed line number of the declaration, so a caller can show a real
    /// source excerpt instead of just a filename — still 100% deterministic
    /// (it's the file's own text), not an inference.
    #[serde(default)]
    pub line: usize,
}

/// 1-indexed line number containing byte offset `pos` in `text`.
fn line_of(text: &str, pos: usize) -> usize {
    text.as_bytes()[..pos.min(text.len())].iter().filter(|&&b| b == b'\n').count() + 1
}

/// An HTTP route extracted from source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub method: String,  // GET, POST, PUT, DELETE, PATCH, ...
    pub path: String,    // /orders, /orders/:id, ...
    pub handler: String, // the function/class name that handles it
    pub file: String,
}

/// A file-level import edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Import {
    /// Repository-relative path of the importing file.
    pub from_file: String,
    /// The imported module/file path, as written in source (may be relative
    /// or a package name — we store it verbatim, resolved paths are costly
    /// and fragile across package managers).
    pub to_module: String,
    /// The specific names imported, if available (named imports).
    /// Empty for default imports or wildcard imports.
    pub names: Vec<String>,
}

/// The full structural graph for one repository.
///
/// This is a VALUE TYPE — rebuilt from cache on every scan just like `Index`.
/// It lives alongside `Index` in the scan result and is serialised into
/// `architect.db` as a second row in the `idx` table.
///
/// ## Why not merge with Index?
///
/// `Index` is the schema/concept layer. Adding structural symbols to it would
/// make the schema→usage invalidation rule apply to structural extraction —
/// i.e., editing any schema file would force re-extraction of every source
/// file for symbols. That would be wrong: structural symbols are independent
/// of the schema pass. Separate struct, separate cache key, separate
/// invalidation rule.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StructuralGraph {
    /// All symbols, keyed by `{file}::{name}` for O(1) lookup.
    pub symbols: BTreeMap<String, Symbol>,
    /// File-level import edges.
    pub imports: Vec<Import>,
    /// HTTP routes found in source.
    pub routes: Vec<Route>,
    /// Per-file extraction cache — same shape as `Index::file_facts`.
    #[serde(default)]
    pub file_facts: BTreeMap<String, StructuralFileFacts>,
    /// Version of the structural extractor. Bump to invalidate all caches.
    #[serde(default)]
    pub extractor_version: u32,
}

/// What one file contributed to the structural graph, cached against
/// (size, mtime, extractor_version). Same invalidation model as FileFacts
/// in model.rs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StructuralFileFacts {
    pub size: u64,
    pub mtime_ms: i64,
    pub symbols: Vec<Symbol>,
    pub imports: Vec<Import>,
    pub routes: Vec<Route>,
}

/// Bump this when the structural extractors change semantics. Invalidates
/// all per-file structural caches, same as EXTRACTOR_VERSION in scan.rs.
///
/// Bumped to 2: added Swift/Objective-C/C-C++/Scala/Dart/Haskell/Clojure
/// extractors and Django route recognition in extract_py — a checked-in
/// validation corpus's cached architect.db predates both and would otherwise
/// keep reporting stale (e.g. zero Django routes) forever via the unchanged
/// (size, mtime) fast path.
pub const STRUCTURAL_EXTRACTOR_VERSION: u32 = 7; // +GraphQL, Protocol Buffers/gRPC, Compojure/Yesod/Vapor routes

// ── Public API ───────────────────────────────────────────────────────────────

/// Extract the structural graph for `root`, reusing `prior` for unchanged
/// files. Called from `scan::scan_with_prior` after the schema pass.
pub fn extract(
    _root: &std::path::Path,
    files: &[crate::scan::ScannableFile],
    prior: Option<&StructuralGraph>,
) -> StructuralGraph {
    use rayon::prelude::*;

    let prior_facts: BTreeMap<String, StructuralFileFacts> =
        prior.map(|p| p.file_facts.clone()).unwrap_or_default();

    let prior_version_matches =
        prior.map(|p| p.extractor_version == STRUCTURAL_EXTRACTOR_VERSION).unwrap_or(false);

    let results: Vec<(String, u64, i64, Vec<Symbol>, Vec<Import>, Vec<Route>)> = files
        .par_iter()
        .map(|f| {
            let unchanged = prior_version_matches
                && prior_facts
                    .get(&f.rel)
                    .map(|pf| pf.size == f.size && pf.mtime_ms == f.mtime_ms)
                    .unwrap_or(false);

            if unchanged {
                let pf = &prior_facts[&f.rel];
                return (
                    f.rel.clone(),
                    f.size,
                    f.mtime_ms,
                    pf.symbols.clone(),
                    pf.imports.clone(),
                    pf.routes.clone(),
                );
            }

            let Ok(text) = std::fs::read_to_string(&f.path) else {
                return (f.rel.clone(), f.size, f.mtime_ms, Vec::new(), Vec::new(), Vec::new());
            };

            let ext = f.path.extension().and_then(|x| x.to_str()).unwrap_or("");
            let (symbols, imports, routes) = extract_file(&f.rel, ext, &text);
            (f.rel.clone(), f.size, f.mtime_ms, symbols, imports, routes)
        })
        .collect();

    let mut graph = StructuralGraph {
        extractor_version: STRUCTURAL_EXTRACTOR_VERSION,
        ..Default::default()
    };

    for (rel, size, mtime_ms, symbols, imports, routes) in results {
        graph.file_facts.insert(
            rel.clone(),
            StructuralFileFacts { size, mtime_ms, symbols: symbols.clone(), imports: imports.clone(), routes: routes.clone() },
        );
        for s in &symbols {
            graph.symbols.insert(format!("{}::{}", rel, s.name), s.clone());
        }
        graph.imports.extend(imports);
        graph.routes.extend(routes);
    }

    graph
}

/// After concept extraction (schema pass) is complete, link structural symbols
/// to their most likely concept. This is OBSERVED linkage — a symbol name
/// token-matches a concept name — still deterministic, no AI. The link is
/// stored as `linked_concept` on the Symbol.
///
/// This is NOT semantic identity. `OrderService` linking to `Order` means
/// "these share a name token." Whether they represent the same domain concept
/// is the semantic layer's job.
pub fn link_to_concepts(
    graph: &mut StructuralGraph,
    concepts: &BTreeMap<String, crate::model::Concept>,
) {
    use crate::model::names_concept;

    for symbol in graph.symbols.values_mut() {
        // Only link classes/interfaces/routes — functions are too noisy.
        if !matches!(symbol.kind, SymbolKind::Class | SymbolKind::Interface | SymbolKind::Route | SymbolKind::Event) {
            continue;
        }
        // Find the best-matching concept by name token overlap.
        // Preference: exact match > token match.
        let linked = concepts
            .keys()
            .find(|cname| *cname == &symbol.name)
            .or_else(|| {
                concepts.keys().find(|cname| names_concept(&symbol.name, cname))
            });
        symbol.linked_concept = linked.cloned();
    }
}

/// Return all symbols in `graph` that are linked to `concept_name`, sorted
/// by kind then name. Used by `query::concept()` to enrich the concept card.
pub fn symbols_for_concept<'a>(
    graph: &'a StructuralGraph,
    concept_name: &str,
) -> Vec<&'a Symbol> {
    let mut out: Vec<&Symbol> = graph
        .symbols
        .values()
        .filter(|s| s.linked_concept.as_deref() == Some(concept_name))
        .collect();
    out.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.name.cmp(&b.name)));
    out
}

/// Return the transitive dependents of `concept_name` via the structural
/// graph import edges + symbol links. Used by `query::impact()`.
///
/// Algorithm:
///   1. Collect all files that contain a symbol linked to `concept_name`
///      ("owner files").
///   2. Walk import edges: any file that imports an owner file is a direct
///      dependent. Any file that imports a direct dependent is a transitive
///      dependent. We cap depth at 3 to avoid whole-repo flood.
///   3. Deduplicate and return with depth labels.
pub fn structural_dependents(
    graph: &StructuralGraph,
    concept_name: &str,
    depth_limit: usize,
) -> Vec<StructuralDependent> {
    // Step 1: owner files — files that declare a symbol for this concept.
    let owner_files: std::collections::HashSet<String> = graph
        .symbols
        .values()
        .filter(|s| s.linked_concept.as_deref() == Some(concept_name))
        .map(|s| s.file.clone())
        .collect();

    if owner_files.is_empty() {
        return Vec::new();
    }

    // Build a reverse import index: module_path → files that import it.
    // We match on the tail of `to_module` against file paths to handle
    // relative imports without resolving them.
    let mut reverse: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for imp in &graph.imports {
        reverse.entry(imp.to_module.clone()).or_default().push(imp.from_file.clone());
    }

    // Helper: given a set of file paths, which other files import any of them?
    let importers_of = |targets: &std::collections::HashSet<String>| -> std::collections::HashSet<String> {
        let mut result = std::collections::HashSet::new();
        for imp in &graph.imports {
            // Match if the to_module ends with the target file stem.
            let matches_any = targets.iter().any(|target| {
                let stem = std::path::Path::new(target)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(target);
                imp.to_module.ends_with(stem)
                    || imp.to_module.ends_with(target.as_str())
                    || imp.names.iter().any(|n| crate::model::names_concept(n, stem))
            });
            if matches_any && !targets.contains(&imp.from_file) {
                result.insert(imp.from_file.clone());
            }
        }
        result
    };

    let mut seen: std::collections::HashSet<String> = owner_files.clone();
    let mut frontier = owner_files.clone();
    let mut out = Vec::new();

    for depth in 1..=depth_limit {
        let next = importers_of(&frontier);
        let new_files: std::collections::HashSet<String> =
            next.difference(&seen).cloned().collect();
        if new_files.is_empty() {
            break;
        }
        for f in &new_files {
            // Find what symbols in this file link back to any concept.
            let symbols: Vec<String> = graph
                .symbols
                .values()
                .filter(|s| &s.file == f)
                .filter(|s| matches!(s.kind, SymbolKind::Class | SymbolKind::Interface))
                .map(|s| s.name.clone())
                .collect();
            out.push(StructuralDependent {
                file: f.clone(),
                depth,
                via_symbols: symbols,
            });
        }
        seen.extend(new_files.iter().cloned());
        frontier = new_files;
    }

    out.sort_by_key(|d| (d.depth, d.file.clone()));
    out
}

/// One structurally-dependent file, with the depth at which it was found
/// and the symbols in it that were on the path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuralDependent {
    pub file: String,
    pub depth: usize,
    pub via_symbols: Vec<String>,
}

/// Return all routes in `graph` that are linked to `concept_name`.
pub fn routes_for_concept<'a>(
    graph: &'a StructuralGraph,
    concept_name: &str,
) -> Vec<&'a Route> {
    use crate::model::names_concept;
    graph
        .routes
        .iter()
        .filter(|r| {
            names_concept(&r.handler, concept_name)
                || r.path.to_lowercase().contains(&concept_name.to_lowercase())
        })
        .collect()
}

// ── Language registry ────────────────────────────────────────────────────────
//
// One table is the seam for everything language-related: file-extension
// dispatch, the coverage report (`coverage_report` below), and eventually —
// if regex ever stops being good enough for a given language — the point
// where a real parser can replace one `extractor` fn without touching
// `extract_file`, `coverage_report`, or anything upstream of them. Regex is a
// perfectly fine MVP; the point of the table is that upgrading one language
// later is a local change, not a rewrite.

/// Every extractor fn is normalized to this shape (some languages don't
/// produce routes; they just leave that Vec untouched).
type ExtractFn = fn(&str, &str, &mut Vec<Symbol>, &mut Vec<Import>, &mut Vec<Route>);

pub struct LanguageSpec {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
    extractor: ExtractFn,
    /// What kind of source-level facts this extractor actually recognizes —
    /// shown verbatim in the coverage report so a caller knows what
    /// "supported" means for this language, not just that it's supported.
    pub symbol_support: &'static str,
    /// Web frameworks whose route-declaration syntax this extractor
    /// recognizes. A framework NOT listed here (e.g. Django's urls.py) will
    /// never produce a Route for this language, even though the language
    /// itself is supported.
    pub frameworks: &'static [&'static str],
}

pub const LANGUAGES: &[LanguageSpec] = &[
    LanguageSpec {
        name: "Rust",
        extensions: &["rs"],
        extractor: extract_rs,
        symbol_support: "structs, enums, traits, top-level functions",
        frameworks: &[],
    },
    LanguageSpec {
        name: "Python",
        extensions: &["py"],
        extractor: extract_py,
        symbol_support: "classes, top-level functions, routes",
        frameworks: &["FastAPI", "Flask", "Django"],
    },
    LanguageSpec {
        name: "TypeScript/JavaScript",
        extensions: &["ts", "tsx", "js", "jsx"],
        extractor: extract_ts_js,
        symbol_support: "classes, interfaces, enums, exported functions, routes, events",
        frameworks: &["Express", "NestJS", "Next.js", "Nuxt (server/api)"],
    },
    LanguageSpec {
        name: "Vue",
        extensions: &["vue"],
        extractor: extract_vue,
        symbol_support: "the SFC itself as a component (named by file), plus any exports in its <script> block",
        frameworks: &["Nuxt (pages)"],
    },
    LanguageSpec {
        name: "Go",
        extensions: &["go"],
        extractor: extract_go,
        symbol_support: "exported structs, interfaces, functions/methods",
        frameworks: &[],
    },
    LanguageSpec {
        name: "Java/Kotlin",
        extensions: &["java", "kt"],
        extractor: extract_java,
        symbol_support: "classes, interfaces, Kotlin top-level functions, routes",
        frameworks: &["Spring MVC"],
    },
    LanguageSpec {
        name: "Ruby",
        extensions: &["rb"],
        extractor: extract_rb,
        symbol_support: "classes, modules, methods, routes",
        frameworks: &["Rails"],
    },
    LanguageSpec {
        name: "Elixir",
        extensions: &["ex", "exs"],
        extractor: extract_ex,
        symbol_support: "modules, public functions, routes",
        frameworks: &["Phoenix"],
    },
    LanguageSpec {
        name: "PHP",
        extensions: &["php"],
        extractor: extract_php,
        symbol_support: "classes, interfaces, top-level functions",
        frameworks: &[],
    },
    LanguageSpec {
        name: "C#",
        extensions: &["cs"],
        extractor: extract_cs,
        symbol_support: "public classes/interfaces/records, public methods, routes",
        frameworks: &["ASP.NET Core"],
    },
    LanguageSpec {
        name: "Swift",
        extensions: &["swift"],
        extractor: extract_swift,
        symbol_support: "classes, structs, protocols, top-level functions, routes",
        frameworks: &["Vapor"],
    },
    LanguageSpec {
        name: "Objective-C",
        extensions: &["m", "mm"],
        extractor: extract_objc,
        symbol_support: "@interface/@implementation classes, @protocol, instance/class methods",
        frameworks: &[],
    },
    LanguageSpec {
        name: "C/C++",
        extensions: &["c", "h", "cpp", "hpp", "cc", "cxx"],
        extractor: extract_c,
        symbol_support: "structs, top-level function definitions; classes for .cpp/.hpp/.cc/.cxx only",
        frameworks: &[],
    },
    LanguageSpec {
        name: "Scala",
        extensions: &["scala"],
        extractor: extract_scala,
        symbol_support: "classes, objects, traits, top-level def",
        frameworks: &[],
    },
    LanguageSpec {
        name: "Dart",
        extensions: &["dart"],
        extractor: extract_dart,
        symbol_support: "classes, top-level functions",
        frameworks: &[],
    },
    LanguageSpec {
        name: "Haskell",
        extensions: &["hs"],
        extractor: extract_haskell,
        symbol_support: "data/newtype declarations, typeclasses, top-level type signatures, routes",
        frameworks: &["Yesod (parseRoutes quasi-quote only — Servant's type-level API DSL is not attempted, too unreliable to regex)"],
    },
    LanguageSpec {
        name: "Clojure",
        extensions: &["clj", "cljs"],
        extractor: extract_clojure,
        symbol_support: "public defn, defrecord/deftype, defprotocol, routes",
        frameworks: &["Compojure"],
    },
    LanguageSpec {
        name: "GraphQL",
        extensions: &["graphql", "gql"],
        extractor: extract_graphql,
        symbol_support: "type/interface/enum/input definitions, query/mutation/subscription operations",
        frameworks: &[],
    },
    LanguageSpec {
        name: "Protocol Buffers",
        extensions: &["proto"],
        extractor: extract_proto,
        symbol_support: "message types, services, rpc methods (as routes)",
        frameworks: &["gRPC"],
    },
];

/// Languages Architect can identify by extension but has NO extractor for —
/// listed explicitly so the coverage report can say "present, unsupported"
/// instead of silently omitting them. A language absent from BOTH tables is
/// simply not something this list anticipated; the report says so too.
pub const KNOWN_UNSUPPORTED: &[(&str, &[&str])] = &[];

/// Per-language, per-framework structural coverage for the files actually
/// present in this scan — the honest answer to "does Architect understand
/// this repo," instead of letting a user discover the boundary one UNKNOWN
/// concept query at a time.
pub fn coverage_report(graph: &StructuralGraph) -> serde_json::Value {
    let mut ext_counts: BTreeMap<String, usize> = BTreeMap::new();
    for rel in graph.file_facts.keys() {
        if let Some(ext) = std::path::Path::new(rel).extension().and_then(|x| x.to_str()) {
            *ext_counts.entry(ext.to_lowercase()).or_default() += 1;
        }
    }

    let supported: Vec<serde_json::Value> = LANGUAGES
        .iter()
        .filter_map(|lang| {
            let files: usize = lang.extensions.iter().filter_map(|e| ext_counts.get(*e)).sum();
            (files > 0).then(|| {
                serde_json::json!({
                    "language": lang.name,
                    "files": files,
                    "symbol_support": lang.symbol_support,
                    "frameworks_recognized": lang.frameworks,
                })
            })
        })
        .collect();

    let unsupported: Vec<serde_json::Value> = KNOWN_UNSUPPORTED
        .iter()
        .filter_map(|(name, exts)| {
            let files: usize = exts.iter().filter_map(|e| ext_counts.get(*e)).sum();
            (files > 0).then(|| serde_json::json!({ "language": name, "files": files }))
        })
        .collect();

    serde_json::json!({
        "supported": supported,
        "present_but_unsupported": unsupported,
        "note": "A file in an 'unsupported' language contributes no structural symbols or routes — a concept query for something implemented only there gets no STRUCTURAL evidence and will not guess from its filename alone. A 'supported' language's symbol_support/frameworks_recognized lists are exactly what is and isn't extracted — a framework not listed there (e.g. Django's urls.py) produces no Route even in a supported language.",
    })
}

// ── Per-file extraction ───────────────────────────────────────────────────────

fn extract_file(
    rel: &str,
    ext: &str,
    text: &str,
) -> (Vec<Symbol>, Vec<Import>, Vec<Route>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut routes = Vec::new();

    if let Some(lang) = LANGUAGES.iter().find(|l| l.extensions.contains(&ext)) {
        (lang.extractor)(rel, text, &mut symbols, &mut imports, &mut routes);
    }

    // Deduplicate symbols by name within a file — a class and its methods
    // would otherwise produce duplicates; we want only the class.
    symbols.dedup_by(|a, b| a.name == b.name && a.kind == b.kind);

    (symbols, imports, routes)
}

// ── TypeScript / JavaScript ───────────────────────────────────────────────────

fn extract_ts_js(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    routes: &mut Vec<Route>,
) {
    // Classes and interfaces
    let class_re = Regex::new(
        r"(?m)^(?:export\s+)?(?:abstract\s+)?(?:class|interface)\s+([A-Z][A-Za-z0-9_]*)"
    ).unwrap();
    for cap in class_re.captures_iter(text) {
        let name = cap[1].to_string();
        // Determine kind from the matched text
        let matched = &text[cap.get(0).unwrap().start()..cap.get(0).unwrap().end()];
        let kind = if matched.contains("interface") { SymbolKind::Interface } else { SymbolKind::Class };
        symbols.push(Symbol { name, kind, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // TypeScript enums (exported — they participate in concept identity)
    let enum_re = Regex::new(r"(?m)^export\s+(?:const\s+)?enum\s+([A-Z][A-Za-z0-9_]*)").unwrap();
    for cap in enum_re.captures_iter(text) {
        symbols.push(Symbol {
            name: cap[1].to_string(),
            kind: SymbolKind::Class, // treat enums like types for impact purposes
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
        });
    }

    // Imports: import { X, Y } from './module'
    let import_re = Regex::new(
        r#"import\s+(?:\*\s+as\s+\w+|\{([^}]*)\}|(\w+))\s+from\s+['"]([^'"]+)['"]"#
    ).unwrap();
    for cap in import_re.captures_iter(text) {
        let names: Vec<String> = cap
            .get(1)
            .map(|m| {
                m.as_str()
                    .split(',')
                    .map(|s| s.trim().split_whitespace().next().unwrap_or("").to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let to_module = cap[3].to_string();
        imports.push(Import { from_file: rel.to_string(), to_module, names });
    }

    // Top-level exported functions: `export function foo(` and the very
    // common `export const foo = (...) => {...}` arrow-as-function style.
    // Not anchored inside a class body — those are methods, already noisy
    // enough via the class itself.
    let fn_decl_re = Regex::new(
        r"(?m)^export\s+(?:default\s+)?(?:async\s+)?function\s+([A-Za-z_$][A-Za-z0-9_$]*)"
    ).unwrap();
    for cap in fn_decl_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }
    let fn_const_re = Regex::new(
        r"(?m)^export\s+const\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*(?:async\s*)?\("
    ).unwrap();
    for cap in fn_const_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // `export const authApi = { login: ..., logout: ... }` — an object-literal
    // namespace, the standard pattern for grouping related API/config methods
    // in TS/JS. Distinct from `fn_const_re` above (which requires `= (`, an
    // arrow function): this requires `= {`, an object. Found missing by
    // dogfooding a real Next.js/axios frontend — `authApi`/`dashboardApi`/
    // `swarmApi`-style exports were completely invisible (verdict ABSENT)
    // despite being exactly the kind of thing "does this API client already
    // exist" should answer. Classed as Class: architecturally it's the same
    // "named, importable unit of behavior" role a class plays here.
    let const_object_re = Regex::new(
        r"(?m)^export\s+const\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*(?::\s*[\w<>\[\],\.\s]+)?=\s*\{"
    ).unwrap();
    for cap in const_object_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // NestJS / Express route decorators and method calls
    extract_ts_routes(rel, text, routes);

    // Event emissions: EventEmitter.emit('event-name'), @OnEvent('...')
    extract_ts_events(rel, text, symbols);
}

fn extract_ts_routes(rel: &str, text: &str, routes: &mut Vec<Route>) {
    // NestJS decorators: @Get('/path'), @Post('/path'), etc.
    // Matches both single and double quoted paths.
    let decorator_re = Regex::new(
        r#"@(Get|Post|Put|Delete|Patch|Options|Head|All)\s*\(\s*["']([^"']*)["']"#
    ).unwrap();
    for cap in decorator_re.captures_iter(text) {
        let method = cap[1].to_string().to_uppercase();
        let path = cap[2].to_string();
        // Find the function name after the decorator
        let after = &text[cap.get(0).unwrap().end()..];
        let fn_re = Regex::new(r"(?m)^\s*(?:async\s+)?(\w+)\s*\(").unwrap();
        let handler = fn_re
            .captures(after)
            .map(|c| c[1].to_string())
            .unwrap_or_else(|| "unknown".to_string());
        routes.push(Route { method, path, handler, file: rel.to_string() });
    }

    // Express-style: router.get('/path', handler) or app.post('/path', ...)
    let express_re = Regex::new(
        r#"(?:router|app|Router)\.(get|post|put|delete|patch)\s*\(\s*["']([^"']+)["']"#
    ).unwrap();
    for cap in express_re.captures_iter(text) {
        routes.push(Route {
            method: cap[1].to_string().to_uppercase(),
            path: cap[2].to_string(),
            handler: "express-handler".to_string(),
            file: rel.to_string(),
        });
    }

    next_app_router_routes(rel, text, routes);
    nuxt_server_api_route(rel, routes);
    graphql_tagged_template_operations(rel, text, routes);
}

/// GraphQL operations embedded as `gql`...`` / `graphql`...`` tagged
/// templates — the standard way Apollo/urql clients declare queries even
/// with no schema layer in the same repo (the schema usually lives on a
/// separate backend). Reported the same way a standalone `.graphql` file's
/// operations are (see `extract_graphql`) — method = operation type,
/// path = operation name.
fn graphql_tagged_template_operations(rel: &str, text: &str, routes: &mut Vec<Route>) {
    let tag_re = Regex::new(r"(?s)\b(?:gql|graphql)\s*`([^`]*)`").unwrap();
    let op_re = Regex::new(r"\b(query|mutation|subscription)\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    for tag in tag_re.captures_iter(text) {
        for op in op_re.captures_iter(&tag[1]) {
            routes.push(Route {
                method: op[1].to_uppercase(),
                path: op[2].to_string(),
                handler: op[2].to_string(),
                file: rel.to_string(),
            });
        }
    }
}

/// Nuxt server routes: also file-based, but via a FILENAME suffix rather
/// than a directory convention — `server/api/hello.get.ts` -> GET /api/hello,
/// `server/api/echo.post.ts` -> POST /api/echo, `server/api/foo.ts` (no
/// method suffix) -> Nuxt's `defineEventHandler` handles every method, so
/// this reports "ANY" rather than guessing one. `server/routes/**` is the
/// same convention for routes outside `/api`.
fn nuxt_server_api_route(rel: &str, routes: &mut Vec<Route>) {
    let marker = if rel.contains("server/api/") {
        "server/api/"
    } else if rel.contains("server/routes/") {
        "server/routes/"
    } else {
        return;
    };
    let Some(after) = rel.split(marker).nth(1) else { return };
    let stem = std::path::Path::new(after).file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let dir = std::path::Path::new(after).parent().and_then(|p| p.to_str()).unwrap_or("");

    let known = ["get", "post", "put", "delete", "patch", "head", "options"];
    let (name, method) = match stem.rsplit_once('.') {
        Some((n, suffix)) if known.contains(&suffix) => (n, suffix.to_uppercase()),
        _ => (stem, "ANY".to_string()),
    };

    let mut path = format!("/{}", marker.trim_end_matches('/'));
    if !dir.is_empty() {
        path.push('/');
        path.push_str(dir);
    }
    path.push('/');
    // `[id]` -> `:id`, same convention as the App Router.
    if let Some(param) = name.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        path.push(':');
        path.push_str(param);
    } else {
        path.push_str(name);
    }

    routes.push(Route { method, path, handler: stem.to_string(), file: rel.to_string() });
}

// ── Vue / Nuxt ────────────────────────────────────────────────────────────────

fn extract_vue(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    routes: &mut Vec<Route>,
) {
    // The file IS the component — a Vue SFC almost never exports an
    // explicit name; identity is the filename itself (PascalCase), the same
    // convention Vue's own devtools/ESLint/IDE tooling already uses.
    // `index.vue` names nothing on its own (its directory does) — skipped.
    if let Some(stem) = std::path::Path::new(rel).file_stem().and_then(|s| s.to_str()) {
        if stem.to_lowercase() != "index" {
            symbols.push(Symbol { name: to_pascal_case(stem), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: 1 });
        }
    }

    // Nuxt pages: `pages/foo/[id].vue` -> GET /foo/:id, directory-based like
    // the Next.js App Router, just with the route file itself instead of a
    // `page.tsx` inside a directory.
    if let Some(after) = rel.split("pages/").nth(1) {
        let no_ext = after.trim_end_matches(".vue");
        let mut path = String::new();
        for seg in no_ext.split('/').filter(|s| !s.is_empty() && s.to_lowercase() != "index") {
            path.push('/');
            path.push_str(seg);
        }
        if path.is_empty() {
            path.push('/');
        }
        // A bracket param can BE a whole segment (`[id]`) or be embedded
        // inside one (`dynamic-[name]`, also valid Nuxt) — a substring
        // replace across the whole path handles both the same way, rather
        // than only matching when the bracket owns the entire segment.
        let bracket_re = Regex::new(r"\[\.\.\.([A-Za-z0-9_]+)\]|\[([A-Za-z0-9_]+)\]").unwrap();
        let path = bracket_re
            .replace_all(&path, |c: &regex::Captures| {
                c.get(1).map(|g| format!("*{}", g.as_str())).unwrap_or_else(|| format!(":{}", &c[2]))
            })
            .to_string();
        routes.push(Route { method: "GET".to_string(), path, handler: "default".to_string(), file: rel.to_string() });
    }

    // <script>/<script setup> is ordinary TS/JS underneath. Reuse that
    // extractor against the WHOLE file rather than slicing out just the
    // script block — `^`-anchored patterns don't spuriously match inside
    // <template>/<style>, and this way line numbers stay correct (they'd be
    // wrong if computed against an extracted substring instead of the real
    // file offsets).
    let mut discard_routes = Vec::new();
    extract_ts_js(rel, text, symbols, imports, &mut discard_routes);
}

fn to_pascal_case(s: &str) -> String {
    s.split(|c: char| c == '-' || c == '_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Next.js App Router: routing is FILE-BASED, not a decorator/DSL — the
/// route comes from the file's own path, not its content. `app/foo/[id]/
/// page.tsx` -> GET /foo/:id; `app/api/foo/route.ts` -> one Route per
/// exported HTTP-method handler (`export async function GET/POST/...`).
/// Route groups `(name)/` are stripped (they organize files, not URLs);
/// `[x]` -> `:x`, `[...x]` -> `*`. Found missing by dogfooding a real
/// Next.js frontend — `doctor` correctly SAID it only recognizes
/// Express/NestJS, rather than silently guessing, but that made routes:0 a
/// permanent fact for every Next.js App Router project rather than a gap
/// worth closing.
fn next_app_router_routes(rel: &str, text: &str, routes: &mut Vec<Route>) {
    let Some(app_pos) = rel.find("app/") else { return };
    if app_pos != 0 && rel.as_bytes().get(app_pos - 1) != Some(&b'/') {
        return;
    }
    let after_app = &rel[app_pos + 4..];
    let is_page = ["page.tsx", "page.ts", "page.jsx", "page.js"]
        .iter()
        .any(|f| after_app == *f || after_app.ends_with(&format!("/{f}")));
    let is_route_handler = ["route.ts", "route.js"]
        .iter()
        .any(|f| after_app == *f || after_app.ends_with(&format!("/{f}")));
    if !is_page && !is_route_handler {
        return;
    }

    let dir = after_app.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let mut path = String::new();
    for seg in dir.split('/').filter(|s| !s.is_empty()) {
        if seg.starts_with('(') && seg.ends_with(')') {
            continue; // route group — organizes files, invisible in the URL
        }
        if let Some(param) = seg.strip_prefix("[...").and_then(|s| s.strip_suffix(']')) {
            let _ = param;
            path.push_str("/*");
        } else if let Some(param) = seg.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            path.push_str("/:");
            path.push_str(param);
        } else {
            path.push('/');
            path.push_str(seg);
        }
    }
    if path.is_empty() {
        path.push('/');
    }

    if is_page {
        routes.push(Route { method: "GET".to_string(), path, handler: "default".to_string(), file: rel.to_string() });
        return;
    }
    let method_re = Regex::new(r"(?m)^export\s+(?:async\s+)?function\s+(GET|POST|PUT|DELETE|PATCH|HEAD|OPTIONS)\s*\(").unwrap();
    let mut any = false;
    for cap in method_re.captures_iter(text) {
        any = true;
        routes.push(Route { method: cap[1].to_string(), path: path.clone(), handler: cap[1].to_string(), file: rel.to_string() });
    }
    if !any {
        routes.push(Route { method: "ANY".to_string(), path, handler: "unknown".to_string(), file: rel.to_string() });
    }
}

fn extract_ts_events(rel: &str, text: &str, symbols: &mut Vec<Symbol>) {
    // EventEmitter2 / NestJS: emit('event.name') or @OnEvent('event.name')
    let emit_re = Regex::new(r#"(?:emit|@OnEvent)\s*\(\s*['"]([A-Za-z][A-Za-z0-9._-]*)['"]"#).unwrap();
    for cap in emit_re.captures_iter(text) {
        let raw = &cap[1];
        // Convert kebab-case and dot-notation to PascalCase for the name
        let name = raw
            .split(|c| c == '.' || c == '-' || c == '_')
            .map(|p| {
                let mut s = p.to_string();
                if let Some(c) = s.get_mut(0..1) {
                    c.make_ascii_uppercase();
                }
                s
            })
            .collect::<String>();
        symbols.push(Symbol {
            name,
            kind: SymbolKind::Event,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
        });
    }
}

// ── Python ────────────────────────────────────────────────────────────────────

fn extract_py(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    routes: &mut Vec<Route>,
) {
    // Classes
    let class_re = Regex::new(r"(?m)^class\s+([A-Za-z][A-Za-z0-9_]*)\s*[:(]").unwrap();
    for cap in class_re.captures_iter(text) {
        symbols.push(Symbol {
            name: cap[1].to_string(),
            kind: SymbolKind::Class,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
        });
    }

    // Top-level module functions (not class methods — those are indented
    // and excluded by the `^` anchor, same rationale as every other extractor
    // here: a class's private helpers would otherwise flood the symbol set).
    let toplevel_fn_re = Regex::new(r"(?m)^(?:async\s+)?def\s+([a-z_][A-Za-z0-9_]*)\s*\(").unwrap();
    for cap in toplevel_fn_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // Imports: from module import X, Y / import module
    let from_re = Regex::new(r"from\s+([\w.]+)\s+import\s+([^\n]+)").unwrap();
    for cap in from_re.captures_iter(text) {
        let to_module = cap[1].to_string();
        let names: Vec<String> = cap[2]
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != "*")
            .collect();
        imports.push(Import { from_file: rel.to_string(), to_module, names });
    }

    // FastAPI / Flask routes
    let route_re = Regex::new(
        r#"@(?:app|router|api_router)\.(get|post|put|delete|patch)\s*\(\s*["']([^"']+)["']"#
    ).unwrap();
    for cap in route_re.captures_iter(text) {
        let after = &text[cap.get(0).unwrap().end()..];
        let fn_re = Regex::new(r"(?m)^(?:async\s+)?def\s+(\w+)\s*\(").unwrap();
        let handler = fn_re
            .captures(after)
            .map(|c| c[1].to_string())
            .unwrap_or_else(|| "unknown".to_string());
        routes.push(Route {
            method: cap[1].to_string().to_uppercase(),
            path: cap[2].to_string(),
            handler,
            file: rel.to_string(),
        });
    }

    // Django urls.py: path('route/', views.some_view) / re_path(r'...', handler).
    // Django has no per-route HTTP method (that's dispatched inside the view),
    // so method is always "ANY" — an honest OBSERVED gap, not a guess.
    // `(?s)` lets the first string argument span multiple lines (Django route
    // patterns are routinely wrapped or split into adjacent string literals),
    // and the handler group stops at the first `(`, `,`, or `)` so a wrapped
    // call like `csrf_exempt(SomeView.as_view())` still yields a real,
    // if partial, observed token instead of nothing.
    let django_re = Regex::new(
        r#"(?s)\b(?:re_path|path)\s*\(\s*\(?\s*r?["']([^"']*)["'](?:\s*r?["'][^"']*["'])*\s*\)?\s*,\s*([A-Za-z_][A-Za-z0-9_.]*)"#,
    )
    .unwrap();
    for cap in django_re.captures_iter(text) {
        routes.push(Route {
            method: "ANY".to_string(),
            path: cap[1].to_string(),
            handler: cap[2].to_string(),
            file: rel.to_string(),
        });
    }
}

// ── Rust ──────────────────────────────────────────────────────────────────────

fn extract_rs(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    _routes: &mut Vec<Route>,
) {
    // pub struct / pub enum (only public — private types are implementation detail)
    let struct_re = Regex::new(r"(?m)^pub\s+(?:struct|enum)\s+([A-Z][A-Za-z0-9_]*)").unwrap();
    for cap in struct_re.captures_iter(text) {
        symbols.push(Symbol {
            name: cap[1].to_string(),
            kind: SymbolKind::Class,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
        });
    }

    // pub fn (top-level only, like the doc comment on SymbolKind::Function
    // requires — an indented `fn` inside `impl`/`mod` won't match `^pub`,
    // which is deliberate: every impl's methods would otherwise flood this).
    let fn_re = Regex::new(r"(?m)^pub\s+(?:async\s+)?fn\s+([a-z_][A-Za-z0-9_]*)").unwrap();
    for cap in fn_re.captures_iter(text) {
        symbols.push(Symbol {
            name: cap[1].to_string(),
            kind: SymbolKind::Function,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
        });
    }

    // pub trait
    let trait_re = Regex::new(r"(?m)^pub\s+trait\s+([A-Z][A-Za-z0-9_]*)").unwrap();
    for cap in trait_re.captures_iter(text) {
        symbols.push(Symbol {
            name: cap[1].to_string(),
            kind: SymbolKind::Interface,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
        });
    }

    // use statements: use crate::module::Type
    let use_re = Regex::new(r"use\s+([\w:]+)(?:::\{([^}]+)\})?;").unwrap();
    for cap in use_re.captures_iter(text) {
        let to_module = cap[1].to_string();
        let names: Vec<String> = cap
            .get(2)
            .map(|m| {
                m.as_str()
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        imports.push(Import { from_file: rel.to_string(), to_module, names });
    }
}

// ── Go ────────────────────────────────────────────────────────────────────────

fn extract_go(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    _routes: &mut Vec<Route>,
) {
    // type FooBar struct / type FooBar interface
    let type_re = Regex::new(r"(?m)^type\s+([A-Z][A-Za-z0-9_]*)\s+(struct|interface)\s*\{").unwrap();
    for cap in type_re.captures_iter(text) {
        let kind = if &cap[2] == "interface" { SymbolKind::Interface } else { SymbolKind::Class };
        symbols.push(Symbol {
            name: cap[1].to_string(),
            kind,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
        });
    }

    // Exported package-level functions and methods: func Foo(...) and
    // func (s *Server) Foo(...). Unexported (lowercase) functions are
    // implementation detail, same rule as the exported-only struct/interface
    // match above.
    let fn_re = Regex::new(r"(?m)^func\s+(?:\([^)]*\)\s+)?([A-Z][A-Za-z0-9_]*)\s*\(").unwrap();
    for cap in fn_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // import "package/path" or import ( "..." )
    let import_re = Regex::new(r#""([^"]+)""#).unwrap();
    // Only run in import blocks
    if let Some(import_block) = extract_go_import_block(text) {
        for cap in import_re.captures_iter(&import_block) {
            imports.push(Import {
                from_file: rel.to_string(),
                to_module: cap[1].to_string(),
                names: Vec::new(),
            });
        }
    }
}

fn extract_go_import_block(text: &str) -> Option<String> {
    let re = Regex::new(r"(?s)import\s*\(([^)]+)\)").unwrap();
    re.captures(text).map(|c| c[1].to_string())
}

// ── Java / Kotlin ─────────────────────────────────────────────────────────────

fn extract_java(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    routes: &mut Vec<Route>,
) {
    // public class / public interface
    let class_re = Regex::new(
        r"(?m)^(?:public\s+)?(?:abstract\s+)?(?:class|interface)\s+([A-Z][A-Za-z0-9_]*)"
    ).unwrap();
    for cap in class_re.captures_iter(text) {
        let matched = cap.get(0).unwrap().as_str();
        let kind = if matched.contains("interface") { SymbolKind::Interface } else { SymbolKind::Class };
        symbols.push(Symbol {
            name: cap[1].to_string(),
            kind,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
        });
    }

    // import statements
    let import_re = Regex::new(r"import\s+([\w.]+(?:\.\*)?);").unwrap();
    for cap in import_re.captures_iter(text) {
        imports.push(Import {
            from_file: rel.to_string(),
            to_module: cap[1].to_string(),
            names: Vec::new(),
        });
    }

    // Kotlin top-level functions: `fun foo(...)`. Java has no free functions
    // (methods live in the class already captured above), so this simply
    // never matches a .java file.
    let fun_re = Regex::new(r"(?m)^(?:public\s+)?fun\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap();
    for cap in fun_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // Spring MVC: @GetMapping("/path"), @RequestMapping(value="/path", method=GET)
    let mapping_re = Regex::new(
        r#"@(Get|Post|Put|Delete|Patch|Request)Mapping\s*(?:\([^)]*value\s*=\s*["']([^"']+)["']|["']([^"']+)["'])"#
    ).unwrap();
    for cap in mapping_re.captures_iter(text) {
        let method = match &cap[1] {
            "Get" => "GET",
            "Post" => "POST",
            "Put" => "PUT",
            "Delete" => "DELETE",
            "Patch" => "PATCH",
            _ => "ANY",
        };
        let path = cap.get(2).or(cap.get(3)).map(|m| m.as_str()).unwrap_or("/");
        routes.push(Route {
            method: method.to_string(),
            path: path.to_string(),
            handler: "spring-handler".to_string(),
            file: rel.to_string(),
        });
    }
}

// ── Ruby ──────────────────────────────────────────────────────────────────────

fn extract_rb(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    routes: &mut Vec<Route>,
) {
    // class Foo / class Foo < Bar
    let class_re = Regex::new(r"(?m)^class\s+([A-Z][A-Za-z0-9_:]*)").unwrap();
    for cap in class_re.captures_iter(text) {
        symbols.push(Symbol {
            name: cap[1].to_string(),
            kind: SymbolKind::Class,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
        });
    }

    // module Foo
    let mod_re = Regex::new(r"(?m)^module\s+([A-Z][A-Za-z0-9_:]*)").unwrap();
    for cap in mod_re.captures_iter(text) {
        symbols.push(Symbol {
            name: cap[1].to_string(),
            kind: SymbolKind::Interface,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
        });
    }

    // Methods: `def foo` / `def self.foo`. Unlike every other extractor here,
    // this intentionally does NOT require a top-level (column 0) anchor —
    // Ruby methods are conventionally indented inside class/module, so a
    // top-level-only rule would extract almost nothing. Trades a bit more
    // noise (private helpers) for actually finding real methods.
    let method_re = Regex::new(r"(?m)^\s*def\s+(?:self\.)?([a-z_][A-Za-z0-9_?!=]*)").unwrap();
    for cap in method_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // require / require_relative
    let require_re = Regex::new(r#"require(?:_relative)?\s+['"]([^'"]+)['"]"#).unwrap();
    for cap in require_re.captures_iter(text) {
        imports.push(Import {
            from_file: rel.to_string(),
            to_module: cap[1].to_string(),
            names: Vec::new(),
        });
    }

    // Rails routes.rb: get '/path', to: 'controller#action'
    let route_re = Regex::new(
        r#"(?m)^\s*(get|post|put|delete|patch|resources?)\s+['"]([^'"]+)['"]"#
    ).unwrap();
    for cap in route_re.captures_iter(text) {
        let method = cap[1].to_string().to_uppercase();
        routes.push(Route {
            method,
            path: cap[2].to_string(),
            handler: "rails-route".to_string(),
            file: rel.to_string(),
        });
    }
}

// ── Elixir ────────────────────────────────────────────────────────────────────

fn extract_ex(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    routes: &mut Vec<Route>,
) {
    // defmodule Foo.Bar
    let mod_re = Regex::new(r"defmodule\s+([A-Z][A-Za-z0-9._]*)").unwrap();
    for cap in mod_re.captures_iter(text) {
        symbols.push(Symbol {
            name: cap[1].to_string(),
            kind: SymbolKind::Class,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
        });
    }

    // Public functions: `def foo(...)`. Same reasoning as Ruby above — always
    // indented under `defmodule`, so no top-level anchor. `defp` (private) is
    // deliberately excluded: `def\s+` cannot match inside the word `defp`.
    let fn_re = Regex::new(r"(?m)^\s*def\s+([a-z_][A-Za-z0-9_?!]*)").unwrap();
    for cap in fn_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // alias / import / use
    let alias_re = Regex::new(r"(?:alias|import|use)\s+([A-Z][A-Za-z0-9._]*)").unwrap();
    for cap in alias_re.captures_iter(text) {
        imports.push(Import {
            from_file: rel.to_string(),
            to_module: cap[1].to_string(),
            names: Vec::new(),
        });
    }

    // Phoenix routes: get "/path", Controller, :action
    let route_re = Regex::new(
        r#"(?m)^\s*(get|post|put|delete|patch)\s+["']([^"']+)["']"#
    ).unwrap();
    for cap in route_re.captures_iter(text) {
        routes.push(Route {
            method: cap[1].to_string().to_uppercase(),
            path: cap[2].to_string(),
            handler: "phoenix-route".to_string(),
            file: rel.to_string(),
        });
    }
}

// ── PHP ───────────────────────────────────────────────────────────────────────

fn extract_php(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    _routes: &mut Vec<Route>,
) {
    // class Foo / interface Foo
    let class_re = Regex::new(
        r"(?m)^(?:abstract\s+)?(?:class|interface)\s+([A-Za-z][A-Za-z0-9_]*)"
    ).unwrap();
    for cap in class_re.captures_iter(text) {
        let matched = cap.get(0).unwrap().as_str();
        let kind = if matched.contains("interface") { SymbolKind::Interface } else { SymbolKind::Class };
        symbols.push(Symbol {
            name: cap[1].to_string(),
            kind,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
        });
    }

    // Top-level global functions (Laravel helpers, WordPress-style procedural
    // code). Class methods are indented and excluded by the `^` anchor.
    let fn_re = Regex::new(r"(?m)^function\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap();
    for cap in fn_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // use Foo\Bar\Baz;
    let use_re = Regex::new(r"use\s+([\w\\]+);").unwrap();
    for cap in use_re.captures_iter(text) {
        imports.push(Import {
            from_file: rel.to_string(),
            to_module: cap[1].to_string(),
            names: Vec::new(),
        });
    }
}

// ── C# ────────────────────────────────────────────────────────────────────────

fn extract_cs(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    routes: &mut Vec<Route>,
) {
    // public class Foo / public interface IFoo / public record Foo — allows
    // leading whitespace because brace-style `namespace Foo { ... }` indents
    // everything inside it one level, unlike the file-scoped `namespace Foo;`
    // style. Private/internal types are implementation detail, same rule as
    // every other extractor here.
    let type_re = Regex::new(
        r"(?m)^\s*public\s+(?:abstract\s+|sealed\s+|static\s+|partial\s+)*(class|interface|record)\s+([A-Za-z_][A-Za-z0-9_]*)"
    ).unwrap();
    for cap in type_re.captures_iter(text) {
        let kind = if &cap[1] == "interface" { SymbolKind::Interface } else { SymbolKind::Class };
        symbols.push(Symbol { name: cap[2].to_string(), kind, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // public methods (and constructors, which share the same shape minus a
    // return type match — accepted as a minor over-extraction, same tradeoff
    // regex-based extraction makes everywhere else in this file).
    let method_re = Regex::new(
        r"(?m)^\s*public\s+(?:static\s+|virtual\s+|override\s+|async\s+|sealed\s+)*[\w<>\[\],\.\?]+\s+([A-Z][A-Za-z0-9_]*)\s*\("
    ).unwrap();
    for cap in method_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // using Namespace.Sub;
    let using_re = Regex::new(r"(?m)^using\s+([\w.]+);").unwrap();
    for cap in using_re.captures_iter(text) {
        imports.push(Import { from_file: rel.to_string(), to_module: cap[1].to_string(), names: Vec::new() });
    }

    // ASP.NET Core: [HttpGet("path")], [HttpPost("path")], [Route("path")]
    let route_re = Regex::new(
        r#"\[Http(Get|Post|Put|Delete|Patch)(?:\s*\(\s*"([^"]*)"\s*\))?\]"#
    ).unwrap();
    for cap in route_re.captures_iter(text) {
        let after = &text[cap.get(0).unwrap().end()..];
        let handler_re = Regex::new(r"(?m)^\s*(?:public\s+)?(?:static\s+|async\s+)*[\w<>\[\],\.\?]+\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap();
        let handler = handler_re.captures(after).map(|c| c[1].to_string()).unwrap_or_else(|| "unknown".to_string());
        routes.push(Route {
            method: cap[1].to_string().to_uppercase(),
            path: cap.get(2).map(|m| m.as_str().to_string()).unwrap_or_default(),
            handler,
            file: rel.to_string(),
        });
    }
}

#[cfg(test)]
mod cs_tests {
    use super::*;

    #[test]
    fn extract_cs_finds_class_methods_and_routes() {
        let src = r#"
namespace MyApp.Auth;

public class AuthController
{
    [HttpPost("/login")]
    public async Task<IActionResult> Login(LoginRequest request)
    {
        return Ok();
    }

    public bool Authenticate(string user, string pass)
    {
        return true;
    }
}

public interface IAuthService
{
}
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_cs("Auth/AuthController.cs", src, &mut symbols, &mut imports, &mut routes);

        assert!(symbols.iter().any(|s| s.name == "AuthController" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "IAuthService" && s.kind == SymbolKind::Interface));
        assert!(symbols.iter().any(|s| s.name == "Authenticate" && s.kind == SymbolKind::Function));
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].method, "POST");
        assert_eq!(routes[0].path, "/login");
        assert_eq!(routes[0].handler, "Login");
    }
}

// ── Swift ─────────────────────────────────────────────────────────────────────

fn extract_swift(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    routes: &mut Vec<Route>,
) {
    // class / struct — top-level only (column 0, allowing access-modifier
    // prefixes). A method or nested type inside a class body is indented and
    // therefore excluded, the same top-level-only rule Rust/PHP use above to
    // keep a class's own members from flooding the symbol set.
    let class_re = Regex::new(
        r"(?m)^(?:public\s+|open\s+|internal\s+|final\s+)*class\s+([A-Z][A-Za-z0-9_]*)"
    ).unwrap();
    for cap in class_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    let struct_re = Regex::new(
        r"(?m)^(?:public\s+|internal\s+)*struct\s+([A-Z][A-Za-z0-9_]*)"
    ).unwrap();
    for cap in struct_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // protocol — Swift's interface equivalent.
    let protocol_re = Regex::new(r"(?m)^(?:public\s+)?protocol\s+([A-Z][A-Za-z0-9_]*)").unwrap();
    for cap in protocol_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Interface, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // func — top-level only, same noise tradeoff as everywhere else in this
    // file: a method inside a class/struct/protocol body is indented and
    // therefore excluded.
    let fn_re = Regex::new(
        r"(?m)^(?:public\s+|open\s+|internal\s+|private\s+|fileprivate\s+|static\s+|final\s+)*func\s+([A-Za-z_][A-Za-z0-9_]*)"
    ).unwrap();
    for cap in fn_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    let import_re = Regex::new(r"(?m)^import\s+([A-Za-z_][A-Za-z0-9_.]*)").unwrap();
    for cap in import_re.captures_iter(text) {
        imports.push(Import { from_file: rel.to_string(), to_module: cap[1].to_string(), names: Vec::new() });
    }

    // Vapor: app.get("path") { req in ... }, router.post("path", use: handler)
    let vapor_re = Regex::new(r#"\b(?:app|router|routes)\.(get|post|put|delete|patch)\s*\(\s*"([^"]*)""#).unwrap();
    for cap in vapor_re.captures_iter(text) {
        routes.push(Route {
            method: cap[1].to_string().to_uppercase(),
            path: format!("/{}", cap[2].trim_start_matches('/')),
            handler: "vapor-handler".to_string(),
            file: rel.to_string(),
        });
    }
}

#[cfg(test)]
mod swift_tests {
    use super::*;

    #[test]
    fn extract_swift_finds_types_and_top_level_func() {
        let src = r#"
import Foundation

public protocol Payable {
    func amount() -> Double
}

public struct Money {
    let cents: Int
}

public class Invoice {
    func total() -> Double {
        return 0.0
    }
}

func formatCurrency(_ value: Double) -> String {
    return "$\(value)"
}

app.get("invoices") { req in
    return "ok"
}
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_swift("Billing/Invoice.swift", src, &mut symbols, &mut imports, &mut routes);

        assert!(symbols.iter().any(|s| s.name == "Invoice" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "Money" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "Payable" && s.kind == SymbolKind::Interface));
        assert!(symbols.iter().any(|s| s.name == "formatCurrency" && s.kind == SymbolKind::Function));
        assert!(routes.iter().any(|r| r.method == "GET" && r.path == "/invoices"));
        assert!(
            !symbols.iter().any(|s| s.name == "total"),
            "indented method should not be extracted under the top-level-only rule"
        );
        assert!(imports.iter().any(|i| i.to_module == "Foundation"));
    }
}

// ── Objective-C ───────────────────────────────────────────────────────────────

fn extract_objc(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    _routes: &mut Vec<Route>,
) {
    // @interface Name / @implementation Name both name the same class — one
    // declares it, one defines it. dedup_by in extract_file collapses the
    // resulting duplicate Class symbol down to one.
    let iface_re = Regex::new(r"(?m)^@interface\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    for cap in iface_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }
    let impl_re = Regex::new(r"(?m)^@implementation\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    for cap in impl_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // @protocol Name — Objective-C's interface equivalent.
    let proto_re = Regex::new(r"(?m)^@protocol\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    for cap in proto_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Interface, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // - (ReturnType)methodName / + (ReturnType)methodName. Top-level only
    // (no leading whitespace) — Objective-C methods are conventionally
    // written flush-left even inside @implementation, so this finds real
    // methods without needing Ruby's indentation-tolerant rule.
    let method_re = Regex::new(r"(?m)^[-+]\s*\([^)]*\)\s*([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    for cap in method_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // #import "Header.h" / #import <Framework/Framework.h>
    let import_re = Regex::new(r#"#import\s+[<"]([^">]+)[">]"#).unwrap();
    for cap in import_re.captures_iter(text) {
        imports.push(Import { from_file: rel.to_string(), to_module: cap[1].to_string(), names: Vec::new() });
    }
}

#[cfg(test)]
mod objc_tests {
    use super::*;

    #[test]
    fn extract_objc_finds_interface_protocol_and_methods() {
        let src = r#"
#import <Foundation/Foundation.h>

@protocol PaymentDelegate
- (void)paymentDidComplete:(NSString *)transactionId;
@end

@interface PaymentProcessor : NSObject
- (BOOL)chargeAmount:(double)amount;
@end

@implementation PaymentProcessor

- (BOOL)chargeAmount:(double)amount {
    return YES;
}

+ (instancetype)sharedProcessor {
    return nil;
}

@end
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_objc("Payment/PaymentProcessor.m", src, &mut symbols, &mut imports, &mut routes);

        assert!(symbols.iter().any(|s| s.name == "PaymentProcessor" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "PaymentDelegate" && s.kind == SymbolKind::Interface));
        assert!(symbols.iter().any(|s| s.name == "chargeAmount" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "sharedProcessor" && s.kind == SymbolKind::Function));
        assert!(imports.iter().any(|i| i.to_module == "Foundation/Foundation.h"));
    }
}

// ── C / C++ ───────────────────────────────────────────────────────────────────

fn extract_c(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    _routes: &mut Vec<Route>,
) {
    // C has no classes at all, so `class` is only recognized for the C++
    // extensions — nothing to "downgrade" for .c/.h, the language simply
    // never had the concept. struct applies to both: a C struct is exactly
    // as real a type declaration as a C++ one.
    let is_cpp = rel
        .rsplit('.')
        .next()
        .map(|e| matches!(e, "cpp" | "hpp" | "cc" | "cxx"))
        .unwrap_or(false);

    let struct_re = Regex::new(r"(?m)^(?:typedef\s+)?struct\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    for cap in struct_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    if is_cpp {
        let class_re = Regex::new(r"(?m)^class\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();
        for cap in class_re.captures_iter(text) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
        }
    }

    // Top-level function DEFINITIONS only — a signature followed by a body
    // brace (same line or the next), never one ending in `;` (a prototype,
    // which declares but does not define and would flood every header).
    let fn_re = Regex::new(
        r"(?m)^[A-Za-z_][\w:<>,\.\*&\[\]\s]*[\s\*&]([A-Za-z_][A-Za-z0-9_]*)\s*\(([^;{}]*)\)\s*(?:const\s*)?\r?\n?\s*\{"
    ).unwrap();
    for cap in fn_re.captures_iter(text) {
        let name = cap[1].to_string();
        if matches!(name.as_str(), "if" | "for" | "while" | "switch" | "catch" | "return" | "sizeof") {
            continue;
        }
        symbols.push(Symbol { name, kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // #include "foo.h" / #include <foo.h>
    let include_re = Regex::new(r#"#include\s+[<"]([^">]+)[">]"#).unwrap();
    for cap in include_re.captures_iter(text) {
        imports.push(Import { from_file: rel.to_string(), to_module: cap[1].to_string(), names: Vec::new() });
    }
}

#[cfg(test)]
mod c_tests {
    use super::*;

    #[test]
    fn extract_c_finds_struct_and_function() {
        let src = r#"
#include <stdio.h>

struct Point {
    int x;
    int y;
};

int add(int a, int b) {
    return a + b;
}

int main(int argc, char *argv[]) {
    return 0;
}
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_c("util.c", src, &mut symbols, &mut imports, &mut routes);

        assert!(symbols.iter().any(|s| s.name == "Point" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "add" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "main" && s.kind == SymbolKind::Function));
        assert!(imports.iter().any(|i| i.to_module == "stdio.h"));
    }

    #[test]
    fn extract_c_recognizes_cpp_classes_only_for_cpp_extension() {
        let src = "class Widget {\npublic:\n    void render();\n};\n";
        let mut symbols_cpp = Vec::new();
        let mut symbols_c = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_c("Widget.cpp", src, &mut symbols_cpp, &mut imports, &mut routes);
        extract_c("Widget.c", src, &mut symbols_c, &mut imports, &mut routes);

        assert!(symbols_cpp.iter().any(|s| s.name == "Widget" && s.kind == SymbolKind::Class));
        assert!(
            !symbols_c.iter().any(|s| s.name == "Widget"),
            "a .c file has no classes — C++ class syntax must not leak into it"
        );
    }
}

// ── Scala ─────────────────────────────────────────────────────────────────────

fn extract_scala(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    _routes: &mut Vec<Route>,
) {
    let class_re = Regex::new(
        r"(?m)^(?:sealed\s+|abstract\s+|final\s+|case\s+)*class\s+([A-Z][A-Za-z0-9_]*)"
    ).unwrap();
    for cap in class_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // object — Scala's singleton; treated as a Class for impact purposes,
    // same call the TypeScript extractor makes for enums above.
    let object_re = Regex::new(r"(?m)^(?:case\s+)?object\s+([A-Z][A-Za-z0-9_]*)").unwrap();
    for cap in object_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    let trait_re = Regex::new(r"(?m)^(?:sealed\s+)?trait\s+([A-Z][A-Za-z0-9_]*)").unwrap();
    for cap in trait_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Interface, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // def — top-level only (a module-level def, e.g. in a `package object`
    // preamble). A method inside a class/object/trait body is indented and
    // therefore excluded — same top-level-only tradeoff Rust/PHP make.
    let def_re = Regex::new(
        r"(?m)^(?:private(?:\[\w+\])?\s+|protected\s+|final\s+|override\s+)*def\s+([a-zA-Z_][A-Za-z0-9_]*)"
    ).unwrap();
    for cap in def_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    let import_re = Regex::new(r"(?m)^import\s+([\w.{}, ]+)").unwrap();
    for cap in import_re.captures_iter(text) {
        imports.push(Import { from_file: rel.to_string(), to_module: cap[1].trim().to_string(), names: Vec::new() });
    }
}

#[cfg(test)]
mod scala_tests {
    use super::*;

    #[test]
    fn extract_scala_finds_class_object_trait_and_top_level_def() {
        let src = r#"
import scala.collection.mutable.ListBuffer

sealed trait Shape {
  def area: Double
}

final case class Circle(radius: Double) extends Shape {
  def area: Double = math.Pi * radius * radius
}

object ShapeFactory {
  def makeCircle(radius: Double): Circle = Circle(radius)
}

def describe(shape: Shape): String = shape.toString
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_scala("shapes/Shape.scala", src, &mut symbols, &mut imports, &mut routes);

        assert!(symbols.iter().any(|s| s.name == "Circle" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "ShapeFactory" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "Shape" && s.kind == SymbolKind::Interface));
        assert!(symbols.iter().any(|s| s.name == "describe" && s.kind == SymbolKind::Function));
        assert!(
            !symbols.iter().any(|s| s.name == "makeCircle"),
            "indented method should not be extracted under the top-level-only rule"
        );
        assert!(imports.iter().any(|i| i.to_module.contains("scala.collection.mutable.ListBuffer")));
    }
}

// ── Dart ──────────────────────────────────────────────────────────────────────

fn extract_dart(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    _routes: &mut Vec<Route>,
) {
    let class_re = Regex::new(r"(?m)^(?:abstract\s+)?class\s+([A-Z][A-Za-z0-9_]*)").unwrap();
    for cap in class_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // Top-level functions only — a signature ending in a body brace (never a
    // bare `;` abstract-method signature), never indented (a class method),
    // same top-level-only rule as everywhere else in this file.
    let fn_re = Regex::new(
        r"(?m)^[A-Za-z_][\w<>,\?\.]*(?:\s+[A-Za-z_][\w<>,\?\.]*)*\s+([a-zA-Z_][A-Za-z0-9_]*)\s*\(([^;{}]*)\)\s*(?:async\s*)?\r?\n?\s*\{"
    ).unwrap();
    for cap in fn_re.captures_iter(text) {
        let name = cap[1].to_string();
        if matches!(name.as_str(), "if" | "for" | "while" | "switch" | "catch") {
            continue;
        }
        symbols.push(Symbol { name, kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // import 'package:foo/foo.dart'; / import 'dart:core';
    let import_re = Regex::new(r#"import\s+['"]([^'"]+)['"]"#).unwrap();
    for cap in import_re.captures_iter(text) {
        imports.push(Import { from_file: rel.to_string(), to_module: cap[1].to_string(), names: Vec::new() });
    }
}

#[cfg(test)]
mod dart_tests {
    use super::*;

    #[test]
    fn extract_dart_finds_class_and_top_level_function() {
        let src = r#"
import 'package:flutter/material.dart';

class TodoItem {
  final String title;
  TodoItem(this.title);

  void toggle() {
    print('toggled');
  }
}

void main() {
  runApp(TodoItem('test'));
}
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_dart("lib/todo.dart", src, &mut symbols, &mut imports, &mut routes);

        assert!(symbols.iter().any(|s| s.name == "TodoItem" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "main" && s.kind == SymbolKind::Function));
        assert!(
            !symbols.iter().any(|s| s.name == "toggle"),
            "indented method should not be extracted under the top-level-only rule"
        );
        assert!(imports.iter().any(|i| i.to_module == "package:flutter/material.dart"));
    }
}

// ── Haskell ───────────────────────────────────────────────────────────────────

fn extract_haskell(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    routes: &mut Vec<Route>,
) {
    // data / newtype — Haskell has no classes in the OOP sense; a named
    // product/sum type declaration is its closest equivalent, so it's the
    // thing recorded as SymbolKind::Class here.
    let data_re = Regex::new(r"(?m)^(?:data|newtype)\s+([A-Z][A-Za-z0-9_']*)").unwrap();
    for cap in data_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // class — a typeclass, Haskell's interface equivalent: a contract types
    // opt into, not a value's own type.
    let class_re = Regex::new(r"(?m)^class\s+(?:.*=>\s*)?([A-Z][A-Za-z0-9_']*)").unwrap();
    for cap in class_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Interface, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // Top-level type signature lines: `name :: Type`. This is the most
    // reliable OBSERVED marker of a top-level binding in Haskell. The
    // corresponding `name arg1 arg2 = ...` equation line exists too but is
    // far noisier to tell apart from a pattern-match clause of the same
    // function, so the signature line is taken as the sole extraction.
    let sig_re = Regex::new(r"(?m)^([a-z_][A-Za-z0-9_']*)\s*::").unwrap();
    for cap in sig_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // import Module.Path
    let import_re = Regex::new(r"(?m)^import\s+(?:qualified\s+)?([A-Z][A-Za-z0-9_.]*)").unwrap();
    for cap in import_re.captures_iter(text) {
        imports.push(Import { from_file: rel.to_string(), to_module: cap[1].to_string(), names: Vec::new() });
    }

    // Yesod: routes declared in a `[parseRoutes| ... |]` quasi-quote block,
    // one per line: "/path NameR METHOD1 METHOD2" (no methods listed means
    // the handler responds to all of them). Servant's competing approach
    // (a type-level API DSL, e.g. `"users" :> Get '[JSON] [User]`) is NOT
    // attempted — combinators can nest and span multiple type declarations
    // in ways a regex can't reliably track, and a wrong route is worse than
    // a missing one.
    if let Some(block_start) = text.find("[parseRoutes|") {
        let body_start = block_start + "[parseRoutes|".len();
        if let Some(rel_end) = text[body_start..].find("|]") {
            let block = &text[body_start..body_start + rel_end];
            let line_re = Regex::new(r"(?m)^\s*(/\S*)\s+([A-Za-z][A-Za-z0-9_']*)(?:\s+(.*))?$").unwrap();
            for cap in line_re.captures_iter(block) {
                let path = cap[1].to_string();
                let name = cap[2].to_string();
                let methods: Vec<String> = cap
                    .get(3)
                    .map(|m| m.as_str().split_whitespace().map(|s| s.to_string()).collect())
                    .unwrap_or_default();
                let known: Vec<String> = methods
                    .into_iter()
                    .filter(|m| ["GET", "POST", "PUT", "DELETE", "PATCH"].contains(&m.as_str()))
                    .collect();
                if known.is_empty() {
                    routes.push(Route { method: "ANY".to_string(), path, handler: name, file: rel.to_string() });
                } else {
                    for m in known {
                        routes.push(Route { method: m, path: path.clone(), handler: name.clone(), file: rel.to_string() });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod haskell_tests {
    use super::*;

    #[test]
    fn extract_haskell_finds_data_class_and_top_level_signature() {
        let src = r#"
import qualified Data.List as List

data Shape = Circle Double | Rectangle Double Double

class Describable a where
  describe :: a -> String

area :: Shape -> Double
area (Circle r) = pi * r * r
area (Rectangle w h) = w * h

mkYesod "App" [parseRoutes|
/shapes ShapesR GET POST
/shapes/#ShapeId ShapeR GET
|]
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_haskell("Shapes.hs", src, &mut symbols, &mut imports, &mut routes);

        assert!(symbols.iter().any(|s| s.name == "Shape" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "Describable" && s.kind == SymbolKind::Interface));
        assert!(symbols.iter().any(|s| s.name == "area" && s.kind == SymbolKind::Function));
        assert!(imports.iter().any(|i| i.to_module == "Data.List"));
        assert!(routes.iter().any(|r| r.method == "GET" && r.path == "/shapes"));
        assert!(routes.iter().any(|r| r.method == "POST" && r.path == "/shapes"));
        assert!(routes.iter().any(|r| r.method == "GET" && r.path == "/shapes/#ShapeId"));
    }
}

// ── Clojure ───────────────────────────────────────────────────────────────────

fn extract_clojure(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    _imports: &mut Vec<Import>,
    routes: &mut Vec<Route>,
) {
    // defn — public function only. `defn-` (private) is naturally excluded:
    // `defn\s+` cannot match the `-` that immediately follows `defn` in
    // `defn-`, the same public-only convention as excluding Elixir's `defp`.
    let defn_re = Regex::new(r"\(defn\s+([A-Za-z][A-Za-z0-9_\-!?*+<>=]*)").unwrap();
    for cap in defn_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // defrecord / deftype — Clojure's closest equivalent to a class.
    let record_re = Regex::new(r"\((?:defrecord|deftype)\s+([A-Za-z][A-Za-z0-9_\-]*)").unwrap();
    for cap in record_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // defprotocol — Clojure's interface equivalent.
    let protocol_re = Regex::new(r"\(defprotocol\s+([A-Za-z][A-Za-z0-9_\-]*)").unwrap();
    for cap in protocol_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Interface, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // Compojure: (GET "/path" [] ...), (POST "/path" [] ...), etc.
    let route_re = Regex::new(r#"\((GET|POST|PUT|DELETE|PATCH|ANY|HEAD)\s+"([^"]+)""#).unwrap();
    for cap in route_re.captures_iter(text) {
        routes.push(Route {
            method: cap[1].to_string(),
            path: cap[2].to_string(),
            handler: "compojure-handler".to_string(),
            file: rel.to_string(),
        });
    }
}

#[cfg(test)]
mod clojure_tests {
    use super::*;

    #[test]
    fn extract_clojure_finds_public_defn_defrecord_and_defprotocol() {
        let src = r#"
(ns myapp.core)

(defprotocol Shape
  (area [this]))

(defrecord Circle [radius]
  Shape
  (area [this] (* Math/PI radius radius)))

(defn compute-area [shape]
  (area shape))

(defn- helper [x]
  (* x 2))

(defroutes app-routes
  (GET "/shapes" [] (list-shapes))
  (POST "/shapes" [] (create-shape)))
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_clojure("src/myapp/core.clj", src, &mut symbols, &mut imports, &mut routes);

        assert!(symbols.iter().any(|s| s.name == "compute-area" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "Circle" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "Shape" && s.kind == SymbolKind::Interface));
        assert!(
            !symbols.iter().any(|s| s.name == "helper"),
            "defn- is private and must not be extracted"
        );
        assert!(routes.iter().any(|r| r.method == "GET" && r.path == "/shapes"));
        assert!(routes.iter().any(|r| r.method == "POST" && r.path == "/shapes"));
    }
}

// ── GraphQL ───────────────────────────────────────────────────────────────────

fn extract_graphql(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    _imports: &mut Vec<Import>,
    routes: &mut Vec<Route>,
) {
    // type Foo { ... } / input Foo { ... } / enum Foo { ... }
    let type_re = Regex::new(r"(?m)^(?:type|input|enum)\s+([A-Z][A-Za-z0-9_]*)").unwrap();
    for cap in type_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    let interface_re = Regex::new(r"(?m)^interface\s+([A-Z][A-Za-z0-9_]*)").unwrap();
    for cap in interface_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Interface, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // Named operations: `query GetUser { ... }`, `mutation CreateUser { ... }`,
    // `subscription OnMessage { ... }` — reported as a Route (method =
    // operation type, path = operation name) so "does a GetUser query
    // already exist" is answerable the same way an HTTP route is.
    let op_re = Regex::new(r"(?m)^\s*(query|mutation|subscription)\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    for cap in op_re.captures_iter(text) {
        routes.push(Route {
            method: cap[1].to_string().to_uppercase(),
            path: cap[2].to_string(),
            handler: cap[2].to_string(),
            file: rel.to_string(),
        });
    }
}

#[cfg(test)]
mod graphql_tests {
    use super::*;

    #[test]
    fn extract_graphql_finds_types_and_operations() {
        let src = r#"
type User {
  id: ID!
  name: String!
}

interface Node {
  id: ID!
}

query GetUser {
  user(id: "1") {
    name
  }
}

mutation CreateUser {
  createUser(name: "x") {
    id
  }
}
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_graphql("schema.graphql", src, &mut symbols, &mut imports, &mut routes);

        assert!(symbols.iter().any(|s| s.name == "User" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "Node" && s.kind == SymbolKind::Interface));
        assert!(routes.iter().any(|r| r.method == "QUERY" && r.path == "GetUser"));
        assert!(routes.iter().any(|r| r.method == "MUTATION" && r.path == "CreateUser"));
    }

    #[test]
    fn extract_ts_js_finds_gql_tagged_template_operations() {
        let src = r#"
import { gql } from '@apollo/client';

export const GET_USER = gql`
  query GetUser($id: ID!) {
    user(id: $id) { name }
  }
`;
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_ts_js("src/queries.ts", src, &mut symbols, &mut imports, &mut routes);

        assert!(routes.iter().any(|r| r.method == "QUERY" && r.path == "GetUser"));
    }
}

// ── Protocol Buffers / gRPC ──────────────────────────────────────────────────

fn extract_proto(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    routes: &mut Vec<Route>,
) {
    let message_re = Regex::new(r"(?m)^message\s+([A-Z][A-Za-z0-9_]*)").unwrap();
    for cap in message_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) });
    }

    // Services and their rpc methods: rpc methods only belong to the
    // service they're textually inside, so each service's body is sliced
    // out (up to the next `service` declaration, or EOF) before its rpc
    // methods are matched — a flat file-wide rpc regex would silently
    // attribute every rpc everywhere to whichever service happened to be
    // named, which is wrong the moment a .proto file declares more than one.
    let service_re = Regex::new(r"(?m)^service\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{").unwrap();
    let services: Vec<(String, usize, usize)> = service_re
        .captures_iter(text)
        .map(|c| (c[1].to_string(), c.get(0).unwrap().start(), c.get(0).unwrap().end()))
        .collect();
    let rpc_re = Regex::new(r"rpc\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap();
    for (i, (name, start, body_start)) in services.iter().enumerate() {
        symbols.push(Symbol { name: name.clone(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, *start) });
        let body_end = services.get(i + 1).map(|(_, s, _)| *s).unwrap_or(text.len());
        for cap in rpc_re.captures_iter(&text[*body_start..body_end]) {
            routes.push(Route {
                method: "RPC".to_string(),
                path: format!("/{name}/{}", &cap[1]),
                handler: cap[1].to_string(),
                file: rel.to_string(),
            });
        }
    }

    let import_re = Regex::new(r#"import\s+"([^"]+)""#).unwrap();
    for cap in import_re.captures_iter(text) {
        imports.push(Import { from_file: rel.to_string(), to_module: cap[1].to_string(), names: Vec::new() });
    }
}

#[cfg(test)]
mod proto_tests {
    use super::*;

    #[test]
    fn extract_proto_finds_messages_and_scopes_rpc_to_its_own_service() {
        let src = r#"
syntax = "proto3";

import "google/protobuf/empty.proto";

message User {
  string id = 1;
  string name = 2;
}

service UserService {
  rpc GetUser(GetUserRequest) returns (User);
  rpc CreateUser(CreateUserRequest) returns (User);
}

service AdminService {
  rpc DeleteUser(DeleteUserRequest) returns (google.protobuf.Empty);
}
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_proto("user.proto", src, &mut symbols, &mut imports, &mut routes);

        assert!(symbols.iter().any(|s| s.name == "User" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "UserService" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "AdminService" && s.kind == SymbolKind::Class));
        assert!(routes.iter().any(|r| r.path == "/UserService/GetUser"));
        assert!(routes.iter().any(|r| r.path == "/UserService/CreateUser"));
        assert!(routes.iter().any(|r| r.path == "/AdminService/DeleteUser"));
        assert!(
            !routes.iter().any(|r| r.path == "/AdminService/GetUser"),
            "rpc method leaked across service boundaries"
        );
        assert!(imports.iter().any(|i| i.to_module == "google/protobuf/empty.proto"));
    }
}
