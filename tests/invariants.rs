//! Invariant regression suite — runs the architectural invariants against
//! every validation corpus repo.
//!
//! ## Why this is separate from laws.rs
//!
//! laws.rs tests PER-LAW fixtures: tiny synthetic repos that reproduce one
//! specific past wrong answer. Those are counterexample records.
//!
//! This file tests CROSS-CUTTING PROPERTIES against real repos: does any
//! concept in the full chatwoot scan have two storage identities? Does any
//! alias in lobe-chat point at nothing? Those questions can't be asked of a
//! six-line fixture — they need the full corpus.
//!
//! Together the two suites give different guarantees:
//!   laws.rs        — "the specific bug that taught us this rule cannot recur"
//!   invariants.rs  — "the class of bug this invariant defines cannot occur
//!                     in any scanned corpus, not just the one we've seen"

use architect::{invariants, scan};
use std::path::PathBuf;

fn corpus_root(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("validation").join(name)
}

/// Run invariant checks on a pre-built corpus repo.
/// Uses scan_with_prior so the pre-built architect.db is the source of truth
/// (same as a real `architect concept` call on that repo).
fn check_corpus(name: &str) -> Vec<invariants::Violation> {
    let root = corpus_root(name);
    assert!(root.exists(), "corpus repo missing: {}", root.display());
    let (schema_prior, graph_prior) = architect::store::load_raw(&root);
    let (idx, _graph) = scan::scan_with_prior(&root, schema_prior, graph_prior);
    invariants::check(&idx)
}

// ── corpus repos ─────────────────────────────────────────────────────────────

#[test]
fn invariants_chatwoot() {
    let v = check_corpus("chatwoot");
    assert!(
        v.is_empty(),
        "chatwoot violated {} invariant(s):\n{}",
        v.len(),
        fmt(&v)
    );
}

#[test]
fn invariants_lobe_chat() {
    let v = check_corpus("lobe-chat");
    assert!(
        v.is_empty(),
        "lobe-chat violated {} invariant(s):\n{}",
        v.len(),
        fmt(&v)
    );
}

#[test]
fn invariants_umami() {
    let v = check_corpus("umami");
    assert!(
        v.is_empty(),
        "umami violated {} invariant(s):\n{}",
        v.len(),
        fmt(&v)
    );
}

#[test]
fn invariants_saleor() {
    let v = check_corpus("saleor");
    assert!(
        v.is_empty(),
        "saleor violated {} invariant(s):\n{}",
        v.len(),
        fmt(&v)
    );
}

#[test]
fn invariants_bookstack() {
    let v = check_corpus("BookStack");
    assert!(
        v.is_empty(),
        "BookStack violated {} invariant(s):\n{}",
        v.len(),
        fmt(&v)
    );
}

#[test]
fn invariants_dub() {
    let v = check_corpus("dub");
    assert!(
        v.is_empty(),
        "dub violated {} invariant(s):\n{}",
        v.len(),
        fmt(&v)
    );
}

#[test]
fn invariants_analytics() {
    let v = check_corpus("analytics");
    assert!(
        v.is_empty(),
        "analytics violated {} invariant(s):\n{}",
        v.len(),
        fmt(&v)
    );
}

#[test]
fn invariants_redash() {
    let v = check_corpus("redash");
    assert!(
        v.is_empty(),
        "redash violated {} invariant(s):\n{}",
        v.len(),
        fmt(&v)
    );
}

/// The C# extractor has no schema-layer ORM to check invariants against (C#
/// isn't in the schema-extractor list, only the structural one) — so instead
/// of `check_corpus`, this asserts the structural graph itself is right
/// against a real, live ASP.NET Core repo, not just the synthetic snippet in
/// `structural::cs_tests`.
#[test]
fn structural_aspnetcore_realworld() {
    let root = corpus_root("aspnetcore-realworld-example-app");
    assert!(root.exists(), "corpus repo missing: {}", root.display());
    let (schema_prior, graph_prior) = architect::store::load_raw(&root);
    let (_idx, graph) = scan::scan_with_prior(&root, schema_prior, graph_prior);
    assert!(
        graph.symbols.values().any(|s| s.name == "Article"
            && matches!(s.kind, architect::structural::SymbolKind::Class)),
        "C# extractor failed to find the real Article domain class in a live ASP.NET Core repo"
    );
    assert!(
        !graph.routes.is_empty(),
        "C# extractor found zero ASP.NET Core routes in a repo that has real [Http*] attributes"
    );
}

/// The C extractor against a real, well-known single-file C program
/// (antirez/kilo, a ~1500-line terminal text editor) — same rationale as
/// `structural_aspnetcore_realworld`: a synthetic snippet alone proved
/// insufficient once before (Kotlin), so every new extractor gets one live
/// corpus check that exercises the real scan pipeline, not just its own unit
/// test.
#[test]
fn structural_kilo_realworld() {
    let root = corpus_root("kilo");
    assert!(root.exists(), "corpus repo missing: {}", root.display());
    let (schema_prior, graph_prior) = architect::store::load_raw(&root);
    let (_idx, graph) = scan::scan_with_prior(&root, schema_prior, graph_prior);
    assert!(
        graph.symbols.values().any(|s| s.name == "editorConfig"
            && matches!(s.kind, architect::structural::SymbolKind::Class)),
        "C extractor failed to find the real editorConfig struct in kilo.c"
    );
    assert!(
        graph.symbols.values().any(|s| s.name == "disableRawMode"
            && matches!(s.kind, architect::structural::SymbolKind::Function)),
        "C extractor failed to find the real disableRawMode function in kilo.c"
    );
}

/// The Dart extractor against a real Dart package (dart-lang/args, the
/// canonical command-line argument parser used across the Dart ecosystem).
#[test]
fn structural_dart_args_realworld() {
    let root = corpus_root("args");
    assert!(root.exists(), "corpus repo missing: {}", root.display());
    let (schema_prior, graph_prior) = architect::store::load_raw(&root);
    let (_idx, graph) = scan::scan_with_prior(&root, schema_prior, graph_prior);
    assert!(
        graph.symbols.values().any(|s| s.name == "CommandRunner"
            && matches!(s.kind, architect::structural::SymbolKind::Class)),
        "Dart extractor failed to find the real CommandRunner class in lib/command_runner.dart"
    );
    assert!(
        graph.symbols.values().any(|s| s.name == "ArgParser"
            && matches!(s.kind, architect::structural::SymbolKind::Class)),
        "Dart extractor failed to find the real ArgParser class"
    );
}

/// The Scala extractor against a real Scala library (scala/scala-xml, the
/// official XML library shipped alongside the Scala standard library).
#[test]
fn structural_scala_xml_realworld() {
    let root = corpus_root("scala-xml");
    assert!(root.exists(), "corpus repo missing: {}", root.display());
    let (schema_prior, graph_prior) = architect::store::load_raw(&root);
    let (_idx, graph) = scan::scan_with_prior(&root, schema_prior, graph_prior);
    assert!(
        graph.symbols.values().any(|s| s.name == "MetaData"
            && matches!(s.kind, architect::structural::SymbolKind::Class)),
        "Scala extractor failed to find the real MetaData class in scala-xml"
    );
    assert!(
        graph.symbols.values().any(|s| s.name == "Comment"
            && matches!(s.kind, architect::structural::SymbolKind::Class)),
        "Scala extractor failed to find the real Comment class in scala-xml"
    );
}

/// The Rails schema extractor against a real, live RealWorld ("Conduit")
/// implementation — ActiveRecord models + db/schema.rb.
#[test]
fn invariants_rails() {
    let v = check_corpus("rails-realworld-example-app");
    assert!(
        v.is_empty(),
        "rails-realworld-example-app violated {} invariant(s):\n{}",
        v.len(),
        fmt(&v)
    );
}

/// The Rails ROUTE extractor specifically — `invariants_rails` above only
/// exercises the schema layer (`invariants::check` never looks at
/// `graph.routes` at all). Dogfooding this real app is what surfaced that
/// `resources :articles` (a bare Ruby symbol) was never matched by a regex
/// that only ever looked for a quoted string — an entirely idiomatic
/// routes.rb produced zero routes until fixed.
#[test]
fn structural_rails_routes_realworld() {
    let root = corpus_root("rails-realworld-example-app");
    assert!(root.exists(), "corpus repo missing: {}", root.display());
    let (schema_prior, graph_prior) = architect::store::load_raw(&root);
    let (_idx, graph) = scan::scan_with_prior(&root, schema_prior, graph_prior);
    assert!(
        graph.routes.iter().any(|r| r.method == "RESOURCES" && r.path == "/articles"),
        "Rails extractor failed to find the real `resources :articles` route"
    );
    assert!(
        graph.routes.iter().any(|r| r.method == "RESOURCES" && r.path == "/tags"),
        "Rails extractor failed to find the real `resources :tags` route"
    );
}

/// The NestJS schema extractor (TypeORM entities) AND its decorator-based
/// routes, against a real, live RealWorld implementation.
#[test]
fn invariants_nestjs() {
    let v = check_corpus("nestjs-realworld-example-app");
    assert!(
        v.is_empty(),
        "nestjs-realworld-example-app violated {} invariant(s):\n{}",
        v.len(),
        fmt(&v)
    );
}

#[test]
fn structural_nestjs_routes_realworld() {
    let root = corpus_root("nestjs-realworld-example-app");
    assert!(root.exists(), "corpus repo missing: {}", root.display());
    let (schema_prior, graph_prior) = architect::store::load_raw(&root);
    let (_idx, graph) = scan::scan_with_prior(&root, schema_prior, graph_prior);
    assert!(
        !graph.routes.is_empty(),
        "NestJS extractor found zero @Get/@Post-decorated routes in a real NestJS app"
    );
}

/// Vue SFCs + both Nuxt routing conventions (file-based pages, and the
/// `name.method.ts` server/api suffix convention), against Nuxt's own
/// devtools monorepo — a large, real, messy Vue/Nuxt codebase.
#[test]
fn structural_nuxt_devtools_realworld() {
    let root = corpus_root("nuxt-devtools");
    assert!(root.exists(), "corpus repo missing: {}", root.display());
    let (schema_prior, graph_prior) = architect::store::load_raw(&root);
    let (_idx, graph) = scan::scan_with_prior(&root, schema_prior, graph_prior);
    assert!(
        graph.symbols.values().any(|s| matches!(s.kind, architect::structural::SymbolKind::Class) && s.file.ends_with(".vue")),
        "Vue extractor found no .vue file registered as a component symbol"
    );
    assert!(
        graph.routes.iter().any(|r| r.method == "GET" && r.file.ends_with(".vue")),
        "Nuxt page-routing convention (pages/**/*.vue) found no GET route"
    );
    assert!(
        graph.routes.iter().any(|r| r.file.contains("server/api/") || r.file.contains("server/routes/")),
        "Nuxt server-API routing convention (server/api/name.method.ts) found no route"
    );
}

/// gRPC's own canonical examples (helloworld, route_guide) — messages,
/// services, and rpc methods correctly scoped to their own service.
#[test]
fn structural_grpc_examples_realworld() {
    let root = corpus_root("grpc-examples");
    assert!(root.exists(), "corpus repo missing: {}", root.display());
    let (schema_prior, graph_prior) = architect::store::load_raw(&root);
    let (_idx, graph) = scan::scan_with_prior(&root, schema_prior, graph_prior);
    assert!(
        graph.symbols.values().any(|s| s.name == "Greeter" && matches!(s.kind, architect::structural::SymbolKind::Class)),
        "Protobuf extractor failed to find the real Greeter service"
    );
    assert!(
        graph.routes.iter().any(|r| r.path == "/Greeter/SayHello"),
        "Protobuf extractor failed to find the real Greeter/SayHello rpc"
    );
    assert!(
        graph.routes.iter().any(|r| r.path == "/RouteGuide/RouteChat"),
        "Protobuf extractor failed to find the real RouteGuide/RouteChat streaming rpc"
    );
}

// ── helper ───────────────────────────────────────────────────────────────────

fn fmt(violations: &[invariants::Violation]) -> String {
    violations
        .iter()
        .map(|v| format!("  [{}] {}", v.invariant, v.detail))
        .collect::<Vec<_>>()
        .join("\n")
}
