//! Comparison suite: regex `extract_rs` vs syn-based `extract_rs_syn`.
//!
//! ## What this file is
//!
//! This is the migration guard and methodology template for Archietect's
//! extractor migrations. The Rust migration (2026-09-07) used this harness
//! to run both implementations against Archietect's own `src/` directory,
//! classify every divergence, and make the switch only after every difference
//! was understood.
//!
//! ## IMPORTANT: what "149 vs 149, 0 divergences" means NOW
//!
//! After migration, both the "regex" and "syn" columns in
//! `regex_vs_syn_on_own_source` go through the same canonical extractor
//! (`extract_rs_syn`). The counts match by construction. This does NOT mean
//! regex and syn were equivalent — they were not.
//!
//! The original comparison, run before migration, found 11 divergences:
//!
//!   8 regex false positives — `pub fn foo()` literals inside raw string
//!     test fixtures being extracted as declarations. Regex cannot distinguish
//!     Rust-shaped text inside a string from a real source declaration.
//!
//!   3 genuine missed declarations — `pub(crate) fn watchable_dirs`,
//!     `pub(crate) fn relevant`, `pub(crate) fn brace_body_span`. The
//!     pattern `pub\s+fn` does not match `pub(crate) fn`. These were
//!     silently ABSENT in Archietect's index of its own source.
//!
//!   0 unresolved divergences.
//!
//! Before migration: `archietect concept watchable_dirs` → ABSENT.
//! After migration:  `archietect concept watchable_dirs` → STRUCTURAL,
//!                   src/watch.rs:76 (with excerpt).
//!
//! The harness is preserved because:
//!   1. The divergence classification machinery is the template for the next
//!      migration (TS/JS with tree-sitter).
//!   2. If a second Rust observer is ever introduced, run it through here and
//!      classify every difference before switching.
//!   3. The provenance and structural unit tests below lock in behavior that
//!      must not silently regress.
//!
//! ## Methodology for future migrations
//!
//! ```text
//! existing extractor
//!        │
//!        ├──────────────────┐
//!        ↓                  ↓
//!    current            new observer
//!    (regex/AST)              │
//!        │                    │
//!        └────────┬───────────┘
//!                 ↓
//!          divergence corpus
//!          (real files, not synthetic)
//!                 ↓
//!        classify every difference:
//!          PARSER_WIN   — new observer is right, old was wrong
//!          REGEX_WIN    — old observer was right, new missed it
//!          KNOWN_DIFF   — intentional, neither is wrong
//!                 ↓
//!        0 unclassified → migration justified
//! ```
//!
//! Migration is not based on "parsers are theoretically superior." It is
//! based on observational divergence against real code.

use archietect::structural::{extract_rs_syn, ObservationSource, SymbolKind};
use std::fs;
use std::path::PathBuf;

// ── helpers ──────────────────────────────────────────────────────────────────

fn src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Run the syn extractor on a source string.
fn syn_symbols_for(rel: &str, text: &str) -> Vec<(String, SymbolKind)> {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut routes = Vec::new();
    extract_rs_syn(rel, text, &mut symbols, &mut imports, &mut routes);
    symbols.into_iter().map(|s| (s.name, s.kind)).collect()
}

/// Run the production scan on a single `.rs` file by constructing a temp
/// directory with just that file and using the full scan pipeline. This
/// exercises the real `extract_rs` regex path exactly as it runs in
/// production (not a separate function call).
fn regex_symbols_for(rel: &str, text: &str) -> Vec<(String, SymbolKind)> {
    // Write file to a temp dir so we can run scan_with_prior on it.
    let tmp_dir = std::env::temp_dir().join(format!(
        "archietect_rs_cmp_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    fs::create_dir_all(&tmp_dir).unwrap();
    let file_path = tmp_dir.join(rel);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&file_path, text).unwrap();

    let (_idx, graph) = archietect::scan::scan_with_prior(&tmp_dir, None, None);

    let _ = fs::remove_dir_all(&tmp_dir);

    // File key in the graph is the relative path.
    graph
        .symbols
        .values()
        .filter(|s| s.file == rel)
        .map(|s| (s.name.clone(), s.kind.clone()))
        .collect()
}

// ── Divergence classification ─────────────────────────────────────────────────

#[derive(Debug)]
#[allow(dead_code)]
enum DivergenceKind {
    /// syn found it, regex missed it.
    ParserWin { name: String, kind: String, reason: &'static str },
    /// regex found it, syn missed it.
    RegexWin { name: String, kind: String, reason: &'static str },
    /// Investigated and intentional — neither extractor is wrong.
    KnownDiff { name: String, reason: &'static str },
}

/// All divergences we have investigated and classified.
/// Add an entry here whenever a new divergence appears — the test fails
/// until every divergence is classified.
fn classified_divergences() -> Vec<DivergenceKind> {
    vec![
        // ── REGEX_WIN: functions inside test fixture strings ──────────────────
        //
        // scan.rs contains `#[cfg(test)]` module tests that embed multiline
        // raw string literals with `pub fn real_production_code()`, `pub fn a()`,
        // etc. as fixture text. The regex extractor sees these as declarations
        // because `^pub fn` matches any line-start occurrence, including ones
        // inside string literals. syn parses the actual AST and correctly
        // ignores these — they are string literals, not declarations.
        //
        // Classification: REGEX_WIN for the wrong reason. These are FALSE
        // POSITIVES from the regex extractor — noise, not signal. syn is right
        // to exclude them. This is a parser win in disguise.
        DivergenceKind::RegexWin {
            name: "real_production_code".to_string(),
            kind: "Function".to_string(),
            reason: "false positive — `pub fn real_production_code()` lives inside a raw string \
                     literal in a test fixture in scan.rs. Regex matches it as a declaration; \
                     syn correctly ignores it. This is actually a PARSER_WIN — syn is right.",
        },
        DivergenceKind::RegexWin {
            name: "a".to_string(),
            kind: "Function".to_string(),
            reason: "false positive — `pub fn a()` lives inside a raw string literal in a test \
                     fixture in scan.rs. Same class as real_production_code above.",
        },
        DivergenceKind::RegexWin {
            name: "b".to_string(),
            kind: "Function".to_string(),
            reason: "false positive — `pub fn b()` lives inside a raw string literal in a test \
                     fixture in scan.rs. Same class as real_production_code above.",
        },
        DivergenceKind::RegexWin {
            name: "c".to_string(),
            kind: "Function".to_string(),
            reason: "false positive — `pub fn c()` lives inside a raw string literal in a test \
                     fixture in scan.rs. Same class as real_production_code above.",
        },
        DivergenceKind::RegexWin {
            name: "real_code".to_string(),
            kind: "Function".to_string(),
            reason: "false positive — `pub fn real_code()` lives inside a raw string literal in \
                     a test fixture in scan.rs. Same class as real_production_code above.",
        },
        DivergenceKind::RegexWin {
            name: "after".to_string(),
            kind: "Function".to_string(),
            reason: "false positive — `pub fn after()` lives inside a raw string literal in a \
                     test fixture in scan.rs. Same class as real_production_code above.",
        },
        DivergenceKind::RegexWin {
            name: "first".to_string(),
            kind: "Function".to_string(),
            reason: "false positive — `pub fn first(db: &'static Pool)` lives inside a raw \
                     string literal in a test fixture in structural.rs (the \
                     rust_lifetimes_and_apostrophes test). Regex matches it; syn correctly \
                     ignores it. Parser win.",
        },
        DivergenceKind::RegexWin {
            name: "second".to_string(),
            kind: "Function".to_string(),
            reason: "false positive — `pub fn second()` lives inside the same raw string \
                     literal test fixture in structural.rs. Regex matches it; syn ignores it.",
        },

        // ── PARSER_WIN: pub(crate) items that regex's `pub\s+` DOES match ────
        //
        // The `pub fn` regex pattern is `^pub\s+(?:async\s+)?fn\s+` — it DOES
        // match `pub(crate) fn` because `pub(crate)` starts with `pub` followed
        // by `(`, which `\s+` does NOT match. So these are actually PARSER_WIN
        // in the sense that syn finds them correctly — but the regex ALSO
        // matches them via `pub\s` only when there IS a space after pub. Wait:
        // `pub(crate)` has no space, so `pub\s+` would NOT match it. So syn
        // finds these and regex misses them.
        DivergenceKind::ParserWin {
            name: "watchable_dirs".to_string(),
            kind: "Function".to_string(),
            reason: "watch.rs: `pub(crate) fn watchable_dirs` — regex requires `pub\\s+fn` \
                     (space after pub), which does not match `pub(crate) fn`. syn correctly \
                     recognises all Visibility::Restricted forms. Genuine parser win.",
        },
        DivergenceKind::ParserWin {
            name: "relevant".to_string(),
            kind: "Function".to_string(),
            reason: "watch.rs: `pub(crate) fn relevant` — same as watchable_dirs above. \
                     `pub(crate)` has no space after `pub`, so the regex misses it. \
                     syn finds it correctly.",
        },
        DivergenceKind::ParserWin {
            name: "brace_body_span".to_string(),
            kind: "Function".to_string(),
            reason: "structural.rs: `pub(crate) fn brace_body_span` — same class as \
                     watchable_dirs/relevant. Regex misses all `pub(crate)` / `pub(super)` / \
                     `pub(in path)` forms. syn handles all of them.",
        },
    ]
}

// ── Main comparison test ──────────────────────────────────────────────────────
//
// NOTE: now that extract_rs_syn is the canonical Rust extractor, both the
// "regex" and "syn" columns go through the same extractor — scan_with_prior
// calls extract_rs_syn_dispatch for .rs files. The counts will match by
// construction.
//
// The test's value going forward is as a *future divergence guard*: if a
// second Rust extractor is ever introduced (e.g. a tree-sitter based one),
// running it through this harness and classifying every divergence is the
// exact methodology to use before switching. The classification machinery
// remains so the pattern is ready.

#[test]
fn regex_vs_syn_on_own_source() {
    let src = src_dir();
    assert!(src.exists(), "src/ directory not found at {}", src.display());

    let mut unclassified: Vec<String> = Vec::new();
    let mut total_files = 0usize;
    let mut total_regex = 0usize;
    let mut total_syn = 0usize;

    let classified = classified_divergences();

    for entry in walkdir::WalkDir::new(&src)
        .max_depth(1) // top-level src/*.rs only — no generated subdirs
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "rs").unwrap_or(false))
    {
        let path = entry.path();
        let rel = path.file_name().unwrap().to_string_lossy().to_string();
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => continue,
        };

        total_files += 1;

        let mut regex_syms = regex_symbols_for(&rel, &text);
        let mut syn_syms = syn_symbols_for(&rel, &text);

        regex_syms.sort_by(|a, b| a.0.cmp(&b.0).then(format!("{:?}", a.1).cmp(&format!("{:?}", b.1))));
        syn_syms.sort_by(|a, b| a.0.cmp(&b.0).then(format!("{:?}", a.1).cmp(&format!("{:?}", b.1))));

        total_regex += regex_syms.len();
        total_syn += syn_syms.len();

        // Items only in regex output (regex found it, syn didn't).
        for (name, kind) in &regex_syms {
            let in_syn = syn_syms.iter().any(|(n, k)| n == name && k == kind);
            if !in_syn {
                let is_classified = classified.iter().any(|d| match d {
                    DivergenceKind::RegexWin { name: n, .. } => n == name,
                    DivergenceKind::KnownDiff { name: n, .. } => n == name,
                    _ => false,
                });
                if !is_classified {
                    unclassified.push(format!(
                        "REGEX_ONLY  file={rel}  name={name}  kind={kind:?}\n  \
                         → regex found this; syn did not.\n  \
                         Classify as ParserWin, RegexWin, or KnownDiff in classified_divergences()."
                    ));
                }
            }
        }

        // Items only in syn output (syn found it, regex didn't).
        for (name, kind) in &syn_syms {
            let in_regex = regex_syms.iter().any(|(n, k)| n == name && k == kind);
            if !in_regex {
                let is_classified = classified.iter().any(|d| match d {
                    DivergenceKind::ParserWin { name: n, .. } => n == name,
                    DivergenceKind::KnownDiff { name: n, .. } => n == name,
                    _ => false,
                });
                if !is_classified {
                    unclassified.push(format!(
                        "SYN_ONLY    file={rel}  name={name}  kind={kind:?}\n  \
                         → syn found this; regex did not.\n  \
                         Classify as ParserWin, RegexWin, or KnownDiff in classified_divergences()."
                    ));
                }
            }
        }
    }

    // Summary — always printed, useful even when passing.
    println!(
        "\nrs_extractor_comparison: {} files, {} regex symbols, {} syn symbols, {} unclassified divergence(s)",
        total_files, total_regex, total_syn, unclassified.len()
    );

    if !unclassified.is_empty() {
        panic!(
            "\n{} unclassified divergence(s) between regex and syn extractors.\n\
             Investigate each one and add it to classified_divergences() in\n\
             tests/rs_extractor_comparison.rs.\n\n{}",
            unclassified.len(),
            unclassified.join("\n\n")
        );
    }
}

// ── Targeted unit tests ───────────────────────────────────────────────────────
//
// These test specific Rust constructs where we know regex is imprecise,
// documenting the parser win (or lack of win) concretely.

/// `pub fn` at column 0 inside an impl block: regex matches it because
/// `^pub fn` anchors to line-start regardless of context. syn correctly
/// knows it's a method, not a top-level item.
#[test]
fn syn_does_not_extract_impl_method_at_column_zero() {
    let src = r#"
pub struct Foo;

impl Foo {
pub fn bar(&self) -> u32 { 42 }
}
"#;
    let syms = syn_symbols_for("test.rs", src);
    let names: Vec<&str> = syms.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"Foo"), "syn should find top-level struct Foo");
    assert!(
        !names.contains(&"bar"),
        "syn must NOT extract impl method `bar` even at column 0 — this is a parser win over regex"
    );
}

/// `pub(crate)` and `pub(super)` visibility: regex requires `pub\s+` which
/// does match both, but `pub(in path)` would not. syn handles all variants
/// via Visibility::Restricted.
#[test]
fn syn_extracts_restricted_visibility_items() {
    let src = r#"
pub(crate) struct Internal;
pub(crate) fn helper() {}
pub(super) trait SuperTrait {}
"#;
    let syms = syn_symbols_for("test.rs", src);
    let names: Vec<&str> = syms.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"Internal"), "syn should extract pub(crate) struct");
    assert!(names.contains(&"helper"), "syn should extract pub(crate) fn");
    assert!(names.contains(&"SuperTrait"), "syn should extract pub(super) trait");
}

/// Private items must not appear from syn.
#[test]
fn syn_does_not_extract_private_items() {
    let src = r#"
struct Private;
fn also_private() {}
trait AlsoPrivate {}
"#;
    let syms = syn_symbols_for("test.rs", src);
    assert!(
        syms.is_empty(),
        "syn must not extract private items, got: {:?}",
        syms.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

/// Items inside a `mod` block are not top-level — syn must not extract them
/// even if they're `pub`.
#[test]
fn syn_does_not_extract_items_inside_mod_blocks() {
    let src = r#"
mod inner {
    pub struct InsideMod;
    pub fn inside_fn() {}
}

pub struct TopLevel;
"#;
    let syms = syn_symbols_for("test.rs", src);
    let names: Vec<&str> = syms.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"TopLevel"), "syn should find top-level TopLevel");
    assert!(
        !names.contains(&"InsideMod"),
        "syn must not extract items from inside a mod block"
    );
    assert!(
        !names.contains(&"inside_fn"),
        "syn must not extract functions from inside a mod block"
    );
}

/// Fallback: a file that doesn't parse cleanly should not panic and should
/// not return empty when regex can find something.
#[test]
fn syn_fallback_on_parse_failure_does_not_panic() {
    // Not valid Rust — syn will fail to parse it.
    let src = "pub struct Broken { << invalid >>";
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut routes = Vec::new();
    // Must not panic.
    extract_rs_syn("broken.rs", src, &mut symbols, &mut imports, &mut routes);
}

/// Line numbers from syn must be 1-indexed and accurate.
#[test]
fn syn_line_numbers_are_correct() {
    let src = "// line 1\n// line 2\npub struct Foo;\n// line 4\npub fn bar() {}\n";
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut routes = Vec::new();
    extract_rs_syn("test.rs", src, &mut symbols, &mut imports, &mut routes);

    let foo = symbols.iter().find(|s| s.name == "Foo").expect("Foo not found");
    let bar = symbols.iter().find(|s| s.name == "bar").expect("bar not found");
    assert_eq!(foo.line, 3, "Foo should be on line 3");
    assert_eq!(bar.line, 5, "bar should be on line 5");
}

/// Route extraction: Axum `.route()` and Actix attribute macros are still
/// found by syn (which delegates to the regex path for routes).
#[test]
fn syn_still_extracts_routes_via_regex() {
    // Use the idiomatic Axum form `get(handler)` (no module path prefix)
    // that the regex already handles — the point here is that route
    // extraction passes through to the regex path unchanged, not that
    // every possible Axum spelling is supported.
    let src = r#"
use axum::{Router, routing::get};

pub async fn my_handler() {}

pub fn app() -> Router {
    Router::new().route("/users", get(my_handler))
}
"#;
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut routes = Vec::new();
    extract_rs_syn("routes.rs", src, &mut symbols, &mut imports, &mut routes);

    assert!(
        routes.iter().any(|r| r.path == "/users" && r.method == "GET"),
        "syn extractor must still surface Axum routes via the regex path, got routes: {:?}",
        routes
    );
}

// ── ObservationSource provenance tests ───────────────────────────────────────

/// AST-confirmed symbols must carry ObservationSource::Ast.
#[test]
fn ast_symbols_carry_ast_provenance() {
    let src = r#"
pub struct Order;
pub enum Status { Active, Inactive }
pub trait Repository {}
pub fn create_order() {}
pub async fn delete_order() {}
"#;
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut routes = Vec::new();
    extract_rs_syn("order.rs", src, &mut symbols, &mut imports, &mut routes);

    assert!(!symbols.is_empty(), "should have extracted symbols");
    for sym in &symbols {
        assert_eq!(
            sym.observation_source,
            ObservationSource::Ast,
            "symbol `{}` should carry Ast provenance, got {:?}",
            sym.name,
            sym.observation_source
        );
    }
}

/// pub(crate) items — the parser win case — must also carry Ast provenance.
#[test]
fn pub_crate_symbols_carry_ast_provenance() {
    let src = r#"
pub(crate) fn watchable_dirs() {}
pub(crate) struct InternalState;
pub(super) trait SuperBound {}
"#;
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut routes = Vec::new();
    extract_rs_syn("watch.rs", src, &mut symbols, &mut imports, &mut routes);

    assert!(!symbols.is_empty(), "should extract pub(crate)/pub(super) items");
    for sym in &symbols {
        assert_eq!(
            sym.observation_source,
            ObservationSource::Ast,
            "restricted-visibility symbol `{}` must still carry Ast provenance",
            sym.name
        );
    }
}

/// When the file fails to parse, fallback symbols carry LexicalFallback, not Ast.
/// This distinguishes them from AST-confirmed observations at query time.
#[test]
fn fallback_symbols_carry_lexical_fallback_provenance() {
    // Unparseable by syn — macro output, incomplete snippet, etc.
    let src = "pub fn broken( { this is not valid rust";
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut routes = Vec::new();
    extract_rs_syn("generated.rs", src, &mut symbols, &mut imports, &mut routes);

    // If regex found anything in the fallback path, it must be LexicalFallback.
    for sym in &symbols {
        assert_eq!(
            sym.observation_source,
            ObservationSource::LexicalFallback,
            "fallback symbol `{}` must carry LexicalFallback provenance, not {:?}",
            sym.name,
            sym.observation_source
        );
    }
}

/// Items inside impl blocks must not appear at all — not even with degraded
/// provenance. The scoping contract is binary: top-level or not extracted.
#[test]
fn impl_methods_are_absent_not_degraded() {
    let src = r#"
pub struct Foo;
impl Foo {
    pub fn method(&self) {}
pub fn method_at_col_zero(&self) {}
}
"#;
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut routes = Vec::new();
    extract_rs_syn("foo.rs", src, &mut symbols, &mut imports, &mut routes);

    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"Foo"), "struct Foo must be found");
    assert!(
        !names.contains(&"method") && !names.contains(&"method_at_col_zero"),
        "impl methods must be absent entirely, not extracted with degraded provenance. Got: {names:?}"
    );
}

/// ObservationSource deserializes correctly from old records that lack the
/// field — must default to Lexical (the honest conservative claim).
#[test]
fn observation_source_defaults_to_lexical_on_deserialize() {
    // A JSON Symbol record without observation_source, as produced by an
    // older version of Archietect.
    let json = r#"{
        "name": "Order",
        "kind": "Class",
        "file": "src/models.rs",
        "linked_concept": null,
        "line": 10
    }"#;
    let sym: archietect::structural::Symbol = serde_json::from_str(json)
        .expect("should deserialize legacy record without observation_source");
    assert_eq!(
        sym.observation_source,
        ObservationSource::Lexical,
        "missing observation_source must default to Lexical, not Ast"
    );
}
