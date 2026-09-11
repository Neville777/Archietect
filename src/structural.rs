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
//!
//! ## Observation mechanism rule
//!
//! Extractors are either *lexical* (regex) or *syntactic* (AST). The rule
//! for choosing between them is not "parsers are more modern" — it is:
//!
//! > Use a syntax-aware observer where the claim being made is a syntactic
//! > fact. Keep lexical matching where the evidence itself is lexical.
//!
//! Examples of syntactic facts: "this Rust file contains a public struct
//! named X", "this function is declared at the top level, not inside an
//! impl block". These require a parser because regex cannot represent the
//! grammar (visibility forms, nesting depth, string boundaries).
//!
//! Examples of legitimately lexical observations: route path strings,
//! framework annotation conventions, schema marker patterns, configuration
//! file conventions. A parser adds no accuracy here and often adds
//! complexity.
//!
//! The `ObservationSource` field on `Symbol` preserves this distinction in
//! the persistent memory: `Ast` means a parser confirmed the declaration;
//! `Lexical` means the claim is intentionally lexical; `LexicalFallback`
//! means a parse failure forced a regex fallback.
//!
//! ## Rust extractor migration (2026-09-07)
//!
//! The Rust extractor was migrated from regex to `syn` after comparing both
//! implementations against all 27 `.rs` files in Archietect's own `src/`
//! directory. The comparison found 11 divergences, classified as:
//!
//!   8 regex false positives — `pub fn foo()` text embedded in raw string
//!     literals (test fixtures) being extracted as declarations. Regex
//!     cannot distinguish Rust-shaped text inside a string from real source.
//!     syn correctly ignored all of them.
//!
//!   3 genuine missed declarations — `pub(crate) fn watchable_dirs`,
//!     `pub(crate) fn relevant`, `pub(crate) fn brace_body_span`. The
//!     pattern `pub\s+fn` does not match `pub(crate) fn` (no space after
//!     `pub`). These were silently ABSENT in Archietect's index of its own
//!     source. syn handles all Visibility::Restricted forms correctly.
//!
//!   0 unresolved divergences.
//!
//! The practical consequence: querying `archietect concept watchable_dirs`
//! previously returned ABSENT ("genuinely new, building it is justified").
//! After migration it returns STRUCTURAL with source location and excerpt.
//! A confident wrong answer became a correct one without changing the query
//! model, verdict model, or evidence model — only the observer changed.
//!
//! The comparison harness lives in `tests/rs_extractor_comparison.rs` and
//! is the template for future extractor migrations (TS/JS next).
//! See also: `extract_rs_syn`, `extract_rs`, `ObservationSource`.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ── Types ────────────────────────────────────────────────────────────────────

/// The kind of a structural symbol. Determines how it participates in
/// How a symbol's existence was established. Preserves the provenance of an
/// observation so callers can reason about confidence and the memory model
/// can eventually surface it.
///
/// This is NOT an evidence tier (those live in model.rs and express
/// schema/usage confidence). It is the observation mechanism — how the
/// extractor arrived at the fact that a symbol exists at all.
///
/// ## Why this matters
///
/// A regex match on `^pub fn` and an AST-confirmed `ItemFn` node look
/// identical after extraction but have different epistemic weight:
///
/// - `Ast`: the parser confirmed that this is a syntactically valid Rust
///   declaration. Visibility, nesting depth, and context are all verified.
/// - `LexicalFallback`: `syn` failed to parse the file (generated code,
///   incomplete snippet, proc-macro output). The regex found something that
///   looks like a declaration but could be in a string literal, a comment,
///   or another context a parser would reject.
/// - `Lexical`: the extractor for this language is regex-based by design
///   (the language doesn't yet have an AST-based extractor). The observation
///   is an honest lexical claim, not a parser-quality claim.
///
/// The distinction becomes load-bearing when Archietect answers:
/// "There is a function called X, established through Rust AST parsing."
/// vs. "There is a function called X (lexical observation, unverified)."
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ObservationSource {
    /// Symbol confirmed by a real syntax tree (syn, tree-sitter, etc.).
    /// Context, nesting, and visibility are verified by the parser.
    Ast,
    /// File failed to parse; regex was used as fallback. The observation is
    /// plausible but not AST-confirmed — treat it like `Lexical`.
    LexicalFallback,
    /// Extractor for this language is regex-based by design. Not a failure;
    /// this is the honest claim of a lexical observer.
    #[default]
    Lexical,
}

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
    /// How this symbol's existence was established. Distinguishes AST-
    /// confirmed declarations from lexical observations and parse-failure
    /// fallbacks. Defaults to `Lexical` when deserializing old records that
    /// predate this field — the honest conservative claim.
    #[serde(default)]
    pub observation_source: ObservationSource,
}

impl Symbol {
    /// Project this structural symbol onto the general `Resource` shape
    /// (see resource.rs / SYSTEM_MEMORY.md). Reproduces exactly the single
    /// evidence entry `query::concept`'s STRUCTURAL tier already builds
    /// inline (`"{:?} declared in {}:{}"`) — construction moves here, the
    /// string does not change.
    pub fn to_resource(&self) -> crate::resource::Resource {
        crate::resource::Resource {
            id: crate::resource::Identity(self.name.clone()),
            kind: format!("{:?}", self.kind),
            domain: "code".to_string(),
            location: crate::resource::Location { file: self.file.clone(), line: Some(self.line) },
            attributes: Default::default(),
            evidence: vec![crate::model::Evidence {
                tier: crate::model::Tier::Declared,
                what: format!("{:?} declared in {}:{}", self.kind, self.file, self.line),
            }],
        }
    }
}

/// 1-indexed line number containing byte offset `pos` in `text`.
fn line_of(text: &str, pos: usize) -> usize {
    text.as_bytes()[..pos.min(text.len())].iter().filter(|&&b| b == b'\n').count() + 1
}

/// Extract text from a tree-sitter node.
fn ts_text(node: tree_sitter::Node, bytes: &[u8]) -> String {
    std::str::from_utf8(&bytes[node.start_byte()..node.end_byte()])
        .unwrap_or("")
        .to_string()
}

/// An HTTP route extracted from source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub method: String,  // GET, POST, PUT, DELETE, PATCH, ...
    pub path: String,    // /orders, /orders/:id, ...
    pub handler: String, // the function/class name that handles it
    pub file: String,
}

/// An OUTBOUND HTTP call — `requests.post(f"http://svc:8000/orders/{id}")`,
/// `fetch('/api/orders')`, `axios.get(\`/orders/${id}\`)` — found live: a
/// declared route in one language (a Rust struct behind an Axum handler) had
/// its only real caller in a DIFFERENT file written in a DIFFERENT language,
/// calling it over HTTP rather than an in-process function call. Every usage
/// signal this engine had before this (ORM/schema matchers, the import-graph
/// walk in `structural_dependents`) requires either a same-language call
/// expression or an import edge — neither exists for a cross-service HTTP
/// call, so `impact()` reported "declared but nothing observed touching it"
/// for a route that was, in fact, live and load-bearing. This is the other
/// half of a Route: not who DECLARES the endpoint, but who CALLS it.
///
/// `path` is the literal path text as it appeared in source, unnormalized —
/// normalization happens once, at match time, in `paths_match` below, so a
/// caller comparing against several declared routes doesn't need to know
/// this struct's own extraction quirks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteCall {
    pub file: String,
    pub path: String,
}

/// Strip a URL down to its path: drop `scheme://host:port`, drop any query
/// string. `f"http://ws-gateway:8001/orders/{id}?verbose=1"` becomes
/// `/orders/{id}`. A literal with no scheme (`"/orders"`, already a bare
/// path — the common case for a same-origin `fetch()`) passes through
/// unchanged apart from the query-string trim.
fn path_only(raw: &str) -> String {
    let without_query = raw.split('?').next().unwrap_or(raw);
    if let Some(after_scheme) = without_query.split("://").nth(1) {
        // after_scheme is "host:port/path..." — the path starts at the
        // first '/', or is empty (bare "http://host" with no path at all).
        match after_scheme.find('/') {
            Some(i) => after_scheme[i..].to_string(),
            None => String::new(),
        }
    } else {
        without_query.to_string()
    }
}

/// Whether a DECLARED route path and a CALLED literal path are the same
/// endpoint, treating any dynamic segment on EITHER side as a wildcard.
/// Frameworks spell a path parameter differently (`:id`, `{id}`, `<id>`),
/// and a caller's literal is often an f-string/template-literal interpolation
/// (`{session_id}`, `${sessionId}`) whose braces survive into the extracted
/// text — normalizing both to "some segment, don't care what" is what lets
/// `/orders/{id}` (declared) match `f"/orders/{order_id}"` (called) even
/// though neither the parameter's name nor its delimiter matches textually.
/// Segment COUNT must still match — this only forgives what a real path
/// parameter always varies, not a genuinely different route shape.
fn paths_match(declared: &str, called: &str) -> bool {
    fn is_dynamic(segment: &str) -> bool {
        (segment.starts_with('{') && segment.ends_with('}'))
            || (segment.starts_with('<') && segment.ends_with('>'))
            || (segment.starts_with("${") && segment.ends_with('}'))
            || segment.starts_with(':')
    }
    let norm = |p: &str| -> Vec<String> {
        path_only(p)
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    };
    let d = norm(declared);
    let c = norm(called);
    if d.len() != c.len() || d.is_empty() {
        return false;
    }
    d.iter().zip(c.iter()).all(|(ds, cs)| {
        is_dynamic(ds) || is_dynamic(cs) || ds.eq_ignore_ascii_case(cs)
    })
}

/// Find outbound HTTP calls in `text` — the literal path passed to a common
/// client-library call. Gated by `ext`, not by matching the caller's own
/// language against the route's declaring language: the whole POINT of this
/// extractor is a cross-language call (see `RouteCall`'s doc), so declared
/// language is irrelevant here — only which client-call SYNTAX this file's
/// language actually uses.
///
/// A verb-shaped call like `.get("/orders")` is genuinely ambiguous in both
/// languages this supports: FastAPI declares routes as `@app.get("/orders")`
/// — a DECORATOR, not a call, and one of this engine's own supported
/// frameworks — and Express declares them as `router.get('/orders', ...)`,
/// syntactically identical to Axios calling out to that same path. Getting
/// this backwards would misfile a server's own route declarations as
/// outbound calls, which is worse than missing a real call (this project's
/// own stated principle for routes generally — "a wrong route is worse than
/// a missing one"). So: Python is matched per LINE with decorator lines
/// (`@...`) excluded outright, and JS/TS is narrowed to an explicit client
/// identifier (`axios`/`apiClient`/`httpClient`) rather than any receiver —
/// `router.get(`/`app.get(` (Express's declaration syntax) never matches
/// that anchor, so accepting the recall loss on unnamed/rebound client
/// instances is the trade this makes, not an oversight.
///
/// Rust is excluded from REST call detection specifically (`reqwest`,
/// `ureq`): their common call shape is bare `client.get(url)`, and there's
/// no equally reliable anchor to require without a real parser tracking
/// types. WebSocket connection calls are a separate branch below with their
/// own, unambiguous anchors per language (`websockets.connect(...)` /
/// `new WebSocket(...)` / tokio-tungstenite's `connect_async(...)`) — none
/// of them share the generic-verb-method ambiguity a REST call has, so Rust
/// participates there even though it sits out REST entirely.
///
/// Whether a captured literal is plausibly a URL/path at all, not just any
/// quoted string that happened to follow `.get(`/`.post(`/etc. Found while
/// building this: `cache.get("some_key")` and `params.get("id")` are
/// completely ordinary, unrelated Python/JS calls with the exact same
/// `.get("...")` shape a route call has — without this filter, EVERY dict-
/// or map-like `.get(` in a scanned repo would get recorded as an outbound
/// HTTP call. A real path either starts with `/` (the overwhelmingly common
/// same-origin/relative case) or contains `://` (an absolute URL) — neither
/// is true of an arbitrary lookup key.
fn looks_like_path(s: &str) -> bool {
    s.starts_with('/') || s.contains("://")
}

fn extract_route_calls(rel: &str, ext: &str, text: &str, route_calls: &mut Vec<RouteCall>) {
    match ext {
        "py" => {
            // requests.get(...) / httpx.post(...) / session.put(...) — the
            // receiver varies (a client instance, not always the literal
            // module name), so this matches the HTTP-verb METHOD name after
            // ANY `.`, then excludes decorator lines separately below rather
            // than trying to anchor the receiver — see this fn's own doc for
            // why FastAPI's `@app.get(...)` route declarations are the
            // specific collision this guards against.
            let re = Regex::new(
                r#"\.\s*(?:get|post|put|patch|delete)\s*\(\s*f?["']([^"']+)["']"#
            ).unwrap();
            for line in text.lines() {
                if line.trim_start().starts_with('@') {
                    continue;
                }
                for cap in re.captures_iter(line) {
                    if looks_like_path(&cap[1]) {
                        route_calls.push(RouteCall { file: rel.to_string(), path: cap[1].to_string() });
                    }
                }
            }
            // websockets.connect("ws://host/path") / await
            // websocket_client.connect(...) — anchored on the literal
            // substring "websocket" (case-insensitive) rather than a fixed
            // module name, since `import websockets as ws` and similar
            // renames are common; unambiguous enough on its own that no
            // decorator-line exclusion is needed here (no Python web
            // framework spells a WS route declaration as `.connect(`).
            let ws_re = Regex::new(
                r#"(?i)websocket\w*\s*\.\s*connect\s*\(\s*f?["']([^"']+)["']"#
            ).unwrap();
            for cap in ws_re.captures_iter(text) {
                if looks_like_path(&cap[1]) {
                    route_calls.push(RouteCall { file: rel.to_string(), path: cap[1].to_string() });
                }
            }
        }
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "vue" => {
            // fetch('/orders') / fetch(`/orders/${id}`) — `fetch` has no
            // server-side-declaration meaning in any supported framework, so
            // this one can safely match any receiver-free call.
            let fetch_re = Regex::new(r#"\bfetch\s*\(\s*[`"']([^`"']+)"#).unwrap();
            for cap in fetch_re.captures_iter(text) {
                if looks_like_path(&cap[1]) {
                    route_calls.push(RouteCall { file: rel.to_string(), path: cap[1].to_string() });
                }
            }
            // axios.get('/orders') / apiClient.post(`/orders/${id}`, ...) —
            // anchored on an explicit client-ish identifier, NOT any
            // receiver: `router.get('/orders', handler)`/`app.post(...)` are
            // Express's own route-DECLARATION syntax (already extracted as
            // Routes by extract_ts_routes) and are byte-for-byte the same
            // shape as an Axios call otherwise. A configured client
            // reassigned to some other local name won't match this — a
            // stated recall tradeoff, not a bug.
            let client_re = Regex::new(
                r#"\b(?:axios|apiClient|httpClient)\s*\.\s*(?:get|post|put|patch|delete)\s*\(\s*[`"']([^`"']+)"#
            ).unwrap();
            for cap in client_re.captures_iter(text) {
                if looks_like_path(&cap[1]) {
                    route_calls.push(RouteCall { file: rel.to_string(), path: cap[1].to_string() });
                }
            }
            // new WebSocket("ws://host/path") — the standard browser/Node
            // WS client API, unambiguous on its own (nothing else in JS is
            // spelled this way), so no receiver-anchoring caveat applies.
            let ws_re = Regex::new(r#"\bnew\s+WebSocket\s*\(\s*[`"']([^`"']+)"#).unwrap();
            for cap in ws_re.captures_iter(text) {
                if looks_like_path(&cap[1]) {
                    route_calls.push(RouteCall { file: rel.to_string(), path: cap[1].to_string() });
                }
            }
        }
        "rs" => {
            // tokio-tungstenite's canonical connect fn — specific enough a
            // name (not a bare `.connect(`) that it needs no additional
            // guard the way a generic `.get(` would on this language; see
            // extract_route_calls's own module-level doc for why Rust is
            // otherwise excluded from REST call detection entirely.
            let ws_re = Regex::new(r#"\bconnect_async\s*\(\s*"([^"]+)""#).unwrap();
            for cap in ws_re.captures_iter(text) {
                if looks_like_path(&cap[1]) {
                    route_calls.push(RouteCall { file: rel.to_string(), path: cap[1].to_string() });
                }
            }
        }
        _ => {}
    }
}

/// One top-level function's own literal-string "fingerprint" — the distinct
/// string literals it contains, not its full body text or behavior. Example
/// of the shape this catches: `updateCandidateStage` (a Node/JS backend) and
/// `moveCandidateToStage` (a TypeScript frontend) independently
/// reimplementing the same stage-derivation business rule — same decisions,
/// same status strings, completely different function names and no shared
/// import or call edge. None of the engine's other matchers can connect
/// them: not the schema-usage matchers (no ORM/schema concept involved at
/// all), not `structural_dependents` (no import edge — independent
/// reimplementations, not one calling the other), not `duplicates()` (that
/// compares CONCEPT names sharing a token, and these two function names
/// share nothing but a generic "stage" token — too thin to mean anything).
///
/// The literal strings inside two such functions are real, checkable
/// evidence a name comparison can never see: two functions that both
/// compare against the literal string `"Passed Screening"` are provably
/// encoding the same business rule, regardless of what either function or
/// its file is called. This is the exact same "shared token implies
/// possible duplication — risk, not proof" shape `duplicates()` already
/// uses for concept names, applied to function bodies instead of names.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionBody {
    pub file: String,
    pub name: String,
    /// Sorted, deduplicated. Short/trivial literals are filtered at
    /// extraction time (see `extract_function_bodies`'s own doc) — this is
    /// never the function's full literal population, only the ones long
    /// enough to mean something as evidence.
    pub literals: Vec<String>,
}

/// A call edge: function `caller` in `from_file` calls function `callee`
/// (possibly in a different file, resolved via the symbol table).
/// Used to extend `structural_dependents` beyond import-graph depth.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CallEdge {
    pub from_file: String,
    pub caller: String,
    pub callee: String,
}

/// String literals inside `body`, filtered to ones actually meaningful as
/// duplicate-logic evidence (see the length floor and color exclusion
/// inline below, both tuned against a real repo, not guessed). Deliberately
/// NOT filtering by "looks like a status value" or any other guess at what
/// the literal MEANS — that would be exactly the kind of invented semantic
/// judgment this engine's whole design avoids; length and "is this a CSS
/// color" are objective, checkable properties, "probably an enum-like
/// business value" is not.
/// `ext` gates whether single-quoted strings count as literals. Rust has no
/// multi-char single-quoted string syntax — `'` there is either a 1-char
/// char literal or an unmatched lifetime marker like `&'static`/`'a`. Verified
/// against a real repo: treating `'` as a string delimiter in Rust paired a
/// lifetime's opening quote with an unrelated apostrophe much later in the
/// same function (a comment like "the polygon's rings", or a `''` empty-string
/// SQL literal), capturing everything between as a fake "shared literal".
///
/// The boundary regex matches a string's full content (any length, with
/// `\\.` so an escaped quote doesn't end it early) and the 4-char floor is
/// applied AFTERWARD as a filter, not baked into the match itself. Also
/// verified against a real repo: with the floor inside the regex (`{4,}?`),
/// a literal shorter than 4 chars (e.g. `"id"`) can't match at its own
/// quotes, so the engine skipped past it and matched from the NEXT open
/// quote onward instead — capturing the source code between two adjacent
/// short literals (e.g. `: r.try_get::<i64, _>(` between two `"id"` calls)
/// as a fake shared "literal" purely because nearby functions had
/// similar-shaped boilerplate, not because they shared any real string
/// constant.
fn literals_in(body: &str, ext: &str) -> Vec<String> {
    // `\\[\s\S]` (not `\\.`) — the regex crate's `.` excludes `\n` by
    // default, and a Rust string can contain a real backslash-newline
    // continuation (`"SELECT ... \` at end of line). With `\\.` that
    // continuation matched neither alternative, so the match failed at
    // that string's own quotes and slid forward to some unrelated later
    // quote instead — merging the code between two real strings into one
    // bogus "literal". Verified against a real repo (a multi-line SQL
    // query built this way) before this fix landed.
    let re = if ext == "rs" {
        Regex::new(r#""((?:[^"\\]|\\[\s\S])*)""#).unwrap()
    } else {
        Regex::new(r#"'((?:[^'\\]|\\[\s\S])*)'|"((?:[^"\\]|\\[\s\S])*)""#).unwrap()
    };
    let mut out: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for cap in re.captures_iter(body) {
        let s = cap.get(1).or_else(|| cap.get(2)).unwrap().as_str();
        // Floor of 10, not 4: a shorter floor matches JSON-shaped field
        // names like "name"/"note"/"id", which let near-unrelated functions
        // pair up on shared vocabulary rather than real shared business
        // strings (SQL fragments, distinctive error messages, event-status
        // names).
        if s.len() >= 10 && !looks_like_color_literal(s) {
            out.insert(s.to_string());
        }
    }
    out.into_iter().collect()
}

/// A CSS color value (`#rrggbb` or `rgb(...)`/`rgba(...)`) is a design
/// token, not business logic — verified against a real repo where two
/// unrelated "status color" UI components recurred across dozens of pairs
/// purely because they drew from the same small traffic-light palette.
fn looks_like_color_literal(s: &str) -> bool {
    let is_hex = s.starts_with('#')
        && matches!(s.len(), 4 | 5 | 7 | 9)
        && s[1..].chars().all(|c| c.is_ascii_hexdigit());
    is_hex || s.starts_with("rgb(") || s.starts_with("rgba(")
}

/// Finds the matching closing brace for the `{` at-or-after `start` in
/// `text`, returning the byte range of everything BETWEEN the braces
/// (excluding both). Tracks string-literal contents separately so a quote
/// mark or brace-like character INSIDE a string can't desynchronize the
/// depth count — found necessary immediately: a function body containing
/// `"{"` as a literal (entirely plausible — JSON-shaped strings are common)
/// would otherwise close the scan early. Doesn't need a real parser's
/// escape-sequence table to do this well: skipping over anything between
/// two matching quote characters is enough for the vast majority of real
/// code, and this is a bounded heuristic feeding a "risk, not proof"
/// signal, not a correctness-critical parse.
///
/// `ext == "rs"` disables `'` as a string-open character, for the same
/// reason as `literals_in`: verified against a real repo that a Rust
/// lifetime (`&'static`) or lone apostrophe (a comment like "the polygon's
/// rings") was being treated as opening a string with no real closing
/// quote, which swallowed the depth count until some unrelated later `'`
/// happened to appear — letting the scan run past the function's real
/// closing `}` into unrelated code and pull unrelated functions' text into
/// the extracted "body".
pub(crate) fn brace_body_span(text: &str, start: usize, ext: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let open = start + text[start..].find('{')?;
    let mut depth = 0i32;
    let mut i = open;
    let mut in_string: Option<u8> = None;
    let quote_chars: &[u8] = if ext == "rs" { b"\"`" } else { b"'\"`" };
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = in_string {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == q {
                in_string = None;
            }
        } else {
            match b {
                b if quote_chars.contains(&b) => in_string = Some(b),
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some((open + 1, i));
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// Top-level function bodies, for the duplicate-logic signal `RouteCall`'s
/// own sibling comment describes. TS/JS and Rust use `brace_body_span`
/// (both are brace-delimited); Python's body is bounded by indentation
/// instead, matched separately below. Reuses the EXACT same function-start
/// patterns already used to find declarations for the symbol index — this
/// is deliberately not a second, independent guess at "what counts as a
/// function" that could quietly drift from what `extract_ts_js`/`extract_py`/
/// `extract_rs` already decided.
/// Extract call edges from Rust source using tree-sitter.
/// Returns (caller_name, callee_name) pairs for each call inside a pub fn body.
/// Only collects simple identifier calls (foo()) and method calls (self.foo()),
/// not full path calls (foo::bar()) — those are imports, not calls.
fn extract_call_edges_rs(rel: &str, text: &str, out: &mut Vec<CallEdge>) {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(tree_sitter_rust::language()).is_err() { return; }
    let tree = match parser.parse(text, None) { Some(t) => t, None => return };
    let bytes = text.as_bytes();

    // Walk top-level pub fn items, then collect call_expression nodes inside
    let root = tree.root_node();
    let mut cursor = root.walk();
    if !cursor.goto_first_child() { return; }
    loop {
        let node = cursor.node();
        if node.kind() == "function_item" {
            if let Some(name_node) = node.child_by_field_name("name") {
                let caller = std::str::from_utf8(
                    &bytes[name_node.start_byte()..name_node.end_byte()]
                ).unwrap_or("").to_string();
                if !caller.is_empty() {
                    collect_calls_in_node(&node, bytes, rel, &caller, out);
                }
            }
        }
        if !cursor.goto_next_sibling() { break; }
    }
}

fn collect_calls_in_node(
    node: &tree_sitter::Node,
    bytes: &[u8],
    rel: &str,
    caller: &str,
    out: &mut Vec<CallEdge>,
) {
    if node.kind() == "call_expression" {
        if let Some(func) = node.child_by_field_name("function") {
            let callee = match func.kind() {
                "identifier" => {
                    std::str::from_utf8(&bytes[func.start_byte()..func.end_byte()])
                        .unwrap_or("").to_string()
                }
                "field_expression" => {
                    // self.foo() or obj.method() — take the field name
                    func.child_by_field_name("field")
                        .map(|f| std::str::from_utf8(&bytes[f.start_byte()..f.end_byte()]).unwrap_or("").to_string())
                        .unwrap_or_default()
                }
                _ => String::new(),
            };
            if !callee.is_empty() && callee != caller {
                out.push(CallEdge {
                    from_file: rel.to_string(),
                    caller: caller.to_string(),
                    callee,
                });
            }
        }
    }
    let mut c = node.walk();
    if c.goto_first_child() {
        loop {
            collect_calls_in_node(&c.node(), bytes, rel, caller, out);
            if !c.goto_next_sibling() { break; }
        }
    }
}

/// Extract call edges from TypeScript/JavaScript using tree-sitter.
fn extract_call_edges_ts(rel: &str, text: &str, out: &mut Vec<CallEdge>) {
    let lang = if rel.ends_with(".ts") || rel.ends_with(".tsx") {
        tree_sitter_typescript::language_typescript()
    } else {
        tree_sitter_javascript::language()
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(lang).is_err() { return; }
    let tree = match parser.parse(text, None) { Some(t) => t, None => return };
    let bytes = text.as_bytes();
    walk_ts_calls(&tree.root_node(), bytes, rel, "global", out, 0);
}

fn walk_ts_calls(
    node: &tree_sitter::Node,
    bytes: &[u8],
    rel: &str,
    current_fn: &str,
    out: &mut Vec<CallEdge>,
    depth: usize,
) {
    if depth > 50 { return; } // guard against pathological nesting

    let kind = node.kind();

    // Track current function context
    let new_fn = if matches!(kind, "function_declaration" | "method_definition" | "arrow_function" | "function") {
        node.child_by_field_name("name")
            .map(|n| std::str::from_utf8(&bytes[n.start_byte()..n.end_byte()]).unwrap_or("").to_string())
            .filter(|s| !s.is_empty())
    } else {
        None
    };
    let fn_ctx = new_fn.as_deref().unwrap_or(current_fn);

    if kind == "call_expression" {
        let callee = match node.child_by_field_name("function") {
            Some(f) => match f.kind() {
                "identifier" => std::str::from_utf8(&bytes[f.start_byte()..f.end_byte()]).unwrap_or("").to_string(),
                "member_expression" => f.child_by_field_name("property")
                    .map(|p| std::str::from_utf8(&bytes[p.start_byte()..p.end_byte()]).unwrap_or("").to_string())
                    .unwrap_or_default(),
                _ => String::new(),
            },
            None => String::new(),
        };
        if !callee.is_empty() && callee != fn_ctx && callee != "require" {
            out.push(CallEdge { from_file: rel.to_string(), caller: fn_ctx.to_string(), callee });
        }
    }

    let mut c = node.walk();
    if c.goto_first_child() {
        loop {
            walk_ts_calls(&c.node(), bytes, rel, fn_ctx, out, depth + 1);
            if !c.goto_next_sibling() { break; }
        }
    }
}

fn extract_function_bodies(rel: &str, ext: &str, text: &str, out: &mut Vec<FunctionBody>) {
    match ext {
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts" | "vue" => {
            let decl_re = Regex::new(
                r"(?m)^(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s+([A-Za-z_$][A-Za-z0-9_$]*)"
            ).unwrap();
            for cap in decl_re.captures_iter(text) {
                let end = cap.get(0).unwrap().end();
                if let Some((s, e)) = brace_body_span(text, end, ext) {
                    let literals = literals_in(&text[s..e], ext);
                    if !literals.is_empty() {
                        out.push(FunctionBody { file: rel.to_string(), name: cap[1].to_string(), literals });
                    }
                }
            }
            let const_re = Regex::new(
                r"(?m)^(?:export\s+)?const\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*(?:async\s*)?\("
            ).unwrap();
            for cap in const_re.captures_iter(text) {
                let end = cap.get(0).unwrap().end();
                if let Some((s, e)) = brace_body_span(text, end, ext) {
                    let literals = literals_in(&text[s..e], ext);
                    if !literals.is_empty() {
                        out.push(FunctionBody { file: rel.to_string(), name: cap[1].to_string(), literals });
                    }
                }
            }
        }
        "rs" => {
            let re = Regex::new(r"(?m)^pub\s+(?:async\s+)?fn\s+([a-z_][A-Za-z0-9_]*)").unwrap();
            for cap in re.captures_iter(text) {
                let end = cap.get(0).unwrap().end();
                if let Some((s, e)) = brace_body_span(text, end, ext) {
                    let literals = literals_in(&text[s..e], ext);
                    if !literals.is_empty() {
                        out.push(FunctionBody { file: rel.to_string(), name: cap[1].to_string(), literals });
                    }
                }
            }
        }
        "py" => {
            let re = Regex::new(r"(?m)^(?:async\s+)?def\s+([a-z_][A-Za-z0-9_]*)\s*\(").unwrap();
            let lines: Vec<&str> = text.lines().collect();
            for cap in re.captures_iter(text) {
                let name = cap[1].to_string();
                let def_line_start = line_of(text, cap.get(0).unwrap().start()) - 1; // 0-indexed
                let def_indent = lines[def_line_start].len() - lines[def_line_start].trim_start().len();
                let mut body_lines = Vec::new();
                for line in lines.iter().skip(def_line_start + 1) {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let indent = line.len() - line.trim_start().len();
                    if indent <= def_indent {
                        break;
                    }
                    body_lines.push(*line);
                }
                let literals = literals_in(&body_lines.join("\n"), ext);
                if !literals.is_empty() {
                    out.push(FunctionBody { file: rel.to_string(), name, literals });
                }
            }
        }
        _ => {}
    }
}

/// Two functions' shared literal count meets `duplicate_logic`'s threshold —
/// see `query::duplicate_logic`'s own doc for what this evidence means and
/// why the threshold is set where it is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuspectedDuplicateLogic {
    pub a_file: String,
    pub a_name: String,
    pub b_file: String,
    pub b_name: String,
    pub shared_literals: Vec<String>,
}

/// Cross-references every `FunctionBody` in `graph` against every other in
/// a DIFFERENT file, reporting pairs sharing at least `min_shared` literal
/// values. Same defensive shape `duplicates()` already uses for concept
/// names (an inverted index keeps this from being a blind O(n²) scan over
/// every function pair in a large repo — found necessary the same way
/// `duplicates()` needed it, for the same reason: this is exactly the kind
/// of pairwise comparison that gets slow fast once a real repo has
/// thousands of functions).
pub fn suspected_duplicate_logic(graph: &StructuralGraph, min_shared: usize) -> Vec<SuspectedDuplicateLogic> {
    let mut literal_index: std::collections::HashMap<&str, Vec<usize>> = std::collections::HashMap::new();
    for (i, f) in graph.function_bodies.iter().enumerate() {
        for lit in &f.literals {
            literal_index.entry(lit.as_str()).or_default().push(i);
        }
    }
    let mut shared_counts: std::collections::HashMap<(usize, usize), Vec<&str>> = std::collections::HashMap::new();
    for (_, indices) in literal_index.iter() {
        if indices.len() < 2 || indices.len() > 15 {
            // A literal shared by more than 15 functions is common
            // boilerplate, not a meaningful fingerprint — including it
            // would connect nearly every function in a large repo to
            // nearly every other one. Common-but-not-universal strings
            // (schema field names occurring in dozens of functions) are
            // exactly the case a higher cap lets through as noise.
            continue;
        }
        for i in 0..indices.len() {
            for j in (i + 1)..indices.len() {
                let (a, b) = (indices[i], indices[j]);
                if graph.function_bodies[a].file == graph.function_bodies[b].file {
                    continue;
                }
                let key = if a < b { (a, b) } else { (b, a) };
                shared_counts.entry(key).or_default();
            }
        }
    }
    // Second pass: now that pairs are known, compute each pair's REAL full
    // shared-literal set (not just the ones the sampling above happened to
    // iterate) via a plain set intersection — cheap once the candidate
    // pairs are already narrowed to a small list.
    let mut out = Vec::new();
    for (a, b) in shared_counts.keys() {
        let fa = &graph.function_bodies[*a];
        let fb = &graph.function_bodies[*b];
        let shared: Vec<String> = fa.literals.iter().filter(|l| fb.literals.contains(l)).cloned().collect();
        if shared.len() >= min_shared {
            out.push(SuspectedDuplicateLogic {
                a_file: fa.file.clone(),
                a_name: fa.name.clone(),
                b_file: fb.file.clone(),
                b_name: fb.name.clone(),
                shared_literals: shared,
            });
        }
    }
    out.sort_by(|x, y| (x.a_file.as_str(), x.a_name.as_str()).cmp(&(y.a_file.as_str(), y.a_name.as_str())));
    out
}

impl Route {
    /// Whether/why this route is believed to relate to `concept_name` — the
    /// evidence `routes_for_concept` previously computed as a bare boolean
    /// filter and discarded. See resource.rs::Relationship / SYSTEM_MEMORY.md
    /// ("Relationships need their own evidence"): a route's handler actually
    /// naming the concept is a real structural fact (USED tier — the code
    /// demonstrably ties this route to that name); the route's PATH merely
    /// containing the concept's name is much weaker (NAMED tier — resemblance
    /// only), same distinction the rest of the engine already draws for a
    /// bare name match versus a real declared/observed one.
    pub fn relationship_to(&self, concept_name: &str) -> Option<crate::resource::Relationship> {
        use crate::model::{names_concept, Evidence, Tier};
        let evidence = if names_concept(&self.handler, concept_name) {
            Evidence {
                tier: Tier::Used,
                what: format!(
                    "route handler '{}' in {} name-matches concept '{}'",
                    self.handler, self.file, concept_name
                ),
            }
        } else if self.path.to_lowercase().contains(&concept_name.to_lowercase()) {
            Evidence {
                tier: Tier::Named,
                what: format!(
                    "route path '{}' contains concept name '{}'",
                    self.path, concept_name
                ),
            }
        } else {
            return None;
        };
        Some(crate::resource::Relationship {
            from: crate::resource::Identity(format!("{} {}", self.method, self.path)),
            kind: "handles".to_string(),
            to: crate::resource::Identity(concept_name.to_string()),
            evidence,
        })
    }
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

impl Import {
    /// Whether this import resolves, EXACTLY and UNAMBIGUOUSLY, to a file
    /// this scan actually knows about — and if so, the real `Relationship`
    /// that fact supports. See resource.rs::Relationship / SYSTEM_MEMORY.md
    /// ("Relationships need their own evidence, not borrowed evidence").
    ///
    /// `structural_dependents`'s own internal `importers_of` closure already
    /// walks this same import graph, but with a FUZZY heuristic (stem/suffix
    /// matching) that is fine for an internal "roughly what's affected" BFS
    /// and NOT fine for a claimed, evidenced fact — a stem match can be
    /// wrong (two files sharing a basename in different directories), and
    /// presenting that guess as a `Relationship` would be exactly the
    /// inference-dressed-as-evidence failure this project exists to
    /// prevent. This method is deliberately stricter: it returns `Some`
    /// only when resolution is exact and unique, `None` otherwise — silence
    /// is the correct answer for anything it can't establish confidently,
    /// the same discipline `docker_domain`/`git_domain` already apply to
    /// their own domains.
    ///
    /// Scope, stated plainly rather than faked: only slash-style relative
    /// imports (`./foo`, `../bar/baz` — what `extract_ts_js`/`extract_dart`
    /// and similar extractors actually produce) are resolved here. Python's
    /// dotted package-relative imports (`from .utils import x`) use a
    /// DIFFERENT addressing scheme — package-relative dotted paths, not
    /// slash-separated file paths — and correctly resolving them requires
    /// knowing package boundaries (`__init__.py` presence, namespace
    /// packages), a meaningfully different and harder algorithm. Bare
    /// package imports (`lodash`, `import "fmt"`, `from django import
    /// forms`) are never attempted: there is no reliable exact-resolution
    /// strategy for an external package without a real, language-specific
    /// package resolver, and guessing one would reintroduce exactly the
    /// fuzzy-match risk this method exists to avoid.
    pub fn relationship(&self, known_files: &std::collections::BTreeSet<String>, workspace_packages: &BTreeMap<String, String>) -> Option<crate::resource::Relationship> {
        let resolved = resolve_relative_import(&self.from_file, &self.to_module, known_files, workspace_packages)?;
        Some(crate::resource::Relationship {
            from: crate::resource::Identity(self.from_file.clone()),
            kind: "imports".to_string(),
            to: crate::resource::Identity(resolved.clone()),
            evidence: crate::model::Evidence {
                tier: crate::model::Tier::Declared,
                what: format!(
                    "'{}' imports '{}', which resolves unambiguously to the scanned file '{}' \
                     (relative import; extension/index inferred, never guessed across more than one match)",
                    self.from_file, self.to_module, resolved
                ),
            },
        })
    }
}

/// Resolve a slash-style relative import (`./x`, `../x/y`) written in
/// `from_file` against the set of files this scan actually knows about
/// (`StructuralGraph::file_facts`'s keys — every scanned file, not just
/// ones with symbols, so a pure re-export/index file still counts as a
/// valid target). Purely lexical — no filesystem access, since these are
/// virtual repo-relative strings, not real paths relative to the process's
/// CWD. Returns `None` for a non-relative `to_module` (out of scope — see
/// `Import::relationship`'s doc) or for zero/multiple matches (unresolved
/// or ambiguous — never guess between them).
fn resolve_relative_import(
    from_file: &str,
    to_module: &str,
    known_files: &std::collections::BTreeSet<String>,
    workspace_packages: &BTreeMap<String, String>,
) -> Option<String> {
    // Godot's `res://` paths are already root-relative and always written
    // with an explicit extension (`res://player/Foo.tscn`, never an
    // extension-optional `res://player/Foo`) — an exact, unambiguous lookup,
    // not the component-walking/extension-guessing the `./`/`../` case below
    // needs.
    if let Some(stripped) = to_module.strip_prefix("res://") {
        return known_files.contains(stripped).then(|| stripped.to_string());
    }

    // ── Workspace package resolution ──────────────────────────────────────
    // `import { X } from "@myorg/core-models"` — not a relative path, but
    // the package name maps to a local directory in this monorepo. Try exact
    // match first, then prefix match (for scoped packages like @myorg/pkg
    // the to_module might be "@myorg/pkg/subpath").
    if !to_module.starts_with("./") && !to_module.starts_with("../") {
        // Exact match: "@myorg/core-models" → "packages/core-models"
        if let Some(local_dir) = workspace_packages.get(to_module) {
            // Resolve to the package's index file
            const INDEX_EXTS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs"];
            for ext in INDEX_EXTS {
                let candidate = format!("{local_dir}/index.{ext}");
                if known_files.contains(&candidate) {
                    return Some(candidate);
                }
                let src_candidate = format!("{local_dir}/src/index.{ext}");
                if known_files.contains(&src_candidate) {
                    return Some(src_candidate);
                }
            }
            // No index file found but the package is known — return the dir
            return Some(local_dir.clone());
        }
        // Prefix match: "@myorg/core-models/utils" → "packages/core-models/utils"
        for (pkg_name, local_dir) in workspace_packages {
            if let Some(subpath) = to_module.strip_prefix(pkg_name.as_str()) {
                let subpath = subpath.trim_start_matches('/');
                let full = if subpath.is_empty() {
                    local_dir.clone()
                } else {
                    format!("{local_dir}/{subpath}")
                };
                if known_files.contains(&full) {
                    return Some(full);
                }
            }
        }
        return None;
    }

    let from_dir = std::path::Path::new(from_file).parent().unwrap_or_else(|| std::path::Path::new(""));
    let mut components: Vec<String> = from_dir
        .components()
        .filter_map(|c| c.as_os_str().to_str().map(str::to_string))
        .collect();
    for part in to_module.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            other => components.push(other.to_string()),
        }
    }
    let normalized = components.join("/");
    if normalized.is_empty() {
        return None;
    }

    // Candidate set: the normalized path itself (to_module already carried
    // an extension), the normalized path with a common source extension
    // appended, and the normalized path as a directory with an index file.
    // Common to the languages that actually write slash-style relative
    // imports (extract_ts_js, extract_dart, and any future one sharing the
    // shape) — not an attempt to cover every language's own convention.
    const EXTS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs", "vue", "dart"];
    let mut matches: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if known_files.contains(&normalized) {
        matches.insert(normalized.clone());
    }
    for ext in EXTS {
        let with_ext = format!("{normalized}.{ext}");
        if known_files.contains(&with_ext) {
            matches.insert(with_ext);
        }
        let index = format!("{normalized}/index.{ext}");
        if known_files.contains(&index) {
            matches.insert(index);
        }
    }

    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

/// The full structural graph for one repository.
///
/// This is a VALUE TYPE — rebuilt from cache on every scan just like `Index`.
/// It lives alongside `Index` in the scan result and is serialised into
/// `archietect.db` as a second row in the `idx` table.
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
    /// Outbound HTTP calls found in source — the other half of a Route; see
    /// `RouteCall`'s own doc for why this exists. `#[serde(default)]`: an
    /// `archietect.db` written before this field existed deserializes with
    /// an empty Vec instead of failing, same forward-compat pattern as
    /// `file_facts`/`extractor_version` below.
    #[serde(default)]
    pub route_calls: Vec<RouteCall>,
    /// Top-level function bodies' literal-string fingerprints — see
    /// `FunctionBody`'s own doc for why this exists (cross-file/cross-
    /// language duplicate BUSINESS LOGIC, not just duplicate declarations).
    #[serde(default)]
    pub function_bodies: Vec<FunctionBody>,
    /// Per-file extraction cache — same shape as `Index::file_facts`.
    #[serde(default)]
    pub file_facts: BTreeMap<String, StructuralFileFacts>,
    /// Version of the structural extractor. Bump to invalidate all caches.
    #[serde(default)]
    pub extractor_version: u32,
    /// Workspace package mapping: package-name → local directory path.
    /// Built once at extract time from package.json/Cargo.toml/pnpm-workspace.yaml.
    /// Used to resolve `import { X } from "@myorg/core-models"` to a local path
    /// instead of treating it as an external node_modules dependency.
    #[serde(default)]
    pub workspace_packages: BTreeMap<String, String>,
    /// Call edges extracted from function bodies: caller → callee.
    /// Extends structural_dependents beyond import-graph hops.
    #[serde(default)]
    pub call_edges: Vec<CallEdge>,
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
    #[serde(default)]
    pub route_calls: Vec<RouteCall>,
    #[serde(default)]
    pub function_bodies: Vec<FunctionBody>,
    #[serde(default)]
    pub call_edges: Vec<CallEdge>,
}

/// Bump this when the structural extractors change semantics. Invalidates
/// all per-file structural caches, same as EXTRACTOR_VERSION in scan.rs.
///
/// Bumped to 2: added Swift/Objective-C/C-C++/Scala/Dart/Haskell/Clojure
/// extractors and Django route recognition in extract_py — a checked-in
/// validation corpus's cached archietect.db predates both and would otherwise
/// keep reporting stale (e.g. zero Django routes) forever via the unchanged
/// (size, mtime) fast path.
pub const STRUCTURAL_EXTRACTOR_VERSION: u32 = 20; // +Terraform (.tf) and Kubernetes (.yaml/.yml) extractors

// ── Public API ───────────────────────────────────────────────────────────────

/// Extract the structural graph for `root`, reusing `prior` for unchanged
/// files. Called from `scan::scan_with_prior` after the schema pass.
pub fn extract(
    root: &std::path::Path,
    files: &[crate::scan::ScannableFile],
    prior: Option<&StructuralGraph>,
) -> StructuralGraph {
    use rayon::prelude::*;

    let workspace_packages = read_workspace_packages(root);

    let prior_facts: BTreeMap<String, StructuralFileFacts> =
        prior.map(|p| p.file_facts.clone()).unwrap_or_default();

    let prior_version_matches =
        prior.map(|p| p.extractor_version == STRUCTURAL_EXTRACTOR_VERSION).unwrap_or(false);

    let results: Vec<(String, u64, i64, Vec<Symbol>, Vec<Import>, Vec<Route>, Vec<RouteCall>, Vec<FunctionBody>, Vec<CallEdge>)> = files
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
                    pf.route_calls.clone(),
                    pf.function_bodies.clone(),
                    pf.call_edges.clone(),
                );
            }

            let Ok(text) = std::fs::read_to_string(&f.path) else {
                return (f.rel.clone(), f.size, f.mtime_ms, Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
            };

            let ext = f.path.extension().and_then(|x| x.to_str()).unwrap_or("");
            let (symbols, imports, routes) = extract_file(&f.rel, ext, &text);
            let mut route_calls = Vec::new();
            extract_route_calls(&f.rel, ext, &text, &mut route_calls);
            let mut function_bodies = Vec::new();
            extract_function_bodies(&f.rel, ext, &text, &mut function_bodies);
            let mut call_edges = Vec::new();
            if ext == "rs" {
                extract_call_edges_rs(&f.rel, &text, &mut call_edges);
            } else if matches!(ext, "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts") {
                extract_call_edges_ts(&f.rel, &text, &mut call_edges);
            }
            (f.rel.clone(), f.size, f.mtime_ms, symbols, imports, routes, route_calls, function_bodies, call_edges)
        })
        .collect();

    let mut graph = StructuralGraph {
        extractor_version: STRUCTURAL_EXTRACTOR_VERSION,
        workspace_packages,
        ..Default::default()
    };

    for (rel, size, mtime_ms, symbols, imports, routes, route_calls, function_bodies, call_edges) in results {
        graph.file_facts.insert(
            rel.clone(),
            StructuralFileFacts {
                size, mtime_ms,
                symbols: symbols.clone(), imports: imports.clone(), routes: routes.clone(),
                route_calls: route_calls.clone(), function_bodies: function_bodies.clone(),
                call_edges: call_edges.clone(),
            },
        );
        for s in &symbols {
            graph.symbols.insert(format!("{}::{}", rel, s.name), s.clone());
        }
        graph.imports.extend(imports);
        graph.routes.extend(routes);
        graph.route_calls.extend(route_calls);
        graph.function_bodies.extend(function_bodies);
        graph.call_edges.extend(call_edges);
    }

    // ── dbt manifest.json — data lineage as structural symbols + edges ────
    // Injected here, after the per-file pass, for two reasons:
    // 1. dbt's manifest.json is a single compiled artifact (target/manifest.json
    //    or dbt/manifest.json), not a per-file source — there's nothing to
    //    cache per-file or invalidate per-mtime in the normal sense.
    // 2. The resulting symbols and imports flow through graph.symbols /
    //    graph.imports, which means `impact`, `claim --type isolation`, and
    //    `structural_dependents` all work for dbt models for free — no new
    //    query logic needed.
    //
    // `#[serde(default)]` on StructuralGraph's fields means an old
    // archietect.db that predates this has no dbt entries — correct, since
    // that DB was written before dbt support existed. A fresh `archietect init`
    // (or `--refresh`) on a dbt project will pick them up.
    ingest_dbt_manifest(root, &mut graph);

    graph
}

/// Scan dbt's compiled `manifest.json` (written by `dbt compile` or `dbt run`
/// to `target/manifest.json`, or at `dbt/manifest.json` in some monorepo
/// layouts) and inject dbt models as `Symbol` nodes and `Import` edges into
/// the structural graph.
///
/// Why `SymbolKind::Class` for a dbt model? It's the closest semantic fit in
/// the existing vocabulary: a dbt model is a named, declared thing with
/// dependents — like a class. `SymbolKind::Function` would imply it's called;
/// `SymbolKind::Route` would imply it's an HTTP endpoint. `Class` is the
/// correct "named declaration with dependents" kind, same choice `extract_py`
/// makes for Django models and `extract_graphql` makes for GraphQL types.
///
/// Edge direction: a dbt model's `depends_on.nodes` lists its UPSTREAM
/// dependencies — `stg_orders` depends on `raw_orders`. In archietect's import
/// model, `from_file` → `to_module` means "from_file uses to_module" (the
/// importer points to the imported). So `stg_orders` → `raw_orders` is an
/// Import where from_file = stg_orders's SQL file, to_module = raw_orders's
/// SQL file. This means `archietect impact raw_orders` correctly surfaces
/// every downstream model that depends on it — the blast radius flows in the
/// right direction.
///
/// Fails silently: if no manifest exists, or parsing fails, or the manifest
/// format doesn't match expectations — the graph is simply unchanged. A dbt
/// project that hasn't been compiled yet (no `target/manifest.json`) will
/// return INSUFFICIENT_COVERAGE for dbt concepts, which is the honest answer,
/// not a crash.
fn ingest_dbt_manifest(root: &std::path::Path, graph: &mut StructuralGraph) {
    // Try the two most common manifest locations.
    let candidates = [
        root.join("target").join("manifest.json"),
        root.join("dbt").join("manifest.json"),
    ];
    let manifest_path = candidates.iter().find(|p| p.exists());
    let manifest_path = match manifest_path {
        Some(p) => p,
        None => return,
    };

    // dbt_project_prefix: the directory containing the dbt project, relative
    // to the git repo root. When dbt lives in a subdirectory (e.g. /data/,
    // /dbt/), original_file_path values like "models/staging/stg_orders.sql"
    // are relative to THAT directory, not the repo root. Prepending this
    // prefix aligns dbt paths with the paths every other extractor uses.
    //
    // Example: manifest at repo/data/target/manifest.json
    //   original_file_path = "models/stg_orders.sql"
    //   dbt_project_prefix = "data/"
    //   → corrected path   = "data/models/stg_orders.sql"
    //
    // When dbt is at the repo root (target/manifest.json), the dbt project
    // dir IS root — prefix is "" — paths pass through unchanged.
    let dbt_project_prefix: String = manifest_path
        .parent()                               // .../target/ or .../dbt/
        .and_then(|p| p.parent())              // the dbt project root
        .and_then(|p| p.strip_prefix(root).ok())
        .map(|p| {
            let s = p.to_string_lossy().to_string();
            if s.is_empty() { s } else { format!("{}/", s) }
        })
        .unwrap_or_default();

    let text = match std::fs::read_to_string(manifest_path) {
        Ok(t) => t,
        Err(_) => return,
    };
    let manifest: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return,
    };

    let nodes = match manifest.get("nodes").and_then(|n| n.as_object()) {
        Some(n) => n,
        None => return,
    };

    let manifest_rel = manifest_path
        .strip_prefix(root)
        .unwrap_or(manifest_path)
        .to_string_lossy()
        .to_string();

    // Helper: prepend the dbt project prefix to a manifest-relative path.
    let prefix_path = |p: &str| -> String {
        if dbt_project_prefix.is_empty() || p.starts_with(&dbt_project_prefix) {
            p.to_string()
        } else {
            format!("{}{}", dbt_project_prefix, p)
        }
    };

    for (_node_id, node) in nodes {
        let resource_type = node.get("resource_type").and_then(|r| r.as_str()).unwrap_or("");
        if !matches!(resource_type, "model" | "source") {
            continue;
        }

        let name = node.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }

        // Apply project prefix to original_file_path so it is repo-root-
        // relative, matching paths produced by every other extractor.
        let file = node
            .get("original_file_path")
            .and_then(|p| p.as_str())
            .filter(|p| !p.is_empty())
            .map(|p| prefix_path(p))
            .unwrap_or(manifest_rel.clone());

        // Cross-silo linkage: store the database table name as linked_concept
        // so structural_dependents can bridge dbt symbols to schema-layer
        // concepts (ORM models, CREATE TABLE) that declare the same table.
        //
        //   Backend:  User ORM model  → Concept { name: "User", table: "users" }
        //   dbt:      stg_users model → Symbol  { name: "stg_users",
        //                                         linked_concept: Some("users") }
        //
        // With this link: `archietect impact User` walks from the ORM model
        // to the dbt symbol (via table name match in structural_dependents)
        // and then to every downstream dbt mart that depends on stg_users —
        // full cross-silo blast radius.
        //
        // relation_name in dbt manifests: "database"."schema"."table_name"
        // → last segment, unquoted. Absent for sources; fall back to name.
        let linked_concept: Option<String> = node
            .get("relation_name")
            .and_then(|r| r.as_str())
            .and_then(|r| r.split('.').last().map(|s| s.trim_matches('"').to_lowercase()))
            .or_else(|| Some(name.to_lowercase()));

        let sym_key = format!("{}::{}", file, name);
        graph.symbols.insert(
            sym_key,
            Symbol {
                name: name.clone(),
                kind: SymbolKind::Class,
                file: file.clone(),
                linked_concept,
                line: 1,
                observation_source: ObservationSource::Lexical,
            },
        );

        // Upstream dependencies → Import edges, paths prefix-corrected.
        if let Some(deps) = node
            .get("depends_on")
            .and_then(|d| d.get("nodes"))
            .and_then(|n| n.as_array())
        {
            for dep in deps {
                let dep_id = match dep.as_str() {
                    Some(s) => s,
                    None => continue,
                };
                let to_module = manifest["nodes"]
                    .get(dep_id)
                    .and_then(|n| n.get("original_file_path"))
                    .and_then(|p| p.as_str())
                    .filter(|p| !p.is_empty())
                    .map(|p| prefix_path(p))
                    .unwrap_or_else(|| dep_id.to_string());

                graph.imports.push(Import {
                    from_file: file.clone(),
                    to_module,
                    names: vec![],
                });
            }
        }
    }
}
/// Handles: npm/pnpm/yarn (package.json workspaces), Cargo (Cargo.toml [workspace]),
/// and pnpm-workspace.yaml. Returns empty map if no workspace file found.
///
/// This is what lets `import { X } from "@myorg/core-models"` resolve to
/// `packages/core-models/src/index.ts` instead of disappearing into the
/// node_modules void.
fn read_workspace_packages(root: &std::path::Path) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();

    // ── npm / pnpm / yarn: package.json workspaces ────────────────────────
    let pkg_json = root.join("package.json");
    if let Ok(text) = std::fs::read_to_string(&pkg_json) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
            // workspaces: ["packages/*"] or workspaces.packages: ["packages/*"]
            let patterns = val["workspaces"]
                .as_array()
                .cloned()
                .or_else(|| val["workspaces"]["packages"].as_array().cloned())
                .unwrap_or_default();

            for pattern in patterns {
                let Some(pat) = pattern.as_str() else { continue };
                // Expand glob patterns like "packages/*" or "apps/*"
                let prefix = pat.trim_end_matches("/*").trim_end_matches('*');
                if let Ok(entries) = std::fs::read_dir(root.join(prefix)) {
                    for entry in entries.flatten() {
                        let dir = entry.path();
                        let member_pkg = dir.join("package.json");
                        if let Ok(mpkg) = std::fs::read_to_string(&member_pkg) {
                            if let Ok(mv) = serde_json::from_str::<serde_json::Value>(&mpkg) {
                                if let Some(name) = mv["name"].as_str() {
                                    let rel = dir
                                        .strip_prefix(root)
                                        .unwrap_or(&dir)
                                        .to_string_lossy()
                                        .to_string();
                                    map.insert(name.to_string(), rel);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ── Cargo workspaces: Cargo.toml [workspace] members ─────────────────
    let cargo_toml = root.join("Cargo.toml");
    if let Ok(text) = std::fs::read_to_string(&cargo_toml) {
        if let Ok(val) = text.parse::<toml::Value>() {
            if let Some(members) = val.get("workspace")
                .and_then(|w| w.get("members"))
                .and_then(|m| m.as_array())
            {
                for member in members {
                    let Some(pat) = member.as_str() else { continue };
                    let prefix = pat.trim_end_matches("/*").trim_end_matches('*');
                    if let Ok(entries) = std::fs::read_dir(root.join(prefix)) {
                        for entry in entries.flatten() {
                            let dir = entry.path();
                            let member_toml = dir.join("Cargo.toml");
                            if let Ok(mtext) = std::fs::read_to_string(&member_toml) {
                                if let Ok(mv) = mtext.parse::<toml::Value>() {
                                    if let Some(name) = mv.get("package")
                                        .and_then(|p| p.get("name"))
                                        .and_then(|n| n.as_str())
                                    {
                                        let rel = dir
                                            .strip_prefix(root)
                                            .unwrap_or(&dir)
                                            .to_string_lossy()
                                            .to_string();
                                        map.insert(name.to_string(), rel);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    map
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
    //
    // Two ways a symbol can own a concept, and the second was missing until
    // it was found live: (a) `linked_concept` — the symbol was linked to a
    // SCHEMA-layer concept by `link_to_concepts`; (b) the symbol's own name
    // IS the concept — a plain class/function with no schema model behind
    // it. Case (b) is every STRUCTURAL-verdict concept there is (archietect's
    // own `structural_dependents` fn, or a plain class with no schema model):
    // `linked_concept` is None for all of them, so `owner_files` came back
    // empty and this function returned before walking a single import edge —
    // reporting "nothing touches it" for symbols that were, in fact, imported
    // and called. The import graph was never the problem; it was never being
    // entered for exactly the symbols wiring questions are about.
    let owner_files: std::collections::HashSet<String> = graph
        .symbols
        .values()
        .filter(|s| s.linked_concept.as_deref() == Some(concept_name) || s.name == concept_name)
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

    // Build a reverse call index: callee_name → (from_file, caller_name).
    // Used to extend reachability beyond import-graph hops — a file that
    // calls a symbol in an owner file is a dependent even if it never
    // imports the owner file directly.
    let mut reverse_calls: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for edge in &graph.call_edges {
        reverse_calls.entry(edge.callee.clone())
            .or_default()
            .push((edge.from_file.clone(), edge.caller.clone()));
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
                    // Symbol-name imports (Terraform, Kubernetes, dbt): the
                    // to_module IS the resource/model name, and the owner file
                    // declares a symbol with that exact name. Match when any
                    // symbol in the target file has a name that the to_module
                    // references — enables blast-radius across IaC resources
                    // without changing how extractors write import edges.
                    || graph.symbols.values().any(|s| {
                        targets.contains(&s.file)
                            && (imp.to_module == s.name
                                || imp.to_module.ends_with(&format!(".{}", s.name)))
                    })
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

    // Collect the concept's own symbol names for call-edge lookup
    let concept_symbols: std::collections::HashSet<String> = graph
        .symbols
        .values()
        .filter(|s| s.linked_concept.as_deref() == Some(concept_name) || s.name == concept_name)
        .map(|s| s.name.clone())
        .collect();

    for depth in 1..=depth_limit {
        let mut next = importers_of(&frontier);

        // Also add files that call any of the concept's symbols directly
        // (call-edge reachability — catches callers that don't import the
        // owner file explicitly, e.g. dependency-injection containers)
        for sym_name in &concept_symbols {
            if let Some(callers) = reverse_calls.get(sym_name) {
                for (from_file, _caller) in callers {
                    if !seen.contains(from_file) {
                        next.insert(from_file.clone());
                    }
                }
            }
        }

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

/// Return all routes in `graph` that are linked to `concept_name`. Via
/// `Route::relationship_to` (see resource.rs::Relationship /
/// SYSTEM_MEMORY.md) — same predicate as before, now an explicit evidenced
/// edge instead of an inline, disposable boolean.
pub fn routes_for_concept<'a>(
    graph: &'a StructuralGraph,
    concept_name: &str,
) -> Vec<&'a Route> {
    graph
        .routes
        .iter()
        .filter(|r| r.relationship_to(concept_name).is_some())
        .collect()
}

/// One real, load-bearing call this project's own import-graph walk
/// (`structural_dependents`) can never find: a caller in a DIFFERENT file,
/// commonly a different language, reaching a route over HTTP rather than an
/// import + function call. See `RouteCall`'s own doc for the full story —
/// this is where that evidence actually gets cross-referenced against
/// `concept_name`'s declared routes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteCallDependent {
    pub file: String,
    pub method: String,
    pub path: String,
    /// The declared route's own path — often not byte-identical to `path`
    /// (different path-parameter names/delimiters; see `paths_match`), so
    /// showing both is what lets a human confirm the match makes sense
    /// rather than trusting a silent normalization.
    pub matched_route: String,
}

/// Every file that calls one of `concept_name`'s declared routes over HTTP,
/// per `RouteCall`'s path-matching rules. Independent of
/// `structural_dependents`'s import-graph walk — this finds exactly the
/// callers that walk can't, and only those; a caller reachable by both
/// signals shows up in both, which is fine, not a duplicate to dedupe away
/// (they're different evidence, arrived at differently).
pub fn route_call_dependents(graph: &StructuralGraph, concept_name: &str) -> Vec<RouteCallDependent> {
    let routes = routes_for_concept(graph, concept_name);
    if routes.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for call in &graph.route_calls {
        for route in &routes {
            if paths_match(&route.path, &call.path) {
                out.push(RouteCallDependent {
                    file: call.file.clone(),
                    method: route.method.clone(),
                    path: call.path.clone(),
                    matched_route: route.path.clone(),
                });
            }
        }
    }
    out.sort_by(|a, b| a.file.cmp(&b.file).then(a.path.cmp(&b.path)));
    out.dedup_by(|a, b| a.file == b.file && a.path == b.path);
    out
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

/// Dispatch wrapper for the syn-based Rust extractor so it can be stored in
/// the LANGUAGES function-pointer table (same signature as all other
/// extractors). The actual implementation lives in `extract_rs_syn`.
fn extract_rs_syn_dispatch(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    routes: &mut Vec<Route>,
) {
    extract_rs_syn(rel, text, symbols, imports, routes);
}

pub const LANGUAGES: &[LanguageSpec] = &[
    LanguageSpec {
        name: "Rust",
        extensions: &["rs"],
        extractor: extract_rs_syn_dispatch,
        symbol_support: "structs, enums, traits, top-level functions (AST-verified via syn; \
                          falls back to lexical extraction if the file does not parse)",
        // Axum's `.route("/path", get(handler))` builder pattern also
        // registers a WebSocket upgrade handler (a WS endpoint is an
        // ordinary handler at an ordinary route in Axum) — one extractor
        // covers both without treating WS as a special case. Warp's own
        // combinator-style routing (`warp::path(...).and(warp::get())...`)
        // is deliberately NOT attempted, same stated reason this project
        // already excludes Servant/Akka HTTP: "a wrong route is worse than
        // a missing one," and warp's routes are built by composing
        // arbitrary filter chains with no fixed textual shape to anchor on.
        frameworks: &["Axum", "Actix-web", "Rocket"],
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
        extensions: &["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts"],
        extractor: extract_ts_js,
        symbol_support: "classes, interfaces, type aliases, enums, exported and unexported-PascalCase functions, routes, events",
        frameworks: &["Express", "NestJS", "Next.js", "Nuxt (server/api)", "Angular (router)"],
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
        extensions: &["rb", "jbuilder"],
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
    LanguageSpec {
        name: "Gherkin",
        extensions: &["feature"],
        extractor: extract_gherkin,
        symbol_support: "Feature and Scenario/Scenario Outline names",
        frameworks: &["Cucumber"],
    },
    LanguageSpec {
        name: "GDScript",
        extensions: &["gd"],
        extractor: extract_gdscript,
        symbol_support: "class_name declarations (falling back to the PascalCase filename for a script with none, the same convention extract_vue already uses — most GDScript files attach to a node with no explicit class_name), top-level functions, signals",
        frameworks: &[],
    },
    LanguageSpec {
        name: "Godot Scene",
        extensions: &["tscn"],
        extractor: extract_tscn,
        symbol_support: "the scene itself as a component (PascalCase filename, same convention as GDScript's own class_name-less fallback), plus its ext_resource dependencies (attached script, composed child scenes) as import edges, with the specific instancing node name(s) recorded for a composed child scene",
        frameworks: &[],
    },
    LanguageSpec {
        name: "Godot Project Config",
        extensions: &["godot"],
        extractor: extract_project_godot,
        symbol_support: "[autoload] global singleton registrations — the only authoritative source of an autoload script's global name, since a Godot 4 script that declares both class_name and an autoload registration of the same name is a hard parse error, so autoload scripts deliberately have no class_name for extract_gdscript to fall back on",
        frameworks: &[],
    },
    LanguageSpec {
        name: "Terraform",
        extensions: &["tf"],
        extractor: extract_terraform,
        symbol_support: "resource, data, and module blocks as named symbols; interpolation references (type.name.attr) as import edges",
        frameworks: &[],
    },
    LanguageSpec {
        name: "Kubernetes Manifest",
        // .yaml/.yml are normally in NON_CODE_EXTS; for Kubernetes we gate
        // on content (must contain `apiVersion:` and `kind:`) inside the
        // extractor, so non-Kubernetes YAML files produce zero symbols and
        // are effectively skipped. Removing from NON_CODE_EXTS means they
        // are walked — the content gate stops them contributing noise.
        extensions: &["yaml", "yml"],
        extractor: extract_kubernetes,
        symbol_support: "Kubernetes resources as {Kind}.{metadata.name} symbols; cross-resource references (configMapRef, secretRef, serviceAccountName, claimName) as import edges",
        frameworks: &[],
    },
];

/// Languages Archietect can identify by extension but has NO extractor for —
/// listed explicitly so the coverage report can say "present, unsupported"
/// instead of silently omitting them. A language absent from BOTH tables is
/// simply not something this list anticipated; the report says so too.
pub const KNOWN_UNSUPPORTED: &[(&str, &[&str])] = &[];

/// Per-language, per-framework structural coverage for the files actually
/// present in this scan — the honest answer to "does Archietect understand
/// this repo," instead of letting a user discover the boundary one UNKNOWN
/// concept query at a time.
pub fn coverage_report(idx: &crate::model::Index, graph: &StructuralGraph) -> serde_json::Value {
    let mut ext_counts: BTreeMap<String, usize> = BTreeMap::new();
    for rel in graph.file_facts.keys() {
        if let Some(ext) = std::path::Path::new(rel).extension().and_then(|x| x.to_str()) {
            *ext_counts.entry(ext.to_lowercase()).or_default() += 1;
        }
    }
    // Same fix as query::concept()'s INSUFFICIENT_COVERAGE path: a language
    // in neither LANGUAGES nor KNOWN_UNSUPPORTED never entered file_facts at
    // all, so it's otherwise invisible even to this report. Live walk,
    // extension-only, no content reads.
    let mut unclassified_counts: BTreeMap<String, usize> = BTreeMap::new();
    for (_rel, ext) in crate::scan::unclassified_files(std::path::Path::new(&idx.root), &idx.excludes, 5000) {
        *unclassified_counts.entry(ext).or_default() += 1;
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

    let mut unsupported: Vec<serde_json::Value> = KNOWN_UNSUPPORTED
        .iter()
        .filter_map(|(name, exts)| {
            let files: usize = exts.iter().filter_map(|e| ext_counts.get(*e)).sum();
            (files > 0).then(|| serde_json::json!({ "language": name, "files": files }))
        })
        .collect();
    unsupported.extend(unclassified_counts.iter().map(|(ext, files)| {
        serde_json::json!({ "language": format!(".{ext} (unclassified)"), "files": files })
    }));

    serde_json::json!({
        "supported": supported,
        "present_but_unsupported": unsupported,
        "note": "A file in an 'unsupported' language contributes no structural symbols or routes — a concept query for something implemented only there gets no STRUCTURAL evidence and will not guess from its filename alone (it returns INSUFFICIENT_COVERAGE instead). A 'supported' language's symbol_support/frameworks_recognized lists are exactly what is and isn't extracted — a framework not listed there (e.g. Django's urls.py) produces no Route even in a supported language. '(unclassified)' entries are extensions nobody has categorized as code OR as a known non-code format at all — possibly a real language Archietect has simply never seen before.",
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
    let lang = if rel.ends_with(".ts") || rel.ends_with(".tsx") || rel.ends_with(".mts") || rel.ends_with(".cts") {
        tree_sitter_typescript::language_typescript()
    } else {
        tree_sitter_javascript::language()
    };

    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(lang).is_ok() {
        if let Some(tree) = parser.parse(text, None) {
            let bytes = text.as_bytes();
            let root = tree.root_node();
            walk_ts_js(&root, bytes, rel, symbols, imports, 0);
        }
    }

    // Framework route extraction still uses regex — these are string-literal
    // patterns inside decorator/call arguments, which tree-sitter sees as
    // string nodes. TODO: migrate to tree-sitter argument-node extraction.
    extract_ts_routes(rel, text, routes);
    extract_ts_events(rel, text, symbols);
}

fn walk_ts_js(
    node: &tree_sitter::Node,
    bytes: &[u8],
    rel: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    depth: usize,
) {
    match node.kind() {
        "class_declaration" | "abstract_class_declaration" => {
            if let Some(n) = node.child_by_field_name("name") {
                symbols.push(Symbol {
                    name: ts_text(n, bytes),
                    kind: SymbolKind::Class,
                    file: rel.to_string(),
                    linked_concept: None,
                    line: n.start_position().row + 1,
                    observation_source: ObservationSource::Ast,
                });
            }
        }
        "interface_declaration" => {
            if let Some(n) = node.child_by_field_name("name") {
                symbols.push(Symbol {
                    name: ts_text(n, bytes),
                    kind: SymbolKind::Interface,
                    file: rel.to_string(),
                    linked_concept: None,
                    line: n.start_position().row + 1,
                    observation_source: ObservationSource::Ast,
                });
            }
        }
        "type_alias_declaration" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = ts_text(n, bytes);
                // PascalCase only — same convention as the old regex
                if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                    symbols.push(Symbol {
                        name,
                        kind: SymbolKind::Class,
                        file: rel.to_string(),
                        linked_concept: None,
                        line: n.start_position().row + 1,
                        observation_source: ObservationSource::Ast,
                    });
                }
            }
        }
        "enum_declaration" => {
            if let Some(n) = node.child_by_field_name("name") {
                symbols.push(Symbol {
                    name: ts_text(n, bytes),
                    kind: SymbolKind::Class,
                    file: rel.to_string(),
                    linked_concept: None,
                    line: n.start_position().row + 1,
                    observation_source: ObservationSource::Ast,
                });
            }
        }
        "function_declaration" => {
            if depth == 0 {
                if let Some(n) = node.child_by_field_name("name") {
                    let name = ts_text(n, bytes);
                    // PascalCase only — lowercase functions are private helpers
                    if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                        symbols.push(Symbol {
                            name,
                            kind: SymbolKind::Function,
                            file: rel.to_string(),
                            linked_concept: None,
                            line: n.start_position().row + 1,
                            observation_source: ObservationSource::Ast,
                        });
                    }
                }
            }
        }
        "lexical_declaration" | "variable_declaration" if depth == 0 => {
            // export const Foo = (...) => ... or export const Foo = { ... }
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    let ch = cursor.node();
                    if ch.kind() == "variable_declarator" {
                        if let Some(name_node) = ch.child_by_field_name("name") {
                            let name = ts_text(name_node, bytes);
                            if let Some(val) = ch.child_by_field_name("value") {
                                let val_kind = val.kind();
                                if val_kind == "object" {
                                    // PascalCase only for object namespaces
                                    if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                                        symbols.push(Symbol {
                                            name,
                                            kind: SymbolKind::Class,
                                            file: rel.to_string(),
                                            linked_concept: None,
                                            line: name_node.start_position().row + 1,
                                            observation_source: ObservationSource::Ast,
                                        });
                                    }
                                } else if matches!(val_kind, "arrow_function" | "function") {
                                    // PascalCase only — lowercase consts are helpers
                                    if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                                        symbols.push(Symbol {
                                            name,
                                            kind: SymbolKind::Function,
                                            file: rel.to_string(),
                                            linked_concept: None,
                                            line: name_node.start_position().row + 1,
                                            observation_source: ObservationSource::Ast,
                                        });
                                    }
                                }
                                // require() on the RHS is handled by the call_expression arm below
                            }
                        } else if let Some(pat_node) = ch.child_by_field_name("name") {
                            // Destructured: const { createApp } = require('./app')
                            // name field is an object_pattern here
                            let _ = pat_node; // handled in call_expression arm
                        }
                    }
                    if !cursor.goto_next_sibling() { break; }
                }
            }
        }
        // require() calls — handles const x = require(), const { x } = require(), require() bare
        "call_expression" if depth == 0 => {
            let func = node.child_by_field_name("function");
            let is_require = func.map(|f| ts_text(f, bytes) == "require").unwrap_or(false);
            if is_require {
                if let Some(args) = node.child_by_field_name("arguments") {
                    // Extract the module path from the first string argument
                    let mut module = String::new();
                    let mut ac = args.walk();
                    if ac.goto_first_child() {
                        loop {
                            let a = ac.node();
                            if a.kind() == "string" {
                                module = ts_text(a, bytes)
                                    .trim_matches(|c| c == '\'' || c == '"')
                                    .to_string();
                                break;
                            }
                            if !ac.goto_next_sibling() { break; }
                        }
                    }
                    if !module.is_empty() {
                        // Walk up to find the variable_declarator that contains this require()
                        // to extract the binding names
                        let names = extract_require_names(node, bytes);
                        imports.push(Import { from_file: rel.to_string(), to_module: module, names });
                    }
                }
            }
        }
        "import_statement" => {
            // import { X, Y } from './module' or import X from './module'
            let source = node.child_by_field_name("source")
                .map(|n| ts_text(n, bytes).trim_matches(|c| c == '\'' || c == '"').to_string())
                .unwrap_or_default();
            let mut names = Vec::new();
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    let ch = cursor.node();
                    match ch.kind() {
                        "import_clause" => {
                            let mut ic = ch.walk();
                            if ic.goto_first_child() {
                                loop {
                                    let item = ic.node();
                                    match item.kind() {
                                        "identifier" => names.push(ts_text(item, bytes)),
                                        "named_imports" => {
                                            let mut ni = item.walk();
                                            if ni.goto_first_child() {
                                                loop {
                                                    let spec = ni.node();
                                                    if spec.kind() == "import_specifier" {
                                                        if let Some(n) = spec.child_by_field_name("name") {
                                                            names.push(ts_text(n, bytes));
                                                        }
                                                    }
                                                    if !ni.goto_next_sibling() { break; }
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                    if !ic.goto_next_sibling() { break; }
                                }
                            }
                        }
                        _ => {}
                    }
                    if !cursor.goto_next_sibling() { break; }
                }
            }
            if !source.is_empty() {
                imports.push(Import { from_file: rel.to_string(), to_module: source, names });
            }
        }
        _ => {}
    }

    let child_depth = if matches!(node.kind(), "class_declaration" | "function_declaration" | "arrow_function" | "function") {
        depth + 1
    } else {
        depth
    };
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            walk_ts_js(&cursor.node(), bytes, rel, symbols, imports, child_depth);
            if !cursor.goto_next_sibling() { break; }
        }
    }
}

/// Given a `call_expression` node that is `require(...)`, walk up to its
/// containing `variable_declarator` and extract the binding name(s):
///   - `const { createApp } = require(...)` → ["createApp"]
///   - `const { Router: createRouter } = require(...)` → ["Router"] (key, not local)
///   - `const app = require(...)` → [] (default import, no named bindings)
///   - `require(...)` bare (no declarator) → []
fn extract_require_names(require_call: &tree_sitter::Node, bytes: &[u8]) -> Vec<String> {
    // Walk up: call_expression → variable_declarator → (name field)
    let declarator = match require_call.parent().and_then(|p| {
        if p.kind() == "variable_declarator" { Some(p) } else { None }
    }) {
        Some(d) => d,
        None => return Vec::new(), // bare require()
    };

    let name_node = match declarator.child_by_field_name("name") {
        Some(n) => n,
        None => return Vec::new(),
    };

    match name_node.kind() {
        "object_pattern" => {
            // { createApp } or { Router: createRouter }
            let mut names = Vec::new();
            let mut c = name_node.walk();
            if c.goto_first_child() {
                loop {
                    let ch = c.node();
                    if ch.kind() == "shorthand_property_identifier_pattern" {
                        // { createApp }
                        names.push(ts_text(ch, bytes));
                    } else if ch.kind() == "pair_pattern" {
                        // { Router: createRouter } — take the KEY
                        if let Some(key) = ch.child_by_field_name("key") {
                            names.push(ts_text(key, bytes));
                        }
                    }
                    if !c.goto_next_sibling() { break; }
                }
            }
            names
        }
        "identifier" => {
            // const app = require(...) — default-style, no named bindings
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn extract_ts_routes(rel: &str, text: &str, routes: &mut Vec<Route>) {
    let lang = if rel.ends_with(".ts") || rel.ends_with(".tsx") {
        tree_sitter_typescript::language_typescript()
    } else {
        tree_sitter_javascript::language()
    };

    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(lang).is_ok() {
        if let Some(tree) = parser.parse(text, None) {
            let bytes = text.as_bytes();
            // Pass 1: collect string constants (const API_PREFIX = "/v1")
            let mut string_consts: BTreeMap<String, String> = BTreeMap::new();
            collect_string_consts(&tree.root_node(), bytes, &mut string_consts);
            // Pass 2: walk for route declarations, resolving known constants
            walk_ts_routes(&tree.root_node(), bytes, rel, routes, &string_consts, None);
        }
    }

    // File-based and tagged-template routes — not expressible as call_expression
    // patterns, so these helpers stay (they look at filename and template tags).
    next_app_router_routes(rel, text, routes);
    nuxt_server_api_route(rel, routes);
    graphql_tagged_template_operations(rel, text, routes);
    angular_routes(rel, text, routes);
}

/// Collect top-level `const NAME = "/string"` declarations for route prefix resolution.
fn collect_string_consts(node: &tree_sitter::Node, bytes: &[u8], out: &mut BTreeMap<String, String>) {
    if node.kind() == "lexical_declaration" || node.kind() == "variable_declaration" {
        let mut c = node.walk();
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if ch.kind() == "variable_declarator" {
                    if let (Some(name_n), Some(val_n)) = (
                        ch.child_by_field_name("name"),
                        ch.child_by_field_name("value"),
                    ) {
                        if matches!(val_n.kind(), "string" | "template_string") {
                            let name = ts_text(name_n, bytes);
                            let val = ts_text(val_n, bytes)
                                .trim_matches(|c| c == '\'' || c == '"' || c == '`')
                                .to_string();
                            if !name.is_empty() && !val.is_empty() {
                                out.insert(name, val);
                            }
                        }
                    }
                }
                if !c.goto_next_sibling() { break; }
            }
        }
    }
    let mut c = node.walk();
    if c.goto_first_child() {
        loop {
            collect_string_consts(&c.node(), bytes, out);
            if !c.goto_next_sibling() { break; }
        }
    }
}

/// Resolve a route path argument node to a string, substituting known constants.
/// Handles: string literals, template literals with `${VAR}` substitution.
fn resolve_route_path(node: &tree_sitter::Node, bytes: &[u8], consts: &BTreeMap<String, String>) -> Option<String> {
    match node.kind() {
        "string" => {
            let raw = ts_text(*node, bytes);
            Some(raw.trim_matches(|c| c == '\'' || c == '"').to_string())
        }
        "template_string" => {
            // Walk template parts, substituting ${IDENTIFIER} with known const values
            let raw = ts_text(*node, bytes);
            // Simple substitution: replace ${VAR} with known const
            let mut result = raw.trim_matches('`').to_string();
            let sub_re = Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").unwrap();
            let mut any_unresolved = false;
            let resolved = sub_re.replace_all(&result, |caps: &regex::Captures| {
                let var_name = &caps[1];
                if let Some(val) = consts.get(var_name) {
                    val.clone()
                } else {
                    any_unresolved = true;
                    format!("${{{var_name}}}") // keep unresolved as-is
                }
            });
            result = resolved.to_string();
            // Only return if meaningful (contains at least a slash)
            if result.contains('/') { Some(result) } else { None }
        }
        "identifier" => {
            // Plain const reference: path = API_PREFIX
            consts.get(&ts_text(*node, bytes)).cloned()
        }
        "binary_expression" => {
            // String concatenation: PREFIX + "/users"
            let left = node.child_by_field_name("left");
            let right = node.child_by_field_name("right");
            let op = node.children(&mut node.walk())
                .find(|n| n.kind() == "+")
                .map(|_| "+");
            if op.is_some() {
                if let (Some(l), Some(r)) = (left, right) {
                    let lv = resolve_route_path(&l, bytes, consts)?;
                    let rv = resolve_route_path(&r, bytes, consts)?;
                    return Some(format!("{lv}{rv}"));
                }
            }
            None
        }
        _ => None,
    }
}

const TS_ROUTE_METHODS: &[&str] = &["get", "post", "put", "delete", "patch", "options", "head", "all"];
const NESTJS_DECORATORS: &[(&str, &str)] = &[
    ("Get", "GET"), ("Post", "POST"), ("Put", "PUT"), ("Delete", "DELETE"),
    ("Patch", "PATCH"), ("Options", "OPTIONS"), ("Head", "HEAD"), ("All", "ANY"),
];

/// Walk the AST looking for:
///   - Express/Fastify: app.get("/path", handler) / router.post("/path", ...)
///   - NestJS decorators: @Get("/path") above a method
fn walk_ts_routes(
    node: &tree_sitter::Node,
    bytes: &[u8],
    rel: &str,
    routes: &mut Vec<Route>,
    consts: &BTreeMap<String, String>,
    _pending_decorator: Option<(&str, String)>, // reserved for future decorator→method linking
) {
    match node.kind() {
        "call_expression" => {
            // Express/Fastify: app.get("/path", handler) or router.post("/path", handler)
            // AST: call_expression → function: member_expression { object, property }
            //                      → arguments: arguments
            if let Some(func) = node.child_by_field_name("function") {
                if func.kind() == "member_expression" {
                    let prop = func.child_by_field_name("property")
                        .map(|n| ts_text(n, bytes))
                        .unwrap_or_default();
                    let prop_lower = prop.to_lowercase();
                    if TS_ROUTE_METHODS.contains(&prop_lower.as_str()) {
                        if let Some(args) = node.child_by_field_name("arguments") {
                            // First argument: the route path — string, template, identifier, or concatenation
                            let mut path: Option<String> = None;
                            let mut handler = "express-handler".to_string();
                            let mut arg_cursor = args.walk();
                            let mut arg_idx = 0;
                            if arg_cursor.goto_first_child() {
                                loop {
                                    let arg = arg_cursor.node();
                                    if arg_idx == 0 {
                                        if let Some(p) = resolve_route_path(&arg, bytes, consts) {
                                            path = Some(p);
                                            arg_idx += 1;
                                        } else if !matches!(arg.kind(), "," | "(" | ")") {
                                            arg_idx += 1; // skip non-string first arg
                                        }
                                    } else if arg_idx == 1 {
                                        if arg.kind() == "identifier" {
                                            handler = ts_text(arg, bytes);
                                        } else if matches!(arg.kind(), "arrow_function" | "function") {
                                            handler = prop_lower.clone();
                                        }
                                        arg_idx += 1;
                                    }
                                    if !arg_cursor.goto_next_sibling() { break; }
                                }
                            }
                            if let Some(p) = path {
                                routes.push(Route {
                                    method: prop.to_uppercase(),
                                    path: p,
                                    handler,
                                    file: rel.to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }
        "decorator" => {
            // NestJS: @Get("/path") or @Post("/path")
            // AST: decorator → call_expression → function: identifier + arguments
            let dec_text = ts_text(*node, bytes);
            for (name, method) in NESTJS_DECORATORS {
                let prefix = format!("@{name}(");
                if dec_text.starts_with(&prefix) || dec_text.starts_with(&format!("@{name} (")) {
                    let mut dec_cursor = node.walk();
                    if dec_cursor.goto_first_child() {
                        loop {
                            let ch = dec_cursor.node();
                            if ch.kind() == "call_expression" {
                                if let Some(args) = ch.child_by_field_name("arguments") {
                                    let mut ac = args.walk();
                                    if ac.goto_first_child() {
                                        loop {
                                            let a = ac.node();
                                            if let Some(path) = resolve_route_path(&a, bytes, consts) {
                                                routes.push(Route {
                                                    method: method.to_string(),
                                                    path,
                                                    handler: "nestjs-handler".to_string(),
                                                    file: rel.to_string(),
                                                });
                                                break;
                                            }
                                            if !ac.goto_next_sibling() { break; }
                                        }
                                    }
                                }
                            }
                            if !dec_cursor.goto_next_sibling() { break; }
                        }
                    }
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            walk_ts_routes(&cursor.node(), bytes, rel, routes, consts, None);
            if !cursor.goto_next_sibling() { break; }
        }
    }
}

/// Angular's `@angular/router` route table: `{ path: 'orders', component:
/// OrdersComponent }` object literals inside a `Routes`-typed array. Found
/// investigating a real Angular SPA — Angular had NO framework recognition
/// at all before this (its own class/interface/type/enum symbols were
/// already caught by the generic patterns above, but the actual navigation
/// graph — which path renders which component — was invisible, the same
/// class of gap Rust's total absence of route recognition was before that
/// got fixed).
///
/// `path` and `component` can appear in either order within one object
/// literal (Angular's own docs show both), so this tries both orders; each
/// is bounded with `[^{}]*?` (non-greedy, no brace crossing) so it can't
/// walk into a SIBLING route object or a nested `data: {...}` block and
/// pair up fields that don't actually belong to the same route. Method is
/// always "ANY" — client-side navigation has no HTTP verb, same honest
/// convention Django/Nuxt's method-less routes already use elsewhere in
/// this file. A route with no `component` at all (lazy-loaded via
/// `loadComponent`/`loadChildren`) is deliberately not attempted here — the
/// target is a dynamic import expression, not a plain identifier, and
/// guessing at it wrongly is worse than reporting nothing for that one
/// route.
fn angular_routes(rel: &str, text: &str, routes: &mut Vec<Route>) {
    let path_then_component = Regex::new(
        r#"\{\s*path\s*:\s*['"]([^'"]*)['"][^{}]*?component\s*:\s*([A-Za-z_][A-Za-z0-9_]*)"#
    ).unwrap();
    let component_then_path = Regex::new(
        r#"\{\s*component\s*:\s*([A-Za-z_][A-Za-z0-9_]*)[^{}]*?path\s*:\s*['"]([^'"]*)['"]"#
    ).unwrap();
    for cap in path_then_component.captures_iter(text) {
        routes.push(Route {
            method: "ANY".to_string(),
            path: cap[1].to_string(),
            handler: cap[2].to_string(),
            file: rel.to_string(),
        });
    }
    for cap in component_then_path.captures_iter(text) {
        routes.push(Route {
            method: "ANY".to_string(),
            path: cap[2].to_string(),
            handler: cap[1].to_string(),
            file: rel.to_string(),
        });
    }
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
            symbols.push(Symbol { name: to_pascal_case(stem), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: 1 , observation_source: ObservationSource::Lexical });
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
            observation_source: ObservationSource::Lexical,
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
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(tree_sitter_python::language()).is_ok() {
        if let Some(tree) = parser.parse(text, None) {
            let bytes = text.as_bytes();
            let root = tree.root_node();
            walk_py(&root, bytes, rel, symbols, imports, routes, 0);
        }
    }
}

fn walk_py(
    node: &tree_sitter::Node,
    bytes: &[u8],
    rel: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    routes: &mut Vec<Route>,
    depth: usize,
) {
    match node.kind() {
        "class_definition" => {
            if let Some(n) = node.child_by_field_name("name") {
                symbols.push(Symbol {
                    name: ts_text(n, bytes),
                    kind: SymbolKind::Class,
                    file: rel.to_string(),
                    linked_concept: None,
                    line: n.start_position().row + 1,
                    observation_source: ObservationSource::Ast,
                });
            }
            // Recurse into class body to catch nested classes; skip method bodies.
        }
        "function_definition" => {
            // Top-level only (depth 0) — methods inside a class body are at depth >= 1.
            if depth == 0 {
                if let Some(n) = node.child_by_field_name("name") {
                    let name = ts_text(n, bytes);
                    // Only lowercase-starting names (PEP-8 functions), not test fixtures.
                    if name.chars().next().map(|c| c.is_lowercase() || c == '_').unwrap_or(false) {
                        symbols.push(Symbol {
                            name,
                            kind: SymbolKind::Function,
                            file: rel.to_string(),
                            linked_concept: None,
                            line: n.start_position().row + 1,
                            observation_source: ObservationSource::Ast,
                        });
                    }
                }
            }
        }
        "import_from_statement" => {
            // from module import X, Y [as Z]
            let module = node.child_by_field_name("module_name")
                .map(|n| ts_text(n, bytes))
                .unwrap_or_default();
            let mut names = Vec::new();
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    let ch = cursor.node();
                    if ch.kind() == "dotted_name" || ch.kind() == "identifier" {
                        // skip the module_name child
                        if Some(ch.id()) != node.child_by_field_name("module_name").map(|n| n.id()) {
                            names.push(ts_text(ch, bytes));
                        }
                    } else if ch.kind() == "aliased_import" {
                        if let Some(n) = ch.child_by_field_name("name") {
                            names.push(ts_text(n, bytes));
                        }
                    }
                    if !cursor.goto_next_sibling() { break; }
                }
            }
            if !module.is_empty() {
                imports.push(Import { from_file: rel.to_string(), to_module: module, names });
            }
        }
        "decorated_definition" => {
            // @app.get("/path") / @router.post(...) — FastAPI/Flask routes
            let mut decorator_method = None;
            let mut decorator_path: Option<String> = None;
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    let ch = cursor.node();
                    if ch.kind() == "decorator" {
                        // Walk the decorator AST: decorator → call → attribute (method) + arguments
                        let mut dc = ch.walk();
                        if dc.goto_first_child() {
                            loop {
                                let inner = dc.node();
                                if inner.kind() == "call" {
                                    // Extract method from function: attribute node
                                    if let Some(func) = inner.child_by_field_name("function") {
                                        if func.kind() == "attribute" {
                                            let attr = func.child_by_field_name("attribute")
                                                .map(|n| ts_text(n, bytes))
                                                .unwrap_or_default();
                                            let attr_lower = attr.to_lowercase();
                                            if matches!(attr_lower.as_str(), "get" | "post" | "put" | "delete" | "patch") {
                                                decorator_method = Some(attr_lower.to_uppercase());
                                            }
                                        }
                                    }
                                    // Extract first string argument as path
                                    if let Some(args) = inner.child_by_field_name("arguments") {
                                        let mut ac = args.walk();
                                        if ac.goto_first_child() {
                                            loop {
                                                let a = ac.node();
                                                if a.kind() == "string" {
                                                    // Strip quotes from Python string
                                                    let raw = ts_text(a, bytes);
                                                    let path = raw.trim_matches(|c| c == '"' || c == '\'')
                                                        .trim_matches(|c| c == '"' || c == '\'')
                                                        .to_string();
                                                    if !path.is_empty() {
                                                        decorator_path = Some(path);
                                                        break;
                                                    }
                                                }
                                                if !ac.goto_next_sibling() { break; }
                                            }
                                        }
                                    }
                                }
                                if !dc.goto_next_sibling() { break; }
                            }
                        }
                    } else if ch.kind() == "function_definition" {
                        if let (Some(method), Some(path)) = (&decorator_method, &decorator_path) {
                            let handler = ch.child_by_field_name("name")
                                .map(|n| ts_text(n, bytes))
                                .unwrap_or_else(|| "unknown".to_string());
                            routes.push(Route {
                                method: method.clone(),
                                path: path.clone(),
                                handler,
                                file: rel.to_string(),
                            });
                        }
                    }
                    if !cursor.goto_next_sibling() { break; }
                }
            }
        }
        _ => {}
    }

    // Recurse — increment depth when entering a class or function body
    let child_depth = if matches!(node.kind(), "class_definition" | "function_definition") {
        depth + 1
    } else {
        depth
    };
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            walk_py(&cursor.node(), bytes, rel, symbols, imports, routes, child_depth);
            if !cursor.goto_next_sibling() { break; }
        }
    }
}

// ── Rust ──────────────────────────────────────────────────────────────────────

fn extract_rs(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    routes: &mut Vec<Route>,
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
            observation_source: ObservationSource::Lexical,
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
            observation_source: ObservationSource::Lexical,
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
            observation_source: ObservationSource::Lexical,
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

    // Rust had NO web-framework route recognition at all before this — an
    // honest, standalone gap regardless of WebSockets: `frameworks: &[]` on
    // this language's own LanguageSpec entry, found while investigating a
    // real reported false negative that turned out to need this first (a
    // Rust service's declared endpoint never became a Route at all, so
    // nothing could ever cross-reference it, independent of whether the
    // CALLING side was ever recognized). Axum's `.route("/path", get(h))`
    // builder pattern doubles as its WebSocket registration too — a
    // `WebSocketUpgrade` extractor is just an ordinary handler function
    // registered the exact same way — so this one pattern covers both REST
    // and WS endpoints in Axum without special-casing WS at all.
    //
    // Bounded to same-statement chained methods (`get(h).post(h2)`) via the
    // nested capture below, not a blind scan to the next `)` — Axum route
    // tables are routinely built by chaining many `.route(...)` calls
    // together, and matching too greedily would blur one route's handler
    // into the next.
    let axum_route_re = Regex::new(
        r#"\.route\s*\(\s*"([^"]+)"\s*,\s*((?:(?:get|post|put|patch|delete)\s*\(\s*[A-Za-z_][A-Za-z0-9_]*\s*\)(?:\s*\.\s*(?:get|post|put|patch|delete)\s*\(\s*[A-Za-z_][A-Za-z0-9_]*\s*\))*))\s*\)"#
    ).unwrap();
    let verb_handler_re = Regex::new(
        r"(get|post|put|patch|delete)\s*\(\s*([A-Za-z_][A-Za-z0-9_]*)\s*\)"
    ).unwrap();
    for cap in axum_route_re.captures_iter(text) {
        let path = cap[1].to_string();
        for vh in verb_handler_re.captures_iter(&cap[2]) {
            routes.push(Route {
                method: vh[1].to_uppercase(),
                path: path.clone(),
                handler: vh[2].to_string(),
                file: rel.to_string(),
            });
        }
    }

    // Actix-web / Rocket attribute-macro routes: `#[get("/path")]` directly
    // above the handler function — same "read the next fn after the match"
    // shape extract_py already uses for FastAPI's decorator, since both are
    // "an annotation names the path; the very next function is the
    // handler" conventions, just Rust attribute syntax instead of a Python
    // decorator.
    let attr_route_re = Regex::new(
        r#"(?m)^\s*#\[\s*(get|post|put|patch|delete)\s*\(\s*"([^"]+)"\s*\)\s*\]"#
    ).unwrap();
    for cap in attr_route_re.captures_iter(text) {
        let after = &text[cap.get(0).unwrap().end()..];
        let fn_re = Regex::new(r"(?m)^\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)").unwrap();
        let handler = fn_re.captures(after).map(|c| c[1].to_string()).unwrap_or_else(|| "unknown".to_string());
        routes.push(Route {
            method: cap[1].to_string().to_uppercase(),
            path: cap[2].to_string(),
            handler,
            file: rel.to_string(),
        });
    }
}

// ── Rust (syn-based) ─────────────────────────────────────────────────────────
//
// Parallel implementation of `extract_rs` using `syn` to parse a real Rust
// AST instead of regex. Used for dogfooding comparison against the regex
// extractor on Archietect's own source files.
//
// Architecture note: the output types are identical to `extract_rs` — Symbol,
// Import, Route. The parser is an implementation detail of observation; the
// memory model doesn't care how the observation was obtained.
//
// What `syn` adds over regex here:
//   - Correct scoping: `pub fn` inside an `impl` block is not top-level and
//     should not be extracted. Regex uses `^pub fn` (line-start anchor) to
//     approximate this, which fails for impl blocks that happen to be at
//     column 0 (e.g., after a macro that dedents). `syn` gives us the real
//     item tree.
//   - `pub(crate)` / `pub(super)` / `pub(in path)` visibility variants are
//     all Visibility::Restricted in syn — we treat them identically to `pub`
//     for the same reason the regex extractor's "public types are declared
//     facts" rationale applies: they're visible outside the defining function.
//   - Accurate line numbers via proc_macro2::Span.
//   - Proc-macro and attribute-macro decorated items (Axum, Actix-web) are
//     still matched by the attribute strings, same as the regex extractor.
//
// What we deliberately keep lexical (regex):
//   - Route extraction: Axum's `.route("/path", get(h))` chain and Actix/
//     Rocket's `#[get("/path")]` attribute are framework conventions, not
//     structural Rust syntax. Matching attribute argument strings with syn
//     would require walking every attribute's token stream — more complex than
//     the existing regex for no accuracy gain on the observations we care about.
//   - `use` imports: `syn` would give us a perfectly parsed UseTree, but the
//     downstream consumer (`Import { from_file, to_module, names }`) only
//     needs the module path and the imported names as strings — the regex
//     already produces exactly that shape, and the only failure mode (nested
//     braces in `use a::{b::{c, d}, e}`) isn't observed in any corpus file.

/// syn visitor that collects top-level public items.
struct RsVisitor<'a> {
    rel: &'a str,
    symbols: &'a mut Vec<Symbol>,
    depth: usize, // tracks nesting depth — top-level = depth 0
}

impl<'a> RsVisitor<'a> {
    fn is_pub(vis: &syn::Visibility) -> bool {
        !matches!(vis, syn::Visibility::Inherited)
    }

    fn line_from_span(&self, span: proc_macro2::Span) -> usize {
        // syn 2.x exposes span().start().line (1-indexed) via the
        // `proc_macro2` span API when built outside a proc-macro context.
        // Fall back to the byte-offset scanner if not available.
        span.start().line
    }
}

impl<'ast, 'a> syn::visit::Visit<'ast> for RsVisitor<'a> {
    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        if self.depth == 0 && Self::is_pub(&node.vis) {
            self.symbols.push(Symbol {
                name: node.ident.to_string(),
                kind: SymbolKind::Class,
                file: self.rel.to_string(),
                linked_concept: None,
                line: self.line_from_span(node.ident.span()),
                observation_source: ObservationSource::Ast,
            });
        }
        // Do NOT recurse — structs don't contain other declaration items.
    }

    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        if self.depth == 0 && Self::is_pub(&node.vis) {
            self.symbols.push(Symbol {
                name: node.ident.to_string(),
                kind: SymbolKind::Class,
                file: self.rel.to_string(),
                linked_concept: None,
                line: self.line_from_span(node.ident.span()),
                observation_source: ObservationSource::Ast,
            });
        }
    }

    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        if self.depth == 0 && Self::is_pub(&node.vis) {
            self.symbols.push(Symbol {
                name: node.ident.to_string(),
                kind: SymbolKind::Interface,
                file: self.rel.to_string(),
                linked_concept: None,
                line: self.line_from_span(node.ident.span()),
                observation_source: ObservationSource::Ast,
            });
        }
        // Don't recurse into trait body — method signatures inside a trait
        // are not top-level declarations.
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if self.depth == 0 && Self::is_pub(&node.vis) {
            self.symbols.push(Symbol {
                name: node.sig.ident.to_string(),
                kind: SymbolKind::Function,
                file: self.rel.to_string(),
                linked_concept: None,
                line: self.line_from_span(node.sig.ident.span()),
                observation_source: ObservationSource::Ast,
            });
        }
        // Do NOT recurse into fn body — nested functions inside a pub fn
        // are private implementation detail even if they happen to be pub.
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        // Increment depth so anything inside an impl block is NOT treated as
        // top-level. We don't visit methods at all — impl methods are not
        // in scope for the top-level-symbol contract.
        self.depth += 1;
        syn::visit::visit_item_impl(self, node);
        self.depth -= 1;
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        // Increment depth for inline mod blocks so items inside are not
        // treated as top-level. Inline mods with a body (`mod foo { ... }`)
        // are a separate namespace; file-level mods (`mod foo;`) have no
        // body to visit.
        self.depth += 1;
        syn::visit::visit_item_mod(self, node);
        self.depth -= 1;
    }
}

/// `syn`-based Rust extractor. Produces the same Symbol/Import/Route types as
/// `extract_rs`. Falls back to the regex extractor on parse failure (e.g.,
/// generated code, incomplete snippets), so a file that doesn't parse cleanly
/// still contributes what the regex can find.
///
/// Routes and imports use the same regex logic as `extract_rs` — see the
/// module comment above for why those remain lexical.
pub fn extract_rs_syn(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    routes: &mut Vec<Route>,
) {
    // Routes and imports: keep regex (deliberately lexical, see module doc).
    let mut regex_symbols = Vec::new();
    extract_rs(rel, text, &mut regex_symbols, imports, routes);
    // We only take the import/route side-effects above; symbols come from syn.

    match syn::parse_file(text) {
        Ok(ast) => {
            let mut visitor = RsVisitor { rel, symbols, depth: 0 };
            syn::visit::visit_file(&mut visitor, &ast);
        }
        Err(_) => {
            // Parse failure: fall back to the regex extractor's symbols so we
            // never produce fewer observations than the baseline. Tag each
            // symbol as LexicalFallback so callers know the observation did
            // not come from a verified AST — the file may be generated code,
            // an incomplete snippet, or a proc-macro expansion that syn cannot
            // handle.
            for mut sym in regex_symbols {
                sym.observation_source = ObservationSource::LexicalFallback;
                symbols.push(sym);
            }
        }
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
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(tree_sitter_go::language()).is_ok() {
        if let Some(tree) = parser.parse(text, None) {
            let bytes = text.as_bytes();
            let root = tree.root_node();
            walk_go(&root, bytes, rel, symbols, imports);
        }
    }
}

fn walk_go(
    node: &tree_sitter::Node,
    bytes: &[u8],
    rel: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
) {
    match node.kind() {
        "type_declaration" => {
            // type FooBar struct { ... } / type FooBar interface { ... }
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    let ch = cursor.node();
                    if ch.kind() == "type_spec" {
                        if let Some(name_node) = ch.child_by_field_name("name") {
                            let name = ts_text(name_node, bytes);
                            // Exported = uppercase first char
                            if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                                let type_node = ch.child_by_field_name("type");
                                let kind = match type_node.map(|n| n.kind()) {
                                    Some("interface_type") => SymbolKind::Interface,
                                    _ => SymbolKind::Class,
                                };
                                symbols.push(Symbol {
                                    name,
                                    kind,
                                    file: rel.to_string(),
                                    linked_concept: None,
                                    line: name_node.start_position().row + 1,
                                    observation_source: ObservationSource::Ast,
                                });
                            }
                        }
                    }
                    if !cursor.goto_next_sibling() { break; }
                }
            }
        }
        "function_declaration" | "method_declaration" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = ts_text(name_node, bytes);
                if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                    symbols.push(Symbol {
                        name,
                        kind: SymbolKind::Function,
                        file: rel.to_string(),
                        linked_concept: None,
                        line: name_node.start_position().row + 1,
                        observation_source: ObservationSource::Ast,
                    });
                }
            }
        }
        "import_declaration" => {
            // import "pkg" or import ( "pkg1" \n "pkg2" )
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    let ch = cursor.node();
                    if ch.kind() == "import_spec" || ch.kind() == "interpreted_string_literal" {
                        let raw = ts_text(ch, bytes);
                        let module = raw.trim_matches('"').to_string();
                        if !module.is_empty() {
                            imports.push(Import {
                                from_file: rel.to_string(),
                                to_module: module,
                                names: Vec::new(),
                            });
                        }
                    }
                    if !cursor.goto_next_sibling() { break; }
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            walk_go(&cursor.node(), bytes, rel, symbols, imports);
            if !cursor.goto_next_sibling() { break; }
        }
    }
}

// ── Java / Kotlin ─────────────────────────────────────────────────────────────

fn extract_java(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    routes: &mut Vec<Route>,
) {
    // Kotlin files share this extractor but tree-sitter-java cannot parse
    // Kotlin syntax — fall back to lexical extraction for .kt/.kts files.
    if rel.ends_with(".kt") || rel.ends_with(".kts") {
        extract_kotlin_lexical(rel, text, symbols, imports);
        return;
    }

    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(tree_sitter_java::language()).is_ok() {
        if let Some(tree) = parser.parse(text, None) {
            let bytes = text.as_bytes();
            let root = tree.root_node();
            walk_java(&root, bytes, rel, symbols, imports, routes);
        }
    }
}

/// Lexical fallback for Kotlin — tree-sitter-java cannot parse Kotlin syntax.
/// Extracts top-level `fun` declarations and classes/interfaces.
fn extract_kotlin_lexical(rel: &str, text: &str, symbols: &mut Vec<Symbol>, imports: &mut Vec<Import>) {
    let class_re = Regex::new(r"(?m)^(?:(?:public|internal|abstract|open|data|sealed)\s+)*class\s+([A-Z][A-Za-z0-9_]*)").unwrap();
    for cap in class_re.captures_iter(text) {
        symbols.push(Symbol {
            name: cap[1].to_string(),
            kind: SymbolKind::Class,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
            observation_source: ObservationSource::Lexical,
        });
    }
    let iface_re = Regex::new(r"(?m)^(?:(?:public|internal)\s+)?interface\s+([A-Z][A-Za-z0-9_]*)").unwrap();
    for cap in iface_re.captures_iter(text) {
        symbols.push(Symbol {
            name: cap[1].to_string(),
            kind: SymbolKind::Interface,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
            observation_source: ObservationSource::Lexical,
        });
    }
    // Top-level functions: `fun foo(...)` not indented inside a class
    let fun_re = Regex::new(r"(?m)^(?:(?:public|internal|private|suspend)\s+)*fun\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap();
    for cap in fun_re.captures_iter(text) {
        symbols.push(Symbol {
            name: cap[1].to_string(),
            kind: SymbolKind::Function,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
            observation_source: ObservationSource::Lexical,
        });
    }
    // import statements
    let import_re = Regex::new(r"import\s+([\w.]+(?:\.\*)?)").unwrap();
    for cap in import_re.captures_iter(text) {
        imports.push(Import { from_file: rel.to_string(), to_module: cap[1].to_string(), names: Vec::new() });
    }
}

fn walk_java(
    node: &tree_sitter::Node,
    bytes: &[u8],
    rel: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    routes: &mut Vec<Route>,
) {
    match node.kind() {
        "class_declaration" | "interface_declaration" | "enum_declaration" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let kind = if node.kind() == "interface_declaration" {
                    SymbolKind::Interface
                } else {
                    SymbolKind::Class
                };
                symbols.push(Symbol {
                    name: ts_text(name_node, bytes),
                    kind,
                    file: rel.to_string(),
                    linked_concept: None,
                    line: name_node.start_position().row + 1,
                    observation_source: ObservationSource::Ast,
                });
            }
        }
        "import_declaration" => {
            // import com.example.Service; or import com.example.*;
            let raw = ts_text(*node, bytes);
            let module = raw
                .trim_start_matches("import")
                .trim_end_matches(';')
                .trim()
                .to_string();
            if !module.is_empty() {
                imports.push(Import {
                    from_file: rel.to_string(),
                    to_module: module,
                    names: Vec::new(),
                });
            }
        }
        "method_declaration" => {
            // Spring MVC route annotations on methods: @GetMapping("/path")
            // Walk the method's modifiers for annotation nodes
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    let ch = cursor.node();
                    if ch.kind() == "modifiers" {
                        let mut mc = ch.walk();
                        if mc.goto_first_child() {
                            loop {
                                let ann = mc.node();
                                if ann.kind() == "annotation" || ann.kind() == "marker_annotation" {
                                    let ann_text = ts_text(ann, bytes);
                                    for (prefix, method) in &[
                                        ("GetMapping", "GET"), ("PostMapping", "POST"),
                                        ("PutMapping", "PUT"), ("DeleteMapping", "DELETE"),
                                        ("PatchMapping", "PATCH"),
                                    ] {
                                        if ann_text.contains(prefix) {
                                            let path_re = Regex::new(r#"["']([^"']+)["']"#).unwrap();
                                            let path = path_re.captures(&ann_text)
                                                .map(|c| c[1].to_string())
                                                .unwrap_or_else(|| "/".to_string());
                                            let handler = node.child_by_field_name("name")
                                                .map(|n| ts_text(n, bytes))
                                                .unwrap_or_else(|| "unknown".to_string());
                                            routes.push(Route {
                                                method: method.to_string(),
                                                path,
                                                handler,
                                                file: rel.to_string(),
                                            });
                                        }
                                    }
                                }
                                if !mc.goto_next_sibling() { break; }
                            }
                        }
                    }
                    if !cursor.goto_next_sibling() { break; }
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            walk_java(&cursor.node(), bytes, rel, symbols, imports, routes);
            if !cursor.goto_next_sibling() { break; }
        }
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
            observation_source: ObservationSource::Lexical,
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
            observation_source: ObservationSource::Lexical,
        });
    }

    // Methods: `def foo` / `def self.foo`. Unlike every other extractor here,
    // this intentionally does NOT require a top-level (column 0) anchor —
    // Ruby methods are conventionally indented inside class/module, so a
    // top-level-only rule would extract almost nothing. Trades a bit more
    // noise (private helpers) for actually finding real methods.
    let method_re = Regex::new(r"(?m)^\s*def\s+(?:self\.)?([a-z_][A-Za-z0-9_?!=]*)").unwrap();
    for cap in method_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
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

    // The far more common real-world form: `resources :articles` /
    // `resource :user` with a bare Ruby SYMBOL, not a quoted string — the
    // regex above only ever matched the string form, so an entirely
    // idiomatic routes.rb (this is how the Rails guides themselves write
    // every example) produced zero routes. Found dogfooding a real Rails
    // app. Doesn't try to enumerate the individual CRUD sub-actions
    // `resources` generates (index/show/create/...) or honor `only:`/
    // `except:` filters — reported as one "RESOURCES" entry per declared
    // resource, enough to answer "does a route family for this already
    // exist" without the added complexity of getting the filters wrong.
    let resource_re = Regex::new(r"(?m)^\s*resources?\s+:(\w+)").unwrap();
    for cap in resource_re.captures_iter(text) {
        routes.push(Route {
            method: "RESOURCES".to_string(),
            path: format!("/{}", &cap[1]),
            handler: "rails-resource".to_string(),
            file: rel.to_string(),
        });
    }
}

#[cfg(test)]
mod rb_tests {
    use super::*;

    #[test]
    fn extract_rb_finds_symbol_form_resources() {
        let src = r#"
Rails.application.routes.draw do
  resources :articles, only: [:index, :show]
  resource :user, only: [:show, :update]
  get '/health', to: 'health#check'
end
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_rb("config/routes.rb", src, &mut symbols, &mut imports, &mut routes);

        assert!(routes.iter().any(|r| r.method == "RESOURCES" && r.path == "/articles"));
        assert!(routes.iter().any(|r| r.method == "RESOURCES" && r.path == "/user"));
        assert!(routes.iter().any(|r| r.method == "GET" && r.path == "/health"));
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
            observation_source: ObservationSource::Lexical,
        });
    }

    // Public functions: `def foo(...)`. Same reasoning as Ruby above — always
    // indented under `defmodule`, so no top-level anchor. `defp` (private) is
    // deliberately excluded: `def\s+` cannot match inside the word `defp`.
    let fn_re = Regex::new(r"(?m)^\s*def\s+([a-z_][A-Za-z0-9_?!]*)").unwrap();
    for cap in fn_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
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
            observation_source: ObservationSource::Lexical,
        });
    }

    // Top-level global functions (Laravel helpers, WordPress-style procedural
    // code). Class methods are indented and excluded by the `^` anchor.
    let fn_re = Regex::new(r"(?m)^function\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap();
    for cap in fn_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
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
        symbols.push(Symbol { name: cap[2].to_string(), kind, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
    }

    // public methods (and constructors, which share the same shape minus a
    // return type match — accepted as a minor over-extraction, same tradeoff
    // regex-based extraction makes everywhere else in this file).
    let method_re = Regex::new(
        r"(?m)^\s*public\s+(?:static\s+|virtual\s+|override\s+|async\s+|sealed\s+)*[\w<>\[\],\.\?]+\s+([A-Z][A-Za-z0-9_]*)\s*\("
    ).unwrap();
    for cap in method_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
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
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
    }

    let struct_re = Regex::new(
        r"(?m)^(?:public\s+|internal\s+)*struct\s+([A-Z][A-Za-z0-9_]*)"
    ).unwrap();
    for cap in struct_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
    }

    // protocol — Swift's interface equivalent.
    let protocol_re = Regex::new(r"(?m)^(?:public\s+)?protocol\s+([A-Z][A-Za-z0-9_]*)").unwrap();
    for cap in protocol_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Interface, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
    }

    // func — top-level only, same noise tradeoff as everywhere else in this
    // file: a method inside a class/struct/protocol body is indented and
    // therefore excluded.
    let fn_re = Regex::new(
        r"(?m)^(?:public\s+|open\s+|internal\s+|private\s+|fileprivate\s+|static\s+|final\s+)*func\s+([A-Za-z_][A-Za-z0-9_]*)"
    ).unwrap();
    for cap in fn_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
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
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
    }
    let impl_re = Regex::new(r"(?m)^@implementation\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    for cap in impl_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
    }

    // @protocol Name — Objective-C's interface equivalent.
    let proto_re = Regex::new(r"(?m)^@protocol\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    for cap in proto_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Interface, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
    }

    // - (ReturnType)methodName / + (ReturnType)methodName. Top-level only
    // (no leading whitespace) — Objective-C methods are conventionally
    // written flush-left even inside @implementation, so this finds real
    // methods without needing Ruby's indentation-tolerant rule.
    let method_re = Regex::new(r"(?m)^[-+]\s*\([^)]*\)\s*([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    for cap in method_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
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
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
    }

    if is_cpp {
        let class_re = Regex::new(r"(?m)^class\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();
        for cap in class_re.captures_iter(text) {
            symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
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
        symbols.push(Symbol { name, kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
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
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
    }

    // object — Scala's singleton; treated as a Class for impact purposes,
    // same call the TypeScript extractor makes for enums above.
    let object_re = Regex::new(r"(?m)^(?:case\s+)?object\s+([A-Z][A-Za-z0-9_]*)").unwrap();
    for cap in object_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
    }

    let trait_re = Regex::new(r"(?m)^(?:sealed\s+)?trait\s+([A-Z][A-Za-z0-9_]*)").unwrap();
    for cap in trait_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Interface, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
    }

    // def — top-level only (a module-level def, e.g. in a `package object`
    // preamble). A method inside a class/object/trait body is indented and
    // therefore excluded — same top-level-only tradeoff Rust/PHP make.
    let def_re = Regex::new(
        r"(?m)^(?:private(?:\[\w+\])?\s+|protected\s+|final\s+|override\s+)*def\s+([a-zA-Z_][A-Za-z0-9_]*)"
    ).unwrap();
    for cap in def_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
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
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
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
        symbols.push(Symbol { name, kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
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
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
    }

    // class — a typeclass, Haskell's interface equivalent: a contract types
    // opt into, not a value's own type.
    let class_re = Regex::new(r"(?m)^class\s+(?:.*=>\s*)?([A-Z][A-Za-z0-9_']*)").unwrap();
    for cap in class_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Interface, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
    }

    // Top-level type signature lines: `name :: Type`. This is the most
    // reliable OBSERVED marker of a top-level binding in Haskell. The
    // corresponding `name arg1 arg2 = ...` equation line exists too but is
    // far noisier to tell apart from a pattern-match clause of the same
    // function, so the signature line is taken as the sole extraction.
    let sig_re = Regex::new(r"(?m)^([a-z_][A-Za-z0-9_']*)\s*::").unwrap();
    for cap in sig_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
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
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
    }

    // defrecord / deftype — Clojure's closest equivalent to a class.
    let record_re = Regex::new(r"\((?:defrecord|deftype)\s+([A-Za-z][A-Za-z0-9_\-]*)").unwrap();
    for cap in record_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
    }

    // defprotocol — Clojure's interface equivalent.
    let protocol_re = Regex::new(r"\(defprotocol\s+([A-Za-z][A-Za-z0-9_\-]*)").unwrap();
    for cap in protocol_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Interface, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
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
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
    }

    let interface_re = Regex::new(r"(?m)^interface\s+([A-Z][A-Za-z0-9_]*)").unwrap();
    for cap in interface_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Interface, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
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

    #[test]
    fn extract_ts_js_finds_commonjs_destructured_require() {
        let src = r#"
const { createApp } = require('./app');
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_ts_js("server/index.js", src, &mut symbols, &mut imports, &mut routes);

        assert!(
            imports.iter().any(|i| i.to_module == "./app" && i.names == vec!["createApp"]),
            "expected an Import for './app' with names=[createApp], got: {imports:?}"
        );
    }

    #[test]
    fn extract_ts_js_finds_commonjs_default_require() {
        let src = r#"
const app = require('./app');
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_ts_js("server/index.js", src, &mut symbols, &mut imports, &mut routes);

        assert!(
            imports.iter().any(|i| i.to_module == "./app" && i.names.is_empty()),
            "expected an Import for './app' with no named bindings (default require), got: {imports:?}"
        );
    }

    #[test]
    fn extract_ts_js_finds_commonjs_bare_require() {
        // Side-effect-only require, no binding at all — e.g. `require('./polyfills')`.
        let src = "require('./polyfills');\n";
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_ts_js("server/index.js", src, &mut symbols, &mut imports, &mut routes);

        assert!(
            imports.iter().any(|i| i.to_module == "./polyfills"),
            "expected an Import for './polyfills', got: {imports:?}"
        );
    }

    #[test]
    fn extract_ts_js_finds_commonjs_renamed_destructured_require() {
        // CommonJS destructuring rename uses `key: local`, not ES's `as` —
        // the extracted name should be the KEY ("Router"), matching what the
        // ES import branch does for `import { x as y }` (keeps "x").
        let src = r#"
const { Router: createRouter } = require('express');
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_ts_js("server/index.js", src, &mut symbols, &mut imports, &mut routes);

        assert!(
            imports.iter().any(|i| i.to_module == "express" && i.names == vec!["Router"]),
            "expected an Import for 'express' with names=[Router], got: {imports:?}"
        );
    }

    #[test]
    fn extract_ts_js_still_finds_es_imports_alongside_commonjs() {
        // Regression guard: adding the require() regex must not disturb the
        // existing ES `import ... from` extraction in the same file.
        let src = r#"
import { useState } from 'react';
const { createApp } = require('./app');
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_ts_js("src/mixed.ts", src, &mut symbols, &mut imports, &mut routes);

        assert!(imports.iter().any(|i| i.to_module == "react" && i.names == vec!["useState"]));
        assert!(imports.iter().any(|i| i.to_module == "./app" && i.names == vec!["createApp"]));
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
        symbols.push(Symbol { name: cap[1].to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
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
        symbols.push(Symbol { name: name.clone(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, *start) , observation_source: ObservationSource::Lexical });
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

// ── Gherkin / Cucumber ───────────────────────────────────────────────────────

fn extract_gherkin(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    _imports: &mut Vec<Import>,
    _routes: &mut Vec<Route>,
) {
    // Feature/Scenario names are free text, not identifiers — including
    // real-world non-ASCII names (Gherkin has no language restriction, and
    // this is common: a real .feature file found during this session's own
    // corpus check had a Chinese-language Feature/Scenario pair).
    let feature_re = Regex::new(r"(?m)^\s*Feature:\s*(.+)$").unwrap();
    for cap in feature_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].trim().to_string(), kind: SymbolKind::Class, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
    }
    let scenario_re = Regex::new(r"(?m)^\s*Scenario(?:\s+Outline)?:\s*(.+)$").unwrap();
    for cap in scenario_re.captures_iter(text) {
        symbols.push(Symbol { name: cap[1].trim().to_string(), kind: SymbolKind::Function, file: rel.to_string(), linked_concept: None, line: line_of(text, cap.get(0).unwrap().start()) , observation_source: ObservationSource::Lexical });
    }
}

fn extract_gdscript(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    _imports: &mut Vec<Import>,
    _routes: &mut Vec<Route>,
) {
    // `class_name X` is OPTIONAL in GDScript — a script attached to a node
    // very often has none at all (checked against a real project: fewer
    // than two-thirds of its .gd files declare one). A script with no
    // class_name is still a real, addressable unit of code (Godot loads it
    // by path, and every other script referencing that node's behavior
    // means THIS file) — same reasoning `extract_vue` already uses for a
    // Vue SFC with no explicit component name: fall back to the PascalCase
    // filename rather than silently producing no symbol at all.
    let class_name_re = Regex::new(r"(?m)^class_name\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    if let Some(cap) = class_name_re.captures(text) {
        symbols.push(Symbol {
            name: cap[1].to_string(),
            kind: SymbolKind::Class,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
            observation_source: ObservationSource::Lexical,
        });
    } else if let Some(stem) = std::path::Path::new(rel).file_stem().and_then(|s| s.to_str()) {
        symbols.push(Symbol {
            name: to_pascal_case(stem),
            kind: SymbolKind::Class,
            file: rel.to_string(),
            linked_concept: None,
            line: 1,
            observation_source: ObservationSource::Lexical,
        });
    }

    // Top-level only (the `^` anchor excludes a nested `func` inside an
    // `if`/inner `class` block, which GDScript indents like Python).
    // Leading underscore is Godot's naming CONVENTION for both engine
    // lifecycle callbacks (_ready, _process) and private helpers — neither
    // is a language-enforced access modifier the way Python's is treated
    // elsewhere in this codebase, so both are extracted the same as any
    // other top-level func.
    let func_re = Regex::new(r"(?m)^func\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap();
    for cap in func_re.captures_iter(text) {
        symbols.push(Symbol {
            name: cap[1].to_string(),
            kind: SymbolKind::Function,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
            observation_source: ObservationSource::Lexical,
        });
    }

    // signal name(args) — Godot's own event-declaration syntax, the
    // closest GDScript equivalent to what extract_ts_events captures for
    // TS/JS. Folded into Function (this project's SymbolKind has no
    // dedicated "event" variant), same as every other language here that
    // has no distinct kind for it.
    let signal_re = Regex::new(r"(?m)^signal\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    for cap in signal_re.captures_iter(text) {
        symbols.push(Symbol {
            name: cap[1].to_string(),
            kind: SymbolKind::Function,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
            observation_source: ObservationSource::Lexical,
        });
    }
}

fn extract_tscn(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    _routes: &mut Vec<Route>,
) {
    // The scene itself is the concept — identity by filename (PascalCase),
    // not the root node's own `name=` attribute: a scene's root node can be
    // renamed independently of the file, but every OTHER scene that
    // instances or attaches this one always references it by its PATH
    // (`res://.../ThisFile.tscn`), so the filename is what's actually
    // reliable to key on — same reasoning extract_vue/extract_gdscript's
    // own class_name-less fallback already use.
    if let Some(stem) = std::path::Path::new(rel).file_stem().and_then(|s| s.to_str()) {
        symbols.push(Symbol {
            name: to_pascal_case(stem),
            kind: SymbolKind::Class,
            file: rel.to_string(),
            linked_concept: None,
            line: 1,
            observation_source: ObservationSource::Lexical,
        });
    }

    // [ext_resource type="Script" path="res://X.gd" id="1"] — the script
    // attached to (some node in) this scene.
    // [ext_resource type="PackedScene" path="res://Y.tscn" id="2"] — a
    // child scene this one instances/composes (see the real
    // `[node ... instance=ExtResource("2")]` line elsewhere in the file —
    // the ext_resource declaration alone is enough to establish the
    // dependency; walking to the specific instancing node isn't needed for
    // an import EDGE to exist, but the id IS needed to find which node(s)
    // do the instancing, below).
    // Both captured uniformly as Import — this codebase has no "import
    // purpose" distinction anywhere else either (a TS `import` and a
    // Python `from X import Y` are both just "imports").
    let ext_resource_re = Regex::new(
        r#"(?m)^\[ext_resource\s+type="(Script|PackedScene)"[^\]]*\bpath="(res://[^"]+)"[^\]]*\bid="([^"]+)""#,
    )
    .unwrap();
    // `[node name="Player" parent="." instance=ExtResource("2")]` — a node
    // that instances a composed child scene, referencing the ext_resource
    // by its id (not its path — the path only appears once, up in the
    // ext_resource declaration). A single PackedScene can be instanced by
    // more than one node in the same scene, so ids map to a list of names.
    let instance_re =
        Regex::new(r#"(?m)^\[node\s+name="([^"]+)"[^\]]*\binstance=ExtResource\("([^"]+)"\)"#).unwrap();
    let mut instances_by_id: std::collections::HashMap<&str, Vec<String>> = std::collections::HashMap::new();
    for cap in instance_re.captures_iter(text) {
        instances_by_id.entry(cap.get(2).unwrap().as_str()).or_default().push(cap[1].to_string());
    }
    for cap in ext_resource_re.captures_iter(text) {
        let kind = &cap[1];
        let id = &cap[3];
        let names = if kind == "PackedScene" {
            instances_by_id.get(id).cloned().unwrap_or_default()
        } else {
            Vec::new()
        };
        imports.push(Import { from_file: rel.to_string(), to_module: cap[2].to_string(), names });
    }
}

// Godot 4 treats a script that declares BOTH `class_name` and an
// `[autoload]` registration of the same name as a hard parse error
// ("Class 'X' hides an autoload singleton") — so an autoload script
// deliberately has no `class_name` for extract_gdscript's PascalCase-
// filename fallback to (coincidentally) match against. project.godot's
// own `[autoload]` section is the only authoritative source of these
// scripts' real global names.
fn extract_project_godot(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    _imports: &mut Vec<Import>,
    _routes: &mut Vec<Route>,
) {
    let Some(section_start) = text.find("[autoload]") else { return };
    let body_start = section_start + "[autoload]".len();
    let body_end = text[body_start..]
        .find("\n[")
        .map(|offset| body_start + offset)
        .unwrap_or(text.len());
    let body = &text[body_start..body_end];

    // `Name="*res://path/To.gd"` — the leading `*` marks the autoload as
    // enabled and is tolerated but not required.
    let autoload_re = Regex::new(r#"(?m)^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*"\*?res://[^"]+""#).unwrap();
    for cap in autoload_re.captures_iter(body) {
        symbols.push(Symbol {
            name: cap[1].to_string(),
            kind: SymbolKind::Class,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, body_start + cap.get(0).unwrap().start()),
            observation_source: ObservationSource::Lexical,
        });
    }
}

#[cfg(test)]
mod gherkin_tests {
    use super::*;

    #[test]
    fn extract_gherkin_finds_feature_and_scenario_names() {
        let src = "\
@oidc @regression @smoke @P0
Feature: OIDC Device Flow 原生表单提交

  作为 LobeHub CLI 用户，

  @OIDC-DEVICE-001
  Scenario: loading 状态不会阻断设备授权表单提交
    Given CLI 已发起 OIDC Device Flow
    When 用户打开设备授权链接

  Scenario Outline: retry with <count> attempts
    Given a failed request
";
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_gherkin("features/oidc.feature", src, &mut symbols, &mut imports, &mut routes);

        assert!(symbols.iter().any(|s| s.name == "OIDC Device Flow 原生表单提交" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "loading 状态不会阻断设备授权表单提交" && s.kind == SymbolKind::Function));
        assert!(symbols.iter().any(|s| s.name == "retry with <count> attempts" && s.kind == SymbolKind::Function));
    }
}

#[cfg(test)]
mod gdscript_tests {
    use super::*;

    fn extract(rel: &str, src: &str) -> Vec<Symbol> {
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_gdscript(rel, src, &mut symbols, &mut imports, &mut routes);
        symbols
    }

    #[test]
    fn finds_class_name_top_level_functions_and_signals() {
        let src = "extends CharacterBody3D\n\nclass_name FirstPersonController\n\nsignal interacted(id: String)\n\nfunc _ready() -> void:\n    pass\n\nfunc _handle_jump() -> void:\n    pass\n";
        let symbols = extract("player/FirstPersonController.gd", src);
        assert!(symbols.iter().any(|s| s.name == "FirstPersonController" && s.kind == SymbolKind::Class), "got: {symbols:?}");
        assert!(symbols.iter().any(|s| s.name == "_ready" && s.kind == SymbolKind::Function), "got: {symbols:?}");
        assert!(symbols.iter().any(|s| s.name == "_handle_jump" && s.kind == SymbolKind::Function), "got: {symbols:?}");
        assert!(symbols.iter().any(|s| s.name == "interacted" && s.kind == SymbolKind::Function), "got: {symbols:?}");
    }

    /// The common real-world case: most GDScript files attached to a node
    /// have NO `class_name` at all — a script is still a real, addressable
    /// unit of code, so it must not be silently invisible. Same fallback
    /// `extract_vue` already uses for an unnamed Vue SFC.
    #[test]
    fn falls_back_to_pascal_case_filename_when_no_class_name() {
        let src = "extends Node\n\nfunc _ready() -> void:\n    pass\n";
        let symbols = extract("tests/unit/test_replay_engine.gd", src);
        assert!(symbols.iter().any(|s| s.name == "TestReplayEngine" && s.kind == SymbolKind::Class), "got: {symbols:?}");
    }

    /// A `func` indented inside a nested `class` block (GDScript supports
    /// inner classes, indentation-scoped like Python) is not top-level and
    /// must not be extracted — same top-level-only convention every other
    /// indentation-scoped language extractor here already applies.
    #[test]
    fn indented_func_inside_a_nested_class_is_not_extracted() {
        let src = "extends Node\n\nclass Inner:\n    func helper() -> void:\n        pass\n\nfunc _ready() -> void:\n    pass\n";
        let symbols = extract("world/Nested.gd", src);
        assert!(!symbols.iter().any(|s| s.name == "helper"), "got: {symbols:?}");
        assert!(symbols.iter().any(|s| s.name == "_ready"), "got: {symbols:?}");
    }

    /// End-to-end through the real scan pipeline: a .gd file must become a
    /// declared concept, not show up as an unclassified/unsupported language.
    #[test]
    fn end_to_end_scan_declares_a_concept_for_a_gd_file() {
        let root = std::env::temp_dir().join(format!("archietect-gdscript-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("Player.gd"),
            "extends CharacterBody3D\n\nclass_name Player\n\nfunc take_damage(amount: int) -> void:\n    pass\n",
        )
        .unwrap();

        let (idx, graph) = crate::scan::scan(&root);
        assert!(
            graph.symbols.values().any(|s| s.name == "Player" && s.kind == SymbolKind::Class),
            "got symbols: {:?}",
            graph.symbols.values().collect::<Vec<_>>()
        );
        let unclassified = crate::scan::unclassified_files(&root, &idx.excludes, 100);
        assert!(!unclassified.iter().any(|(_, ext)| ext == "gd"), "got: {unclassified:?}");

        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod tscn_tests {
    use super::*;

    fn extract(rel: &str, src: &str) -> (Vec<Symbol>, Vec<Import>) {
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_tscn(rel, src, &mut symbols, &mut imports, &mut routes);
        (symbols, imports)
    }

    #[test]
    fn scene_becomes_a_symbol_and_its_script_becomes_an_import() {
        let src = "[gd_scene load_steps=2 format=3]\n\n[ext_resource type=\"Script\" path=\"res://ui/WorkbenchCanvas.gd\" id=\"1\"]\n\n[node name=\"WorkbenchCanvas\" type=\"Control\"]\nscript = ExtResource(\"1\")\n";
        let (symbols, imports) = extract("ui/WorkbenchCanvas.tscn", src);
        assert!(symbols.iter().any(|s| s.name == "WorkbenchCanvas" && s.kind == SymbolKind::Class), "got: {symbols:?}");
        assert!(
            imports.iter().any(|i| i.to_module == "res://ui/WorkbenchCanvas.gd"),
            "got: {imports:?}"
        );
    }

    /// A scene composing a child scene (`instance=ExtResource(...)` on a
    /// node) is a real dependency edge — the ext_resource declaration alone
    /// establishes it; walking to the specific instancing node isn't needed.
    #[test]
    fn composed_child_scene_becomes_an_import() {
        let src = "[gd_scene load_steps=2 format=3]\n\n[ext_resource type=\"PackedScene\" path=\"res://player/FirstPersonController.tscn\" id=\"2\"]\n\n[node name=\"Zone\" type=\"Node3D\"]\n\n[node name=\"Player\" parent=\".\" instance=ExtResource(\"2\")]\n";
        let (_, imports) = extract("world/zones/Zone.tscn", src);
        assert!(
            imports.iter().any(|i| i.to_module == "res://player/FirstPersonController.tscn"),
            "got: {imports:?}"
        );
    }

    /// The ext_resource declaration establishes the edge; the `id=` it
    /// carries is also enough to find exactly which node(s) instance it,
    /// which is a strictly finer-grained fact than "this scene depends on
    /// that one" — real scenes in the wild have multiple ext_resources with
    /// distinct ids, so the cross-reference must key on id, not just be
    /// "the only instance node in the file".
    #[test]
    fn composed_child_scene_records_which_node_instances_it() {
        let src = "[gd_scene load_steps=3 format=3]\n\n[ext_resource type=\"Script\" path=\"res://world/ZoneTransition.gd\" id=\"1\"]\n[ext_resource type=\"PackedScene\" path=\"res://player/FirstPersonController.tscn\" id=\"2\"]\n\n[node name=\"Zone\" type=\"Node3D\"]\n\n[node name=\"Player\" parent=\".\" instance=ExtResource(\"2\")]\n";
        let (_, imports) = extract("world/zones/Zone.tscn", src);
        let script_import = imports.iter().find(|i| i.to_module == "res://world/ZoneTransition.gd").unwrap();
        assert!(script_import.names.is_empty(), "a script attachment isn't instanced by a node: {script_import:?}");
        let scene_import = imports.iter().find(|i| i.to_module == "res://player/FirstPersonController.tscn").unwrap();
        assert_eq!(scene_import.names, vec!["Player".to_string()], "got: {scene_import:?}");
    }

    /// A `[sub_resource ...]` block (materials, meshes, shapes — real
    /// content in every scene checked) has no `path=` at all and must never
    /// be mistaken for a dependency edge.
    #[test]
    fn sub_resource_blocks_are_not_extracted_as_imports() {
        let src = "[gd_scene load_steps=3 format=3]\n\n[sub_resource type=\"BoxShape3D\" id=\"BoxShape3D_quay\"]\nsize = Vector3(16, 1, 16)\n\n[node name=\"Zone\" type=\"Node3D\"]\n";
        let (_, imports) = extract("world/zones/Zone.tscn", src);
        assert!(imports.is_empty(), "got: {imports:?}");
    }

    /// End-to-end: `res://` import resolution (a project-root-relative path
    /// with an explicit extension, unlike the `./`/`../` case) must find the
    /// real scanned file, giving a genuine cross-file relationship — a scene
    /// depending on its script — not just an isolated Import record.
    #[test]
    fn end_to_end_scan_resolves_the_res_path_to_the_real_script_file() {
        let root = std::env::temp_dir().join(format!("archietect-tscn-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("player")).unwrap();
        std::fs::write(
            root.join("player/Player.gd"),
            "extends CharacterBody3D\n\nclass_name Player\n",
        )
        .unwrap();
        std::fs::write(
            root.join("player/Player.tscn"),
            "[gd_scene load_steps=2 format=3]\n\n[ext_resource type=\"Script\" path=\"res://player/Player.gd\" id=\"1\"]\n\n[node name=\"Player\" type=\"CharacterBody3D\"]\nscript = ExtResource(\"1\")\n",
        )
        .unwrap();

        let (_idx, graph) = crate::scan::scan(&root);
        assert!(
            graph.symbols.values().any(|s| s.name == "Player" && s.kind == SymbolKind::Class && s.file.ends_with(".tscn")),
            "the scene itself must be a symbol, got: {:?}",
            graph.symbols.values().collect::<Vec<_>>()
        );
        let scene_import = graph.imports.iter().find(|i| i.from_file == "player/Player.tscn");
        assert!(scene_import.is_some(), "got imports: {:?}", graph.imports);
        assert_eq!(scene_import.unwrap().to_module, "res://player/Player.gd");

        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod project_godot_tests {
    use super::*;

    fn extract(rel: &str, src: &str) -> Vec<Symbol> {
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_project_godot(rel, src, &mut symbols, &mut imports, &mut routes);
        symbols
    }

    #[test]
    fn autoload_entries_become_symbols() {
        let src = "config_version=5\n\n[application]\n\nconfig/name=\"Operator Ziwani\"\nconfig/icon=\"res://assets/icon.svg\"\n\n[autoload]\n\nEventBus=\"*res://core/EventBus.gd\"\nGameState=\"*res://core/GameState.gd\"\n\n[application]\n\nrun/main_scene=\"res://world/Main.tscn\"\n";
        let symbols = extract("project.godot", src);
        assert!(symbols.iter().any(|s| s.name == "EventBus" && s.kind == SymbolKind::Class), "got: {symbols:?}");
        assert!(symbols.iter().any(|s| s.name == "GameState"), "got: {symbols:?}");
        assert_eq!(symbols.len(), 2, "got: {symbols:?}");
    }

    /// The `*` enabled-marker is common but not guaranteed — an autoload a
    /// developer has toggled off in the editor loses it while the entry
    /// stays in the file, and it's still the real global name of that
    /// script if/when it's re-enabled.
    #[test]
    fn tolerates_missing_enabled_marker() {
        let src = "[autoload]\n\nTelemetryRecorder=\"res://core/TelemetryRecorder.gd\"\n";
        let symbols = extract("project.godot", src);
        assert!(symbols.iter().any(|s| s.name == "TelemetryRecorder"), "got: {symbols:?}");
    }

    /// A `key="res://..."` assignment outside `[autoload]` (e.g.
    /// `config/icon=` in `[application]`, or `run/main_scene=`) must never
    /// be mistaken for an autoload registration — scoping to the
    /// `[autoload]` section's own body is what prevents that.
    #[test]
    fn ignores_res_paths_outside_the_autoload_section() {
        let src = "[application]\n\nconfig/icon=\"res://assets/icon.svg\"\nrun/main_scene=\"res://world/Main.tscn\"\n";
        let symbols = extract("project.godot", src);
        assert!(symbols.is_empty(), "got: {symbols:?}");
    }

    #[test]
    fn no_autoload_section_yields_no_symbols() {
        let src = "config_version=5\n\n[application]\n\nconfig/name=\"Operator Ziwani\"\n";
        let symbols = extract("project.godot", src);
        assert!(symbols.is_empty(), "got: {symbols:?}");
    }

    /// End-to-end: an autoload script that (per real Godot 4 semantics)
    /// deliberately has no `class_name` still resolves to a real, correctly-
    /// named concept — because project.godot's own [autoload] section, not
    /// the script file, is authoritative for that name.
    #[test]
    fn end_to_end_scan_declares_a_concept_for_an_autoload_singleton() {
        let root = std::env::temp_dir().join(format!("archietect-godot-autoload-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("core")).unwrap();
        std::fs::write(
            root.join("core/EventBus.gd"),
            "extends Node\n\n# No class_name here on purpose — this script is registered as\n# an autoload, and Godot 4 treats a class_name of the same name as an\n# autoload registration as a hard parse error.\n\nsignal telemetry_event(name: String)\n",
        )
        .unwrap();
        std::fs::write(
            root.join("project.godot"),
            "config_version=5\n\n[autoload]\n\nEventBus=\"*res://core/EventBus.gd\"\n",
        )
        .unwrap();

        let (_idx, graph) = crate::scan::scan(&root);
        assert!(
            graph.symbols.values().any(|s| s.name == "EventBus" && s.kind == SymbolKind::Class && s.file == "project.godot"),
            "got: {:?}",
            graph.symbols.values().collect::<Vec<_>>()
        );
    }
}

#[cfg(test)]
mod structural_dependents_structural_only_tests {
    use super::*;

    /// `structural_dependents` must find importers of a STRUCTURAL-only
    /// symbol — a class with no schema model behind it, so `linked_concept`
    /// is `None`. Before the fix, `owner_files` was filtered on
    /// `linked_concept` alone, came back empty for every such symbol, and
    /// the function returned before walking a single import edge — reporting
    /// "nothing touches it" for symbols that were imported and called. Found
    /// live against a real repository: `impact` on a plain exported class
    /// returned no dependents while another file demonstrably imported it.
    /// This fixture is the same shape: a plain exported class, a file that
    /// imports it by relative path, nothing declared in any schema.
    #[test]
    fn finds_importers_of_a_structural_only_class() {
        let tmp = std::env::temp_dir()
            .join(format!("archietect-structdeps-structural-only-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("services")).unwrap();
        std::fs::write(
            tmp.join("services").join("governance-client.ts"),
            "export class NotificationClient {\n  evaluate() {}\n}\n",
        )
        .unwrap();
        std::fs::write(
            tmp.join("extension.ts"),
            "import { NotificationClient } from './services/governance-client';\nconst c = new NotificationClient();\n",
        )
        .unwrap();
        std::fs::write(tmp.join("unrelated.ts"), "export const x = 1;\n").unwrap();

        let (idx, graph) = crate::scan::scan(&tmp);
        assert!(
            !idx.concepts.contains_key("NotificationClient"),
            "sanity: must be structural-only, with no schema concept behind it"
        );
        assert!(
            graph.symbols.values().any(|s| s.name == "NotificationClient" && s.linked_concept.is_none()),
            "sanity: the symbol exists and is NOT linked to any schema concept"
        );

        let deps = structural_dependents(&graph, "NotificationClient", 3);
        let files: Vec<&str> = deps.iter().map(|d| d.file.as_str()).collect();
        assert!(
            files.contains(&"extension.ts"),
            "extension.ts imports NotificationClient and must be reported as a dependent, got: {files:?}"
        );
        assert!(
            !files.contains(&"unrelated.ts"),
            "a file that imports nothing relevant must not be reported, got: {files:?}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

#[cfg(test)]
mod ts_js_local_symbol_tests {
    use super::*;

    /// Every symbol pattern in extract_ts_js used to require `export` —
    /// found live: two real React components, neither exported at their own
    /// declaration site (used only within the same file, or re-exported one
    /// level up through a barrel file), were completely invisible to the
    /// concept index. This reproduces that exact shape with generic names —
    /// a `function` declaration and a `const` arrow, both PascalCase,
    /// neither `export`ed.
    #[test]
    fn finds_unexported_pascalcase_function_and_const_declarations() {
        let src = r#"
function Dashboard() {
  return null;
}

const SettingsPanel = () => {
  return null;
};

function helper() {
  return 1;
}

const config = () => ({});
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_ts_js("src/App.tsx", src, &mut symbols, &mut imports, &mut routes);

        assert!(
            symbols.iter().any(|s| s.name == "Dashboard" && s.kind == SymbolKind::Function),
            "unexported PascalCase `function Dashboard()` must be captured, got: {symbols:?}"
        );
        assert!(
            symbols.iter().any(|s| s.name == "SettingsPanel" && s.kind == SymbolKind::Function),
            "unexported PascalCase `const SettingsPanel = () =>` must be captured, got: {symbols:?}"
        );
        assert!(
            !symbols.iter().any(|s| s.name == "helper"),
            "lowercase unexported functions must stay excluded — capturing every local helper would flood the index, got: {symbols:?}"
        );
        assert!(
            !symbols.iter().any(|s| s.name == "config"),
            "lowercase unexported consts must stay excluded, got: {symbols:?}"
        );
    }

    /// An exported PascalCase function must not be recorded twice (once by
    /// the pre-existing `export`-anchored pattern, once by the new
    /// unexported-local pattern) — `extract_file`'s own `dedup_by` collapses
    /// same name+kind, but that dedup only works if this extractor doesn't
    /// hand it two structurally different Symbol values (e.g. different
    /// `line`) for the same declaration in the first place.
    #[test]
    fn exported_pascalcase_function_is_not_double_counted() {
        let src = "export function Dashboard() {\n  return null;\n}\n";
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_ts_js("src/App.tsx", src, &mut symbols, &mut imports, &mut routes);
        let hits: Vec<&Symbol> = symbols.iter().filter(|s| s.name == "Dashboard").collect();
        assert_eq!(hits.len(), 1, "exported Dashboard must appear exactly once before dedup even runs, got: {symbols:?}");
    }

    /// Found investigating a real Angular SPA: `type View = 'dashboard' |
    /// 'candidates' | ...` — a discriminated-union type driving the app's
    /// entire hand-rolled navigation — had no structural representation at
    /// all before this. Neither exported nor unexported forms were caught
    /// by any prior pattern (class_re requires the literal keyword `class`
    /// or `interface`, never `type`).
    #[test]
    fn finds_type_alias_declarations_exported_and_not() {
        let src = r#"
export type View = 'dashboard' | 'candidates' | 'pipeline';

type StageMoveOptions = {
  note?: string;
};

type helperAlias = string;
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_ts_js("src/app.ts", src, &mut symbols, &mut imports, &mut routes);
        assert!(
            symbols.iter().any(|s| s.name == "View" && s.kind == SymbolKind::Class),
            "exported PascalCase type alias must be captured, got: {symbols:?}"
        );
        assert!(
            symbols.iter().any(|s| s.name == "StageMoveOptions"),
            "unexported PascalCase type alias must be captured, got: {symbols:?}"
        );
        assert!(
            !symbols.iter().any(|s| s.name == "helperAlias"),
            "lowercase-led type aliases stay excluded, same convention as every other pattern here, got: {symbols:?}"
        );
    }
}

#[cfg(test)]
mod angular_route_tests {
    use super::*;

    /// The real reported shape: an Angular `Routes` array, `path` before
    /// `component` (Angular's own docs' usual ordering). Before this,
    /// Angular had NO route recognition at all — this array was invisible
    /// to `graph.routes` entirely, the same class of gap Rust's total
    /// absence of route recognition was before that got fixed.
    #[test]
    fn finds_routes_with_path_before_component() {
        let src = r#"
export const routes: Routes = [
  { path: 'dashboard', component: DashboardComponent },
  { path: 'candidates/:id', component: CandidateDetailComponent },
];
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_ts_js("src/app.routes.ts", src, &mut symbols, &mut imports, &mut routes);
        assert!(
            routes.iter().any(|r| r.path == "dashboard" && r.handler == "DashboardComponent"),
            "got: {routes:?}"
        );
        assert!(
            routes.iter().any(|r| r.path == "candidates/:id" && r.handler == "CandidateDetailComponent"),
            "got: {routes:?}"
        );
    }

    /// Angular's docs also show `component` written before `path` in some
    /// examples — both orderings are real, valid TypeScript object-literal
    /// syntax with identical meaning, so both must resolve to the same
    /// Route.
    #[test]
    fn finds_routes_with_component_before_path() {
        let src = r#"
export const routes: Routes = [
  { component: PipelineComponent, path: 'pipeline' },
];
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_ts_js("src/app.routes.ts", src, &mut symbols, &mut imports, &mut routes);
        assert!(
            routes.iter().any(|r| r.path == "pipeline" && r.handler == "PipelineComponent"),
            "got: {routes:?}"
        );
    }

    /// Two adjacent routes in one array must not bleed into each other —
    /// the non-greedy, brace-bounded matching between `path`/`component`
    /// must not let route A's `path` pair up with route B's `component`.
    #[test]
    fn adjacent_routes_do_not_cross_contaminate() {
        let src = r#"
export const routes: Routes = [
  { path: 'a', component: AComponent },
  { path: 'b', component: BComponent },
];
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_ts_js("src/app.routes.ts", src, &mut symbols, &mut imports, &mut routes);
        assert!(routes.iter().any(|r| r.path == "a" && r.handler == "AComponent"), "got: {routes:?}");
        assert!(routes.iter().any(|r| r.path == "b" && r.handler == "BComponent"), "got: {routes:?}");
        assert!(
            !routes.iter().any(|r| r.path == "a" && r.handler == "BComponent"),
            "route A's path must never pair with route B's component, got: {routes:?}"
        );
    }
}

#[cfg(test)]
mod duplicate_logic_tests {
    use super::*;

    /// The real reported shape, reproduced with generic names: two
    /// functions in two DIFFERENT files/languages, different names sharing
    /// no meaningful token, independently encoding the same business rule —
    /// evidenced by both containing the same status-string literals.
    #[test]
    fn cross_file_cross_language_shared_literals_are_detected() {
        let js_src = r#"
function updateStage(record, nextStage) {
  let status = null;
  if (nextStage === 'Passed Screening') {
    status = 'Screened';
  }
  if (nextStage === 'Fully Hired') {
    status = 'Candidate Hired';
  }
  return status;
}
"#;
        let ts_src = r#"
function moveStage(record: Record, nextStage: string) {
  let status = null;
  if (nextStage === 'Passed Screening') {
    status = 'Screened';
  }
  if (nextStage === 'Fully Hired') {
    status = 'Candidate Hired';
  }
  return status;
}
"#;
        let mut bodies = Vec::new();
        extract_function_bodies("server/repository.js", "js", js_src, &mut bodies);
        extract_function_bodies("src/app/service.ts", "ts", ts_src, &mut bodies);
        assert_eq!(bodies.len(), 2, "got: {bodies:?}");

        let graph = StructuralGraph { function_bodies: bodies, ..Default::default() };
        let dups = suspected_duplicate_logic(&graph, 2);
        assert_eq!(dups.len(), 1, "got: {dups:?}");
        assert!(dups[0].shared_literals.contains(&"Passed Screening".to_string()));
        assert!(dups[0].shared_literals.contains(&"Candidate Hired".to_string()));
    }

    /// Two functions in the SAME file sharing literals is not this
    /// feature's concern — same-file duplication is either normal (a
    /// switch-like sequence of ifs each checking the same constants) or
    /// something a human reading the one file in front of them will
    /// already see; the actual risk this exists for is business logic
    /// silently drifting apart ACROSS files.
    #[test]
    fn same_file_pairs_are_never_reported() {
        let src = r#"
function a() {
  if (x === 'Passed Screening') { return 'Screened'; }
}
function b() {
  if (y === 'Passed Screening') { return 'Screened'; }
}
"#;
        let mut bodies = Vec::new();
        extract_function_bodies("one.js", "js", src, &mut bodies);
        let graph = StructuralGraph { function_bodies: bodies, ..Default::default() };
        let dups = suspected_duplicate_logic(&graph, 1);
        assert!(dups.is_empty(), "got: {dups:?}");
    }

    /// A single shared literal below the threshold must not be reported —
    /// one coincidentally shared string (a common error message, say) is
    /// not evidence of duplicated business logic on its own.
    #[test]
    fn single_shared_literal_below_threshold_is_not_reported() {
        let a_src = "function a() {\n  return 'Resource Not Found';\n}\n";
        let b_src = "function b() {\n  return 'Resource Not Found';\n}\n";
        let mut bodies = Vec::new();
        extract_function_bodies("a.js", "js", a_src, &mut bodies);
        extract_function_bodies("b.js", "js", b_src, &mut bodies);
        let graph = StructuralGraph { function_bodies: bodies, ..Default::default() };
        let dups = suspected_duplicate_logic(&graph, 2);
        assert!(dups.is_empty(), "one shared literal must not clear a threshold of 2, got: {dups:?}");
    }

    /// A literal shared by a large number of functions (generic boilerplate
    /// like "error"/"success") must not connect all of them to each other —
    /// that would flood real results with noise instead of surfacing an
    /// actual duplicated rule.
    #[test]
    fn overly_common_literal_does_not_create_a_flood_of_pairs() {
        let mut bodies = Vec::new();
        for i in 0..60 {
            let src = format!("function f{i}() {{\n  return 'Generic Value';\n}}\n");
            extract_function_bodies(&format!("file{i}.js"), "js", &src, &mut bodies);
        }
        let graph = StructuralGraph { function_bodies: bodies, ..Default::default() };
        let dups = suspected_duplicate_logic(&graph, 1);
        assert!(
            dups.is_empty(),
            "a literal shared by 60 functions must be treated as generic boilerplate, not evidence — got {} pairs",
            dups.len()
        );
    }

    /// A string literal INSIDE another string literal (a brace-like
    /// character as literal text, e.g. a JSON-shaped string) must not
    /// desynchronize brace-depth counting and truncate the function body
    /// early.
    #[test]
    fn braces_inside_string_literals_do_not_break_body_bounds() {
        let src = r#"
function build() {
  const template = "{\"key\": \"value\"}";
  if (template === 'Passed Screening Marker') {
    return 'Hired Marker';
  }
}
"#;
        let mut bodies = Vec::new();
        extract_function_bodies("weird.js", "js", src, &mut bodies);
        assert_eq!(bodies.len(), 1, "got: {bodies:?}");
        assert!(
            bodies[0].literals.iter().any(|l| l == "Passed Screening Marker"),
            "a literal AFTER a brace-containing string must still be captured — the body must not have been cut short, got: {:?}", bodies[0].literals
        );
    }

    /// Python's indentation-bounded body extraction, exercised end-to-end
    /// against JS via the real cross-reference pipeline — cross-language
    /// (a Node/JS backend and a Python service), not just two JS-family
    /// files.
    #[test]
    fn python_function_body_bounds_are_indentation_based() {
        let py_src = "def approve(record):\n    if record.stage == 'Passed Screening':\n        return 'Candidate Hired'\n    return None\n\ndef unrelated():\n    return 1\n";
        let mut bodies = Vec::new();
        extract_function_bodies("service.py", "py", py_src, &mut bodies);
        let approve = bodies.iter().find(|b| b.name == "approve").expect("got: {bodies:?}");
        assert!(approve.literals.contains(&"Passed Screening".to_string()));
        assert!(approve.literals.contains(&"Candidate Hired".to_string()));
        assert!(
            !bodies.iter().any(|b| b.name == "unrelated" && !b.literals.is_empty()),
            "unrelated()'s trivial body (no literal >=10 chars) must not spuriously match anything"
        );
    }

    /// A Rust lifetime (`&'static`) or a lone apostrophe in a comment/SQL
    /// empty-string literal (`''`) must never be treated as opening a
    /// string: `'` pairing a lifetime's quote with an unrelated LATER
    /// apostrophe swallows everything between as a fake "string" and
    /// desyncs brace-depth counting, letting the function body run past its
    /// real closing `}` into the next function.
    #[test]
    fn rust_lifetimes_and_apostrophes_do_not_desync_body_bounds() {
        let src = r#"
pub fn first(db: &'static Pool) -> Value {
    // handles the polygon's rings correctly
    let empty = "" ;
    let q = "SELECT 1 WHERE x <> ''";
    json!({ "marker one long": empty, "other marker long": q })
}

pub fn second() -> Value {
    json!({ "second marker long": "unique second value here" })
}
"#;
        let mut bodies = Vec::new();
        extract_function_bodies("admin1.rs", "rs", src, &mut bodies);
        let first = bodies.iter().find(|b| b.name == "first").expect("got: {bodies:?}");
        let second = bodies.iter().find(|b| b.name == "second").expect("got: {bodies:?}");
        assert!(
            !first.literals.iter().any(|l| l.contains("second marker")),
            "first()'s body must not have swallowed second()'s content, got: {:?}", first.literals
        );
        assert!(
            second.literals.iter().any(|l| l.contains("unique second value")),
            "second() must still be extracted as its own function, got: {:?}", second.literals
        );
    }

    /// A Rust string built with backslash-newline line continuations
    /// (common for multi-line SQL queries) must be captured as ONE literal,
    /// not broken apart — with `.` excluding `\n` by default, `\` followed
    /// by a literal newline matched neither "not a quote" nor "escaped
    /// character", so the match failed at the string's own quotes and slid
    /// forward to an unrelated later quote, merging the CODE between two
    /// real strings into one fake "literal".
    #[test]
    fn backslash_newline_continuations_do_not_break_string_bounds() {
        let src = "pub fn q() -> Value {\n    let query = \"SELECT admin1, \\\n         FROM world_events\";\n    json!({ \"result marker\": query })\n}\n";
        let mut bodies = Vec::new();
        extract_function_bodies("q.rs", "rs", src, &mut bodies);
        let f = &bodies[0];
        assert!(
            f.literals.iter().any(|l| l.contains("SELECT admin1") && l.contains("FROM world_events")),
            "the backslash-newline-continued SQL string must be captured whole, got: {:?}", f.literals
        );
        assert!(
            !f.literals.iter().any(|l| l.contains("query))")),
            "no literal should contain raw code from between two real strings, got: {:?}", f.literals
        );
    }

    /// A CSS color value is a design token, not business logic — it must
    /// be excluded even though `rgba(...)` values easily clear the 10-char
    /// floor, since two wholly unrelated "status color" components sharing
    /// nothing but a common traffic-light palette is not evidence of
    /// duplicated business logic.
    #[test]
    fn color_literals_are_excluded_from_duplicate_logic_evidence() {
        let a = "function barColor() {\n  return 'rgba(52,211,153,1)';\n}\n";
        let mut bodies = Vec::new();
        extract_function_bodies("a.js", "js", a, &mut bodies);
        assert!(bodies.is_empty(), "a function with ONLY a color literal must yield no FunctionBody at all, got: {bodies:?}");
    }
}

#[cfg(test)]
mod route_call_tests {
    use super::*;

    /// The collision this whole feature has to avoid: FastAPI declares a
    /// route as `@app.get(...)` — a DECORATOR — which is textually
    /// `.get("...")` after a dot, the exact shape an outbound call has. If
    /// route-call extraction can't tell these apart, every FastAPI/Flask
    /// project (two of this engine's own supported frameworks) would have
    /// its own route declarations misfiled as outbound calls.
    #[test]
    fn fastapi_decorator_is_never_mistaken_for_an_outbound_call() {
        let src = r#"
@app.get("/orders/{order_id}")
def get_order(order_id: str):
    return {"id": order_id}
"#;
        let mut route_calls = Vec::new();
        extract_route_calls("service.py", "py", src, &mut route_calls);
        assert!(
            route_calls.is_empty(),
            "a route DECLARATION must never be recorded as a call, got: {route_calls:?}"
        );
    }

    /// The other false-positive this has to avoid: `cache.get("some_key")`
    /// has the exact same `.get("...")` shape as a real HTTP call, with an
    /// argument that is not a path at all — nothing here should assume every
    /// `.get(string)` call in a scanned repo is an HTTP request.
    #[test]
    fn non_path_dict_style_get_is_not_recorded_as_a_call() {
        let src = r#"value = cache.get("some_key")"#;
        let mut route_calls = Vec::new();
        extract_route_calls("service.py", "py", src, &mut route_calls);
        assert!(
            route_calls.is_empty(),
            "a lookup key is not a URL path, got: {route_calls:?}"
        );
    }

    /// A genuine outbound call — the real positive case — must still be
    /// recorded, on the same line shape that the FastAPI test above proves
    /// does NOT fire for a decorator.
    #[test]
    fn genuine_outbound_call_is_recorded() {
        let src = r#"resp = requests.post(f"http://orders-svc:8001/orders/{order_id}/approve", json=body)"#;
        let mut route_calls = Vec::new();
        extract_route_calls("client.py", "py", src, &mut route_calls);
        assert_eq!(route_calls.len(), 1, "got: {route_calls:?}");
        assert_eq!(route_calls[0].path, "http://orders-svc:8001/orders/{order_id}/approve");
    }

    /// End-to-end: a route declared in one file (Python/FastAPI) is called
    /// ONLY from a different file (TypeScript, via axios) with NO import
    /// edge between them — impossible in this pairing anyway, but that's
    /// the point: an import graph has nothing to walk here regardless of
    /// language. Without route_call_dependents, `impact()` on the concept
    /// behind this route would report "NONE OBSERVED — declared but nothing
    /// seen touching it" for a route a real caller demonstrably calls.
    #[test]
    fn route_declared_in_one_file_called_from_another_is_no_longer_invisible() {
        let tmp = std::env::temp_dir()
            .join(format!("archietect-routecall-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("backend")).unwrap();
        std::fs::create_dir_all(tmp.join("frontend")).unwrap();
        std::fs::write(
            tmp.join("backend").join("orders_service.py"),
            "class Orders:\n    pass\n\n@app.post(\"/orders/{order_id}/approve\")\ndef approve_order(order_id: str):\n    return {\"ok\": True}\n",
        )
        .unwrap();
        std::fs::write(
            tmp.join("frontend").join("orderClient.ts"),
            "export async function approveOrder(orderId: string) {\n  return axios.post(`/orders/${orderId}/approve`, {});\n}\n",
        )
        .unwrap();

        let (_idx, graph) = crate::scan::scan(&tmp);
        assert!(
            graph.routes.iter().any(|r| r.handler == "approve_order" && r.path == "/orders/{order_id}/approve"),
            "sanity: the FastAPI route must actually be declared, got: {:?}", graph.routes
        );
        assert!(
            !graph.route_calls.is_empty(),
            "sanity: the axios call must actually be extracted, got route_calls: {:?}", graph.route_calls
        );
        // "Orders" isn't a token-match for handler "approve_order" (same_word
        // requires a shared prefix, not a shared substring) — this concept
        // is only reachable via the route's OWN path-contains-name fallback
        // (relationship_to's NAMED tier: "/orders/.../approve" contains
        // "orders"), same real path a concept name unrelated to its route
        // handler's own naming convention would take.
        assert!(
            !crate::model::same_word("approve_order", "Orders"),
            "sanity: this test must exercise the path-contains-name fallback, not a handler name-match"
        );

        let deps = route_call_dependents(&graph, "Orders");
        assert!(
            deps.iter().any(|d| d.file == "frontend/orderClient.ts"),
            "orderClient.ts calls the /orders/{{order_id}}/approve route and must be reported as a route-call dependent of Orders, got: {deps:?}"
        );

        let impact = crate::query::impact(&_idx, &graph, "Orders");
        assert_ne!(
            impact["severity"], "NONE OBSERVED — declared but nothing seen touching it",
            "a concept whose route is genuinely called from another file must not report zero touchpoints, got: {impact}"
        );
        assert!(
            !impact["route_call_dependents"].as_array().unwrap().is_empty(),
            "impact() must surface the route-call evidence, got: {impact}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Rust had zero route/framework recognition before this — a
    /// precondition `route_call_dependents` needs: it requires a declared
    /// Route to exist at all, and a Rust service's endpoint could never
    /// become one regardless of how good call-site detection got. Axum's
    /// chained builder syntax is the target here.
    #[test]
    fn axum_route_declarations_are_extracted() {
        let src = r#"
let app = Router::new()
    .route("/orders/:id", get(get_order).post(update_order))
    .route("/ws", get(ws_handler));
"#;
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_rs("gateway.rs", src, &mut symbols, &mut imports, &mut routes);
        assert!(
            routes.iter().any(|r| r.method == "GET" && r.path == "/orders/:id" && r.handler == "get_order"),
            "got: {routes:?}"
        );
        assert!(
            routes.iter().any(|r| r.method == "POST" && r.path == "/orders/:id" && r.handler == "update_order"),
            "chained .post(...) on the same .route(...) call must also be captured, got: {routes:?}"
        );
        assert!(
            routes.iter().any(|r| r.method == "GET" && r.path == "/ws" && r.handler == "ws_handler"),
            "a WebSocket upgrade handler is registered exactly like any other Axum route — no special-casing needed, got: {routes:?}"
        );
    }

    /// Actix-web/Rocket's attribute-macro route declaration — same "read the
    /// next fn after the match" shape as FastAPI's decorator in extract_py,
    /// just Rust attribute syntax instead of a Python decorator.
    #[test]
    fn actix_style_attribute_route_is_extracted() {
        let src = "#[post(\"/orders/{order_id}/approve\")]\nasync fn approve_order(path: web::Path<String>) -> impl Responder {\n    HttpResponse::Ok()\n}\n";
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut routes = Vec::new();
        extract_rs("orders.rs", src, &mut symbols, &mut imports, &mut routes);
        assert!(
            routes.iter().any(|r| r.method == "POST" && r.path == "/orders/{order_id}/approve" && r.handler == "approve_order"),
            "got: {routes:?}"
        );
    }

    /// The real reported shape, reproduced end-to-end: a WebSocket endpoint
    /// declared in Rust (Axum), called from Python via `websockets.connect`
    /// — no import edge, a different language on each side, and (unlike the
    /// REST case earlier in this module) an actual `ws://` scheme. Proves
    /// path_only's generic scheme-stripping (verified earlier to work on
    /// ANY `://`, not just http/https) and the new Rust route extraction
    /// compose correctly through the full scan -> route_call_dependents ->
    /// impact() pipeline, not just as isolated units.
    #[test]
    fn websocket_endpoint_declared_in_rust_called_from_python_is_not_invisible() {
        let tmp = std::env::temp_dir()
            .join(format!("archietect-ws-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("gateway")).unwrap();
        std::fs::create_dir_all(tmp.join("client")).unwrap();
        std::fs::write(
            tmp.join("gateway").join("main.rs"),
            "pub struct Council;\n\nlet app = Router::new().route(\"/council/deliberate\", get(deliberate_handler));\n",
        )
        .unwrap();
        std::fs::write(
            tmp.join("client").join("council_client.py"),
            "import websockets\n\nasync def call_council():\n    async with websockets.connect(\"ws://ws_gateway:8001/council/deliberate\") as ws:\n        return await ws.recv()\n",
        )
        .unwrap();

        let (_idx, graph) = crate::scan::scan(&tmp);
        assert!(
            graph.routes.iter().any(|r| r.path == "/council/deliberate" && r.handler == "deliberate_handler"),
            "sanity: the Axum WS route must actually be declared, got: {:?}", graph.routes
        );
        assert!(
            graph.route_calls.iter().any(|c| c.path.contains("/council/deliberate")),
            "sanity: the websockets.connect call must actually be extracted, got: {:?}", graph.route_calls
        );

        let deps = route_call_dependents(&graph, "Council");
        assert!(
            deps.iter().any(|d| d.file == "client/council_client.py"),
            "council_client.py connects to the WS route declared alongside Council and must be reported as a dependent, got: {deps:?}"
        );

        let impact = crate::query::impact(&_idx, &graph, "Council");
        assert_ne!(
            impact["severity"], "NONE OBSERVED — declared but nothing seen touching it",
            "a WebSocket endpoint genuinely called cross-language must not report zero touchpoints, got: {impact}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

#[cfg(test)]
mod import_relationship_tests {
    use super::*;
    use std::collections::BTreeSet;

    fn known(files: &[&str]) -> BTreeSet<String> {
        files.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn resolves_a_relative_import_with_inferred_extension() {
        let files = known(&["src/a.ts", "src/utils/b.ts"]);
        let resolved = resolve_relative_import("src/a.ts", "./utils/b", &files, &BTreeMap::new());
        assert_eq!(resolved, Some("src/utils/b.ts".to_string()));
    }

    #[test]
    fn resolves_parent_relative_import() {
        let files = known(&["src/components/a.ts", "src/lib/b.ts"]);
        let resolved = resolve_relative_import("src/components/a.ts", "../lib/b", &files, &BTreeMap::new());
        assert_eq!(resolved, Some("src/lib/b.ts".to_string()));
    }

    #[test]
    fn resolves_index_file_when_directory_imported() {
        let files = known(&["src/a.ts", "src/widgets/index.ts"]);
        let resolved = resolve_relative_import("src/a.ts", "./widgets", &files, &BTreeMap::new());
        assert_eq!(resolved, Some("src/widgets/index.ts".to_string()));
    }

    /// Godot's `res://` paths are project-root-relative, not `./`/`../`-
    /// relative, and already carry an explicit extension — a direct exact
    /// lookup, no component-walking or extension-guessing.
    #[test]
    fn resolves_a_godot_res_path() {
        let files = known(&["player/Player.gd", "player/Player.tscn"]);
        let resolved = resolve_relative_import("player/Player.tscn", "res://player/Player.gd", &files, &BTreeMap::new());
        assert_eq!(resolved, Some("player/Player.gd".to_string()));
    }

    #[test]
    fn unresolved_res_path_resolves_to_nothing() {
        let files = known(&["player/Player.gd"]);
        assert_eq!(resolve_relative_import("player/Player.tscn", "res://nonexistent/Foo.gd", &files, &BTreeMap::new()), None);
    }

    #[test]
    fn external_package_import_resolves_to_nothing() {
        let files = known(&["src/a.ts"]);
        assert_eq!(resolve_relative_import("src/a.ts", "lodash", &files, &BTreeMap::new()), None);
        assert_eq!(resolve_relative_import("src/a.ts", "react", &files, &BTreeMap::new()), None);
    }

    #[test]
    fn python_dotted_relative_import_is_out_of_scope_not_guessed() {
        // Different addressing scheme (package-relative dotted, not
        // slash-separated file paths) — deliberately not attempted, per
        // Import::relationship's own doc. Must return None, not a wrong guess.
        let files = known(&["myapp/utils.py"]);
        assert_eq!(resolve_relative_import("myapp/main.py", ".utils", &files, &BTreeMap::new()), None);
    }

    #[test]
    fn ambiguous_match_across_two_extensions_resolves_to_nothing() {
        // Two real scanned files could both satisfy "./foo" — .ts and .js
        // both present. Silence is correct; guessing between them is not.
        let files = known(&["src/foo.ts", "src/foo.js"]);
        assert_eq!(resolve_relative_import("src/main.ts", "./foo", &files, &BTreeMap::new()), None);
    }

    #[test]
    fn no_match_at_all_resolves_to_nothing() {
        let files = known(&["src/other.ts"]);
        assert_eq!(resolve_relative_import("src/main.ts", "./missing", &files, &BTreeMap::new()), None);
    }

    #[test]
    fn import_relationship_carries_declared_tier_and_real_evidence_text() {
        let imp = Import { from_file: "src/a.ts".to_string(), to_module: "./b".to_string(), names: vec![] };
        let files = known(&["src/a.ts", "src/b.ts"]);
        let rel = imp.relationship(&files, &BTreeMap::new()).expect("expected a resolved relationship");
        assert_eq!(rel.from.0, "src/a.ts");
        assert_eq!(rel.to.0, "src/b.ts");
        assert_eq!(rel.kind, "imports");
        assert_eq!(rel.evidence.tier, crate::model::Tier::Declared);
        assert!(rel.evidence.what.contains("src/b.ts"));
    }

    #[test]
    fn import_relationship_is_none_for_unresolvable_import() {
        let imp = Import { from_file: "src/a.ts".to_string(), to_module: "some-package".to_string(), names: vec![] };
        let files = known(&["src/a.ts"]);
        assert!(imp.relationship(&files, &BTreeMap::new()).is_none());
    }
}

// ── Terraform extractor ──────────────────────────────────────────────────────

/// Extract Terraform resources, data sources, and modules as `Symbol` nodes,
/// and HCL interpolation references (`type.name.attribute`) as `Import` edges.
///
/// Lexical, not a full HCL parser. The HCL block grammar is regular enough
/// for the claims this extractor makes:
///   - `resource "aws_s3_bucket" "uploads"` → Symbol { name: "aws_s3_bucket.uploads", kind: Class }
///   - `data "aws_iam_policy" "base"` → Symbol { name: "data.aws_iam_policy.base", kind: Class }
///   - `module "vpc"` → Symbol { name: "module.vpc", kind: Class }
///   - `${aws_s3_bucket.uploads.arn}` inside any block → Import edge to "aws_s3_bucket.uploads"
///
/// SymbolKind::Class is the right fit: a Terraform resource is a named,
/// declared thing with dependents — the same semantic as a class or a dbt model.
///
/// Edge direction: if a Lambda resource references `aws_iam_role.worker.arn`,
/// the Lambda's file gets an Import { from_file: lambda.tf, to_module: "aws_iam_role.worker" }.
/// `impact aws_iam_role.worker` then surfaces the Lambda as a dependent.
fn extract_terraform(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    _routes: &mut Vec<Route>,
) {
    use regex::Regex;

    // resource "type" "name" { or data "type" "name" {
    let block_re = Regex::new(
        r#"(?m)^\s*(resource|data)\s+"([^"]+)"\s+"([^"]+)""#
    ).unwrap();
    for cap in block_re.captures_iter(text) {
        let block_type = &cap[1];
        let res_type = &cap[2];
        let res_name = &cap[3];
        // data sources prefixed to avoid collision with resources of same type+name
        let name = if block_type == "data" {
            format!("data.{}.{}", res_type, res_name)
        } else {
            format!("{}.{}", res_type, res_name)
        };
        symbols.push(Symbol {
            name,
            kind: SymbolKind::Class,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
            observation_source: ObservationSource::Lexical,
        });
    }

    // module "name" {
    let module_re = Regex::new(r#"(?m)^\s*module\s+"([^"]+)""#).unwrap();
    for cap in module_re.captures_iter(text) {
        symbols.push(Symbol {
            name: format!("module.{}", &cap[1]),
            kind: SymbolKind::Class,
            file: rel.to_string(),
            linked_concept: None,
            line: line_of(text, cap.get(0).unwrap().start()),
            observation_source: ObservationSource::Lexical,
        });
    }

    // Interpolation references: ${type.name.attr} or type.name.attr in expressions.
    // Matches resource-type-shaped dotted paths (at least two dot-separated
    // lowercase/underscore segments) that appear inside ${ } or as bare values.
    // data.type.name references are preserved as-is.
    let ref_re = Regex::new(
        r"\$\{([a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*){1,3})\}"
    ).unwrap();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for cap in ref_re.captures_iter(text) {
        let full_ref = cap[1].to_string();
        // Normalize: drop the trailing attribute segment (keep type.name only).
        // "aws_s3_bucket.uploads.arn" → "aws_s3_bucket.uploads"
        // "data.aws_iam_policy.base.arn" → "data.aws_iam_policy.base"
        let parts: Vec<&str> = full_ref.splitn(4, '.').collect();
        let to_module = if parts.first() == Some(&"data") && parts.len() >= 3 {
            // data.type.name[.attr] → keep first 3 segments
            parts[..3.min(parts.len())].join(".")
        } else if parts.len() >= 2 {
            // type.name[.attr] → keep first 2 segments
            parts[..2].join(".")
        } else {
            continue;
        };
        if seen.insert(to_module.clone()) {
            imports.push(Import {
                from_file: rel.to_string(),
                to_module,
                names: vec![],
            });
        }
    }
}

// ── Kubernetes manifest extractor ────────────────────────────────────────────

/// Extract Kubernetes resources as `Symbol` nodes and cross-resource
/// references as `Import` edges from YAML manifests.
///
/// Gated on content: a file must contain both `apiVersion:` and `kind:` to
/// be treated as a Kubernetes manifest — generic YAML config files (docker-
/// compose, GitHub Actions workflows, etc.) produce zero symbols and are
/// effectively skipped. This gate is why `.yaml`/`.yml` can safely be added
/// to `LANGUAGES` without turning every config file in a repo into noise.
///
/// Symbol: `{Kind}.{metadata.name}` — e.g. `Deployment.api-server`,
/// `Secret.db-password`, `ConfigMap.app-config`. SymbolKind::Class, same
/// reasoning as Terraform and dbt: a named declared infrastructure resource
/// with dependents.
///
/// Edges: scan `spec:` for common cross-resource reference fields:
///   - `configMapRef/configMapKeyRef → name:` → Import to ConfigMap.{name}
///   - `secretRef/secretKeyRef → name:` → Import to Secret.{name}
///   - `serviceAccountName:` → Import to ServiceAccount.{name}
///   - `claimName:` → Import to PersistentVolumeClaim.{name}
///
/// This is deliberately lexical/line-oriented, not a YAML parser. The subset
/// of Kubernetes YAML that carries these cross-resource references is
/// regular enough for this claim — the same "fails open, never wrong" stance
/// docker_domain.rs takes with docker-compose YAML.
fn extract_kubernetes(
    rel: &str,
    text: &str,
    symbols: &mut Vec<Symbol>,
    imports: &mut Vec<Import>,
    _routes: &mut Vec<Route>,
) {
    use regex::Regex;

    // Content gate: must look like a Kubernetes manifest.
    if !text.contains("apiVersion:") || !text.contains("kind:") {
        return;
    }

    // Support multi-document YAML (--- separator) — a single file can
    // contain multiple Kubernetes objects.
    let docs: Vec<&str> = text.split("\n---").collect();

    let kind_re = Regex::new(r"(?m)^kind:\s*(\S+)").unwrap();
    let name_re = Regex::new(r"(?m)^\s{0,2}name:\s*(\S+)").unwrap();

    // Cross-resource reference patterns
    let configmap_ref_re = Regex::new(r"(?m)configMap(?:Ref|KeyRef)?\s*:\s*\n\s+name:\s*(\S+)").unwrap();
    let configmap_name_re = Regex::new(r"(?m)configMapRef:\s*\n\s+name:\s*(\S+)").unwrap();
    let secret_ref_re = Regex::new(r"(?m)secret(?:Ref|KeyRef)?\s*:\s*\n\s+name:\s*(\S+)").unwrap();
    let svc_account_re = Regex::new(r"(?m)serviceAccountName:\s*(\S+)").unwrap();
    let claim_re = Regex::new(r"(?m)claimName:\s*(\S+)").unwrap();
    // inline configMapRef: {name: foo}
    let inline_cm_re = Regex::new(r"configMapRef:\s*\{[^}]*name:\s*(\S+?)[},]").unwrap();
    let inline_secret_re = Regex::new(r"secretRef:\s*\{[^}]*name:\s*(\S+?)[},]").unwrap();

    for doc in &docs {
        if !doc.contains("apiVersion:") || !doc.contains("kind:") {
            continue;
        }

        let kind = match kind_re.captures(doc) {
            Some(c) => c[1].trim().to_string(),
            None => continue,
        };

        // metadata.name: first `name:` at root/near-root indentation
        let resource_name = match name_re.captures(doc) {
            Some(c) => c[1].trim().trim_end_matches('"').trim_start_matches('"').to_string(),
            None => continue,
        };
        if resource_name.is_empty() {
            continue;
        }

        let sym_name = format!("{}.{}", kind, resource_name);
        let sym_file = rel.to_string();

        // Find approximate line number of `kind:` declaration
        let kind_line = kind_re
            .find(doc)
            .map(|m| line_of(doc, m.start()))
            .unwrap_or(1);

        symbols.push(Symbol {
            name: sym_name.clone(),
            kind: SymbolKind::Class,
            file: sym_file.clone(),
            linked_concept: None,
            line: kind_line,
            observation_source: ObservationSource::Lexical,
        });

        // Collect cross-resource references, deduped
        let mut refs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

        for cap in configmap_ref_re.captures_iter(doc) {
            refs.insert(format!("ConfigMap.{}", cap[1].trim()));
        }
        for cap in configmap_name_re.captures_iter(doc) {
            refs.insert(format!("ConfigMap.{}", cap[1].trim()));
        }
        for cap in inline_cm_re.captures_iter(doc) {
            refs.insert(format!("ConfigMap.{}", cap[1].trim()));
        }
        for cap in secret_ref_re.captures_iter(doc) {
            refs.insert(format!("Secret.{}", cap[1].trim()));
        }
        for cap in inline_secret_re.captures_iter(doc) {
            refs.insert(format!("Secret.{}", cap[1].trim()));
        }
        for cap in svc_account_re.captures_iter(doc) {
            let name = cap[1].trim();
            // Skip default service account — it's implicit everywhere, not
            // a meaningful cross-resource dependency worth surfacing.
            if name != "default" {
                refs.insert(format!("ServiceAccount.{}", name));
            }
        }
        for cap in claim_re.captures_iter(doc) {
            refs.insert(format!("PersistentVolumeClaim.{}", cap[1].trim()));
        }

        for to_module in refs {
            // Don't create self-edges (a resource referencing itself)
            if to_module != sym_name {
                imports.push(Import {
                    from_file: sym_file.clone(),
                    to_module,
                    names: vec![],
                });
            }
        }
    }
}
