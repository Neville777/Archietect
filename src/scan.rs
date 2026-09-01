//! Extraction — incremental, per-file, dependency-aware.
//!
//! ## The incremental model (compiler thinking)
//!
//! Extraction is PURE per file: a file's (size, mtime, extractor version)
//! decides whether it is re-read; unchanged files contribute their cached
//! fragments. The global graph is then ASSEMBLED from fragments — assembly is
//! cheap, so it always runs fresh (merge laws, aliases, provenance).
//!
//! Usage has a dependency the cache must honour: matchers are built FROM the
//! concept set. So invalidation follows the compiler rule exactly —
//!
//! ```text
//! a changed code file invalidates ITSELF;
//! a changed CONCEPT SET invalidates ALL usage.
//! ```
//!
//! `concepts_sig` (names + tables, hashed) detects the second case. Editing a
//! service file re-reads one file; editing schema.prisma re-runs the usage
//! pass everywhere, because every cached answer about "who uses what" was
//! computed against a set that no longer exists.
//!
//! ## Extractor laws (see VALIDATION.md — every one came from a wrong answer)
//!
//! Declarations: Prisma, Django, pydantic/SQLModel (`table=True` declares
//! storage regardless of base names), SQLAlchemy, CREATE TABLE from ALL
//! sources — with comment lines stripped and a `(`/AS follower required,
//! because prose about schema is not schema (a doc comment and a log message
//! each minted phantom concepts that defeated the guard).

use crate::model::{Concept, DeclFragment, FileFacts, Index};
use crate::structural::{self, StructuralGraph};
use rayon::prelude::*;
use regex::{Regex, RegexBuilder};
use std::collections::BTreeMap;
use std::path::Path;
use walkdir::WalkDir;

/// Bump to invalidate every cached extraction (a changed extractor is a
/// changed compiler — old object files are lies).
pub const EXTRACTOR_VERSION: u32 = 9; // +rust pub-struct extractor, +ontology-before-name-search (law-011)

const SKIP_DIRS: &[&str] = &[
    "node_modules", ".git", ".next", "target", "dist", "build", "__pycache__",
    ".venv", "venv", ".turbo", "coverage", ".cache", "vendor",
];
const MAX_FILE_BYTES: u64 = 2_000_000;
/// Schema-declaration formats structural.rs has no reason to know about —
/// not "code" in the symbols/routes sense, so they live outside
/// `structural::LANGUAGES`, but the schema extractors above still need them
/// walked.
const SCHEMA_ONLY_EXTS: &[&str] = &["prisma", "sql"];

/// LAW-014: the file-walk's scannable-extension filter is DERIVED from
/// `structural::LANGUAGES`/`structural::KNOWN_UNSUPPORTED` — never a second
/// hand-maintained list. A hand-maintained SRC_EXTS here once drifted from
/// the extractor registry (Kotlin was added to LANGUAGES but not to
/// SRC_EXTS, so every .kt file was silently unreachable through the real
/// scan path despite its extractor compiling and its unit test passing).
fn is_scannable_ext(ext: &str) -> bool {
    SCHEMA_ONLY_EXTS.contains(&ext)
        || structural::LANGUAGES.iter().any(|l| l.extensions.contains(&ext))
        || structural::KNOWN_UNSUPPORTED.iter().any(|(_, exts)| exts.contains(&ext))
}

/// One file the scan pipeline walked — schema and structural extraction
/// both operate over the same inventory. Public: `structural::extract`
/// takes a slice of these directly, so the file-walk step is not
/// duplicated between the schema pass and the structural pass.
pub struct ScannableFile {
    pub path: std::path::PathBuf,
    pub rel: String,
    pub size: u64,
    pub mtime_ms: i64,
}

fn skip_dir(e: &walkdir::DirEntry) -> bool {
    e.file_type().is_dir()
        && e.file_name().to_str().map(|n| SKIP_DIRS.contains(&n)).unwrap_or(false)
}

/// Returns true if the entry's root-relative path starts with any of the
/// user-declared exclude prefixes (architect.toml `exclude = ["validation/",
/// "fixtures/"]`). Trailing slash on a prefix is stripped before comparison
/// so both `"validation"` and `"validation/"` work.
fn skip_excluded(e: &walkdir::DirEntry, root: &Path, excludes: &[String]) -> bool {
    if excludes.is_empty() {
        return false;
    }
    let rel = match e.path().strip_prefix(root) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let rel_str = rel.to_string_lossy();
    excludes.iter().any(|ex| {
        let ex = ex.trim_end_matches('/');
        rel_str == ex || rel_str.starts_with(&format!("{ex}/"))
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn scan(root: &Path) -> (Index, StructuralGraph) {
    let (schema_prior, graph_prior) = crate::store::load_raw(root);
    scan_with_prior(root, schema_prior, graph_prior)
}

pub fn scan_with_prior(
    root: &Path,
    prior: Option<Index>,
    graph_prior: Option<StructuralGraph>,
) -> (Index, StructuralGraph) {
    let prior = prior.filter(|p| p.extractor_version == EXTRACTOR_VERSION);
    let mut idx = Index {
        root: root.display().to_string(),
        extractor_version: EXTRACTOR_VERSION,
        ..Default::default()
    };

    // ── ontology + decisions: always re-read (one small file) ───────────────
    if let Ok(t) = std::fs::read_to_string(root.join("architect.toml")) {
        if let Ok(v) = t.parse::<toml::Value>() {
            if let Some(al) = v.get("aliases").and_then(|a| a.as_table()) {
                for (k, val) in al {
                    if let Some(s) = val.as_str() {
                        idx.aliases.insert(k.to_lowercase(), s.to_string());
                    }
                }
            }
            if let Some(ds) = v.get("decision").and_then(|d| d.as_array()) {
                for d in ds {
                    let g = |k: &str| d.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let arr = |k: &str| {
                        d.get(k)
                            .and_then(|x| x.as_array())
                            .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                            .unwrap_or_default()
                    };
                    idx.decisions.push(crate::model::Decision {
                        id: g("id"),
                        decision: g("decision"),
                        because: g("because"),
                        rejected: arr("rejected"),
                        links: arr("links"),
                    });
                }
            }
            idx.declaration_files.push(("architect.toml".into(), "ontology".into()));
        }
    }

    // ── exclude list: paths declared in architect.toml [exclude] ────────────
    // Loaded separately (after the toml block above) so the walker below can
    // use it. Supports both string and array forms:
    //   exclude = ["validation/", "fixtures/"]
    let excludes: Vec<String> = std::fs::read_to_string(root.join("architect.toml"))
        .ok()
        .and_then(|t| t.parse::<toml::Value>().ok())
        .and_then(|v| {
            v.get("exclude").cloned().map(|e| match e {
                toml::Value::Array(arr) => arr
                    .into_iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect(),
                toml::Value::String(s) => vec![s],
                _ => vec![],
            })
        })
        .unwrap_or_default();

    idx.excludes = excludes.clone();

    // ── file inventory with metadata ────────────────────────────────────────
    let files: Vec<ScannableFile> = WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !skip_dir(e) && !skip_excluded(e, root, &excludes))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(is_scannable_ext)
                .unwrap_or(false)
        })
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            if m.len() > MAX_FILE_BYTES {
                return None;
            }
            let mtime_ms = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let rel = e.path().strip_prefix(root).unwrap_or(e.path()).display().to_string();
            Some(ScannableFile { path: e.into_path(), rel, size: m.len(), mtime_ms })
        })
        .collect();
    idx.files_scanned = files.len();

    let prior_facts: BTreeMap<String, FileFacts> =
        prior.as_ref().map(|p| p.file_facts.clone()).unwrap_or_default();
    let unchanged = |f: &ScannableFile| {
        prior_facts
            .get(&f.rel)
            .map(|pf| pf.size == f.size && pf.mtime_ms == f.mtime_ms)
            .unwrap_or(false)
    };

    // ── pass 1: DECLARATIONS — changed files re-extracted, rest from cache ──
    let decl_results: Vec<(String, u64, i64, Vec<DeclFragment>, Vec<String>)> = files
        .par_iter()
        .map(|f| {
            if unchanged(f) {
                let pf = &prior_facts[&f.rel];
                return (f.rel.clone(), f.size, f.mtime_ms, pf.decls.clone(), pf.decl_kinds.clone());
            }
            let Ok(text) = std::fs::read_to_string(&f.path) else {
                return (f.rel.clone(), f.size, f.mtime_ms, Vec::new(), Vec::new());
            };
            let (decls, kinds) = extract_declarations(&f.path, &text);
            (f.rel.clone(), f.size, f.mtime_ms, decls, kinds)
        })
        .collect();

    for (rel, size, mtime_ms, decls, decl_kinds) in &decl_results {
        idx.file_facts.insert(
            rel.clone(),
            FileFacts {
                size: *size,
                mtime_ms: *mtime_ms,
                decls: decls.clone(),
                usage: Vec::new(),
                decl_kinds: decl_kinds.clone(),
            },
        );
        for k in decl_kinds {
            idx.declaration_files.push((rel.clone(), k.clone()));
        }
    }

    // ── assemble concepts from fragments (always fresh — cheap) ─────────────
    let now = now_ms();
    for (rel, ff) in &idx.file_facts.clone() {
        for d in &ff.decls {
            let c = idx.concepts.entry(d.name.clone()).or_insert_with(|| Concept {
                name: d.name.clone(),
                first_seen_ms: now,
                ..Default::default()
            });
            c.declared_in.push((rel.clone(), d.kind.clone()));
            if c.fields.is_empty() {
                c.fields = d.fields.clone();
            }
            if c.relations.is_empty() {
                c.relations = d.relations.clone();
            }
            if c.table.is_none() {
                c.table = d.table.clone();
            }
        }
    }
    // Provenance: FIRST SEEN survives rescans — that is what makes this
    // memory instead of cache. last_verified is this assembly.
    if let Some(pr) = &prior {
        for (name, c) in idx.concepts.iter_mut() {
            if let Some(old) = pr.concepts.get(name) {
                if old.first_seen_ms > 0 {
                    c.first_seen_ms = old.first_seen_ms;
                }
            }
        }
    }
    for c in idx.concepts.values_mut() {
        c.last_verified_ms = now;
    }

    // Merge law: declarations sharing a TABLE are one concept.
    let merges: Vec<(String, String)> = idx
        .concepts
        .iter()
        .filter(|(_, c)| c.declared_in.iter().all(|(_, k)| k == "sql"))
        .filter_map(|(name, _)| {
            idx.concepts
                .iter()
                .find(|(n2, c2)| {
                    *n2 != name
                        && c2.table.as_deref().map(|t| t.eq_ignore_ascii_case(name)).unwrap_or(false)
                })
                .map(|(target, _)| (name.clone(), target.clone()))
        })
        .collect();
    for (dupe, target) in merges {
        if let Some(d) = idx.concepts.remove(&dupe) {
            let t = idx.concepts.get_mut(&target).unwrap();
            t.declared_in.extend(d.declared_in);
            if d.first_seen_ms > 0 {
                t.first_seen_ms = t.first_seen_ms.min(d.first_seen_ms);
            }
            if t.table.is_none() {
                t.table = d.table;
            }
        }
    }

    // ── concept-set signature: the dependency edge schema → usage ───────────
    let mut sig_src: Vec<String> = idx
        .concepts
        .iter()
        .map(|(n, c)| format!("{n}:{}", c.table.as_deref().unwrap_or("")))
        .collect();
    sig_src.sort();
    idx.concepts_sig = {
        // FNV-1a — change detection, not cryptography
        let mut h: u64 = 0xcbf29ce484222325;
        for b in sig_src.join("|").bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{h:x}")
    };
    let usage_cache_valid =
        prior.as_ref().map(|p| p.concepts_sig == idx.concepts_sig).unwrap_or(false);

    // ── pass 2: USAGE — matchers built once from the assembled set ──────────
    struct Matcher {
        concept: String,
        needle_client: String,
        client_re: Regex,
        needle_django: String,
        needle_construct: String,
        construct_re: Regex,
        static_re: Option<Regex>,
        repo_re: Option<Regex>,
        rails_re: Option<Regex>,
        drizzle_re: Option<Regex>,
        eloquent_re: Option<Regex>,
        gorm_re: Option<Regex>,
        table_re: Option<Regex>,
        needle_table: Option<String>,
    }
    let matchers: Vec<Matcher> = idx
        .concepts
        .keys()
        .map(|name| {
            let mut lname = name.clone();
            if let Some(c) = lname.get_mut(0..1) {
                c.make_ascii_lowercase();
            }
            let client_re =
                Regex::new(&format!(r"\b(?:prisma|db|tx|client)\.{}\.", regex::escape(&lname)))
                    .unwrap();
            let c0 = &idx.concepts[name];
            let has_kind = |k: &str| c0.declared_in.iter().any(|(_, dk)| dk == k);
            // Dialect matchers are built ONLY for concepts DECLARED in that
            // dialect — `Anything.find(` in unrelated code must not inflate
            // unrelated concepts.
            let static_re = if has_kind("mongoose") {
                Some(Regex::new(&format!(
                    r"\b{}\.(?:find|findOne|findById|create|updateOne|deleteOne|countDocuments|aggregate|exists)\(",
                    regex::escape(name)
                )).unwrap())
            } else { None };
            let rails_re = if has_kind("rails") {
                Some(Regex::new(&format!(
                    r"\b{}\.(?:find|find_by|where|create|new|all|first|joins|includes|count)\b",
                    regex::escape(name)
                )).unwrap())
            } else { None };
            let drizzle_re = if has_kind("drizzle") {
                Some(Regex::new(&format!(
                    r"(?:from|insert|update|delete)\(\s*{}\s*[,)]",
                    regex::escape(name)
                )).unwrap())
            } else { None };
            let eloquent_re = if has_kind("eloquent") {
                // Book::query(), Book::where(...) — PHP static access
                Some(Regex::new(&format!(r"\b{}::", regex::escape(name))).unwrap())
            } else { None };
            let gorm_re = if has_kind("gorm") {
                // &ArticleModel{...} / ArticleModel{} — Go struct literals
                Some(Regex::new(&format!(r"\b{}\{{", regex::escape(name))).unwrap())
            } else { None };
            let repo_re = if has_kind("typeorm") {
                Some(Regex::new(&format!(
                    r"(?:Repository<{0}>|getRepository\({0}\)|InjectRepository\({0}\))",
                    regex::escape(name)
                )).unwrap())
            } else { None };
            let table = c0.table.clone();
            let table_re = table.as_ref().map(|t| {
                RegexBuilder::new(&format!(
                    r#"(?:insert\s+into|update|from|join)\s+["'`]?{}\b"#,
                    regex::escape(&t.to_lowercase())
                ))
                .case_insensitive(true)
                .build()
                .unwrap()
            });
            Matcher {
                concept: name.clone(),
                needle_client: format!(".{lname}."),
                client_re,
                needle_django: format!("{name}.objects."),
                needle_construct: format!("{name}("),
                construct_re: Regex::new(&format!(r"\b{}\(", regex::escape(name))).unwrap(),
                static_re,
                repo_re,
                rails_re,
                drizzle_re,
                eloquent_re,
                gorm_re,
                needle_table: table.map(|t| t.to_lowercase()),
                table_re,
            }
        })
        .collect();

    let usage_results: Vec<(String, Vec<(String, String)>)> = files
        .par_iter()
        .filter(|f| f.path.extension().and_then(|x| x.to_str()) != Some("prisma"))
        .map(|f| {
            // The compiler rule: reuse cached usage ONLY if this file is
            // unchanged AND the concept set it was computed against is the
            // one that exists now.
            if usage_cache_valid && unchanged(f) {
                return (f.rel.clone(), prior_facts[&f.rel].usage.clone());
            }
            let Ok(text) = std::fs::read_to_string(&f.path) else {
                return (f.rel.clone(), Vec::new());
            };
            let lower = text.to_lowercase();
            let mut hits = Vec::new();
            for m in &matchers {
                if text.contains(&m.needle_client) && m.client_re.is_match(&text) {
                    hits.push((m.concept.clone(), "orm-client".to_string()));
                }
                if text.contains(&m.needle_django) {
                    hits.push((m.concept.clone(), "django-orm".to_string()));
                }
                if m.concept.len() >= 5
                    && text.contains(&m.needle_construct)
                    && m.construct_re.is_match(&text)
                {
                    hits.push((m.concept.clone(), "constructed".to_string()));
                }
                if let Some(re) = &m.static_re {
                    if text.contains(m.concept.as_str()) && re.is_match(&text) {
                        hits.push((m.concept.clone(), "mongoose-static".to_string()));
                    }
                }
                if let Some(re) = &m.rails_re {
                    if text.contains(m.concept.as_str()) && re.is_match(&text) {
                        hits.push((m.concept.clone(), "rails-static".to_string()));
                    }
                }
                if let Some(re) = &m.eloquent_re {
                    if text.contains(m.concept.as_str()) && re.is_match(&text) {
                        hits.push((m.concept.clone(), "eloquent-static".to_string()));
                    }
                }
                if let Some(re) = &m.gorm_re {
                    if text.contains(m.concept.as_str()) && re.is_match(&text) {
                        hits.push((m.concept.clone(), "gorm-literal".to_string()));
                    }
                }
                if let Some(re) = &m.drizzle_re {
                    if text.contains(m.concept.as_str()) && re.is_match(&text) {
                        hits.push((m.concept.clone(), "drizzle-query".to_string()));
                    }
                }
                if let Some(re) = &m.repo_re {
                    if text.contains(m.concept.as_str()) && re.is_match(&text) {
                        hits.push((m.concept.clone(), "typeorm-repository".to_string()));
                    }
                }
                if let (Some(needle), Some(re)) = (&m.needle_table, &m.table_re) {
                    if lower.contains(needle.as_str()) && re.is_match(&lower) {
                        hits.push((m.concept.clone(), "raw-sql".to_string()));
                    }
                }
            }
            (f.rel.clone(), hits)
        })
        .collect();

    for (rel, hits) in usage_results {
        if let Some(ff) = idx.file_facts.get_mut(&rel) {
            ff.usage = hits.clone();
        }
        for (concept, kind) in hits {
            let Some(c) = idx.concepts.get_mut(&concept) else { continue };
            if c.declared_in.iter().any(|(f, _)| *f == rel) {
                continue; // a declaration file "using" its own concept is not usage
            }
            c.usage.push((rel.clone(), kind));
        }
    }
    for c in idx.concepts.values_mut() {
        c.usage.sort();
        c.usage.dedup();
    }

    // ── structural graph: independent of the schema pass (see structural.rs
    // module doc — no concepts_sig-style invalidation here, just per-file
    // size/mtime/extractor_version caching), linked to concepts once both
    // are assembled.
    let mut graph = structural::extract(root, &files, graph_prior.as_ref());
    structural::link_to_concepts(&mut graph, &idx.concepts);

    (idx, graph)
}

// ── per-file declaration extraction (pure) ───────────────────────────────────

fn extract_declarations(path: &Path, text: &str) -> (Vec<DeclFragment>, Vec<String>) {
    let name = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
    let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("");
    let mut decls = Vec::new();
    let mut kinds = Vec::new();

    if ext == "rs" {
        let before = decls.len();
        extract_rust(text, &mut decls);
        if decls.len() > before {
            kinds.push("rust".into());
        }
    }
    if ext == "prisma" {
        extract_prisma(text, &mut decls);
        if !decls.is_empty() {
            kinds.push("prisma".into());
        }
    }
    if name == "models.py" && text.contains("models.Model") {
        let before = decls.len();
        extract_django(text, &mut decls);
        if decls.len() > before {
            kinds.push("django".into());
        }
    }
    if ext == "py"
        && (text.contains("BaseModel") || text.contains("SQLModel") || text.contains("table=True"))
    {
        let before = decls.len();
        extract_pydantic(text, &mut decls);
        if decls.len() > before {
            kinds.push("pydantic".into());
        }
    }
    if ext == "py" && (text.contains("db.Model") || text.contains("__tablename__")) {
        let before = decls.len();
        extract_sqlalchemy(text, &mut decls);
        if decls.len() > before {
            kinds.push("sqlalchemy".into());
        }
    }
    if matches!(ext, "js" | "ts") && text.contains("mongoose.model(") {
        let before = decls.len();
        extract_mongoose(text, &mut decls);
        if decls.len() > before {
            kinds.push("mongoose".into());
        }
    }
    if ext == "ts" && text.contains("@Entity") {
        let before = decls.len();
        extract_typeorm(text, &mut decls);
        if decls.len() > before {
            kinds.push("typeorm".into());
        }
    }
    if ext == "rb" {
        let before = decls.len();
        extract_rails(name, text, &mut decls);
        if decls.len() > before {
            kinds.push(if name == "schema.rb" { "rails-schema".into() } else { "rails".into() });
        }
    }
    if matches!(ext, "ts" | "js") && (text.contains("pgTable(") || text.contains("sqliteTable(") || text.contains("mysqlTable(")) {
        let before = decls.len();
        extract_drizzle(text, &mut decls);
        if decls.len() > before {
            kinds.push("drizzle".into());
        }
    }
    if ext == "php" && text.contains("class ") && (path_has_models_dir(path) || text.contains("extends Model")) {
        let before = decls.len();
        extract_eloquent(text, &mut decls);
        if decls.len() > before {
            kinds.push("eloquent".into());
        }
    }
    if ext == "java" && text.contains("@Entity") {
        let before = decls.len();
        extract_jpa(text, &mut decls);
        if decls.len() > before {
            kinds.push("jpa".into());
        }
    }
    if ext == "go" && text.contains("gorm") {
        let before = decls.len();
        extract_gorm(text, &mut decls);
        if decls.len() > before {
            kinds.push("gorm".into());
        }
    }
    if matches!(ext, "ex" | "exs") && text.contains("schema \"") {
        let before = decls.len();
        extract_ecto(text, &mut decls);
        if decls.len() > before {
            kinds.push("ecto".into());
        }
    }
    if text.contains("CREATE TABLE") || text.contains("create table") {
        let before = decls.len();
        extract_sql(ext == "sql", text, &mut decls);
        if decls.len() > before {
            kinds.push("sql".into());
        }
    }
    (decls, kinds)
}

const PRISMA_SCALARS: &[&str] = &[
    "String", "Int", "BigInt", "Float", "Decimal", "Boolean", "DateTime", "Json", "Bytes",
];

fn extract_prisma(text: &str, out: &mut Vec<DeclFragment>) {
    let model_re = Regex::new(r"(?ms)^model\s+(\w+)\s*\{(.*?)^\}").unwrap();
    let map_re = Regex::new(r#"@@map\("([^"]+)"\)"#).unwrap();
    let field_re = Regex::new(r"^(\w+)\s+(\w+)").unwrap();
    for cap in model_re.captures_iter(text) {
        let (name, body) = (&cap[1], &cap[2]);
        let mut fields = Vec::new();
        let mut relations = Vec::new();
        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with("//") || line.starts_with("@@") {
                continue;
            }
            if let Some(f) = field_re.captures(line) {
                fields.push(f[1].to_string());
                let ftype = &f[2];
                if ftype.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false)
                    && !PRISMA_SCALARS.contains(&ftype)
                {
                    relations.push(ftype.to_string());
                }
            }
        }
        relations.sort();
        relations.dedup();
        let table =
            map_re.captures(body).map(|m| m[1].to_string()).unwrap_or_else(|| name.to_string());
        out.push(DeclFragment {
            name: name.to_string(),
            kind: "prisma".into(),
            fields,
            relations,
            table: Some(table),
        });
    }
    let enum_re = Regex::new(r"(?m)^enum\s+(\w+)\s*\{").unwrap();
    for cap in enum_re.captures_iter(text) {
        out.push(DeclFragment {
            name: cap[1].to_string(),
            kind: "prisma-enum".into(),
            ..Default::default()
        });
    }
}

fn extract_django(text: &str, out: &mut Vec<DeclFragment>) {
    let class_re = Regex::new(r"(?m)^class\s+(\w+)\s*\([^)]*Model[^)]*\)\s*:").unwrap();
    let field_re = Regex::new(r"(?m)^    (\w+)\s*=\s*models\.").unwrap();
    let rel_re =
        Regex::new(r#"models\.(?:ForeignKey|OneToOneField|ManyToManyField)\(\s*['"]?(\w+)"#)
            .unwrap();
    let top_re = Regex::new(r"(?m)^\S").unwrap();
    let starts: Vec<(usize, String)> = class_re
        .captures_iter(text)
        .map(|c| (c.get(0).unwrap().end(), c[1].to_string()))
        .collect();
    for (start, name) in starts {
        let body_end = top_re.find_at(text, start).map(|m| m.start()).unwrap_or(text.len());
        let body = &text[start..body_end];
        let fields: Vec<String> = field_re.captures_iter(body).map(|f| f[1].to_string()).collect();
        let mut relations: Vec<String> = rel_re
            .captures_iter(body)
            .map(|r| r[1].to_string())
            .filter(|r| r != "self")
            .collect();
        relations.sort();
        relations.dedup();
        // table stays None: Django's real table name needs the app label, and
        // we do not guess.
        out.push(DeclFragment { name, kind: "django".into(), fields, relations, table: None });
    }
}

fn extract_pydantic(text: &str, out: &mut Vec<DeclFragment>) {
    // Bases captured raw and filtered in the loop: `class Item(ItemBase,
    // table=True)` names neither BaseModel nor SQLModel, yet table=True IS
    // the storage declaration (validation law 6).
    let class_re = Regex::new(r"(?m)^class\s+(\w+)\s*\(([^)]*)\)\s*:").unwrap();
    let field_re = Regex::new(r"(?m)^    (\w+)\s*:").unwrap();
    let top_re = Regex::new(r"(?m)^\S").unwrap();
    let starts: Vec<(usize, String, String)> = class_re
        .captures_iter(text)
        .map(|c| (c.get(0).unwrap().end(), c[1].to_string(), c[2].to_string()))
        .collect();
    for (start, name, bases) in starts {
        let is_table = bases.contains("table=True") || bases.contains("table = True");
        if !is_table && !bases.contains("BaseModel") && !bases.contains("SQLModel") {
            continue;
        }
        let body_end = top_re.find_at(text, start).map(|m| m.start()).unwrap_or(text.len());
        let body = &text[start..body_end];
        let fields: Vec<String> = field_re.captures_iter(body).map(|f| f[1].to_string()).collect();
        let kind = if is_table || bases.contains("SQLModel") { "sqlmodel" } else { "pydantic" };
        // SQLModel's default table name is the lowercased class name — a
        // framework-documented rule, not a guess. Contracts get None.
        let table = if is_table { Some(name.to_lowercase()) } else { None };
        out.push(DeclFragment { name, kind: kind.into(), fields, relations: Vec::new(), table });
    }
}

fn extract_sqlalchemy(text: &str, out: &mut Vec<DeclFragment>) {
    let class_re = Regex::new(r"(?m)^class\s+(\w+)\s*\(([^)]*)\)\s*:").unwrap();
    let tname_re = Regex::new(r#"__tablename__\s*=\s*["']([^"']+)["']"#).unwrap();
    let field_re = Regex::new(r"(?m)^    (\w+)\s*=\s*(?:db\.)?Column\(").unwrap();
    let top_re = Regex::new(r"(?m)^\S").unwrap();
    let starts: Vec<(usize, String, String)> = class_re
        .captures_iter(text)
        .map(|c| (c.get(0).unwrap().end(), c[1].to_string(), c[2].to_string()))
        .collect();
    for (start, name, bases) in starts {
        let body_end = top_re.find_at(text, start).map(|m| m.start()).unwrap_or(text.len());
        let body = &text[start..body_end];
        let tname = tname_re.captures(body).map(|m| m[1].to_string());
        // a SQLAlchemy model states a table OR inherits db.Model; a bare
        // "Base" parent without __tablename__ is too ambiguous to assert.
        if tname.is_none() && !bases.contains("db.Model") {
            continue;
        }
        let fields: Vec<String> = field_re.captures_iter(body).map(|f| f[1].to_string()).collect();
        out.push(DeclFragment {
            name,
            kind: "sqlalchemy".into(),
            fields,
            relations: Vec::new(),
            table: tname,
        });
    }
}

fn extract_sql(is_sql_file: bool, text: &str, out: &mut Vec<DeclFragment>) {
    // Comment lines stripped for non-.sql sources (law 3) and a `(`/AS
    // follower required (law 8): prose about schema is not schema.
    let filtered;
    let text: &str = if is_sql_file {
        text
    } else {
        filtered = text
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                !(t.starts_with("//") || t.starts_with('#') || t.starts_with("--") || t.starts_with('*'))
            })
            .collect::<Vec<_>>()
            .join("\n");
        &filtered
    };
    let re = RegexBuilder::new(
        r#"create\s+table\s+(?:if\s+not\s+exists\s+)?["'`]?(\w+)["'`]?\s*(?:\(|as\b)"#,
    )
    .case_insensitive(true)
    .build()
    .unwrap();
    for cap in re.captures_iter(text) {
        let name = cap[1].to_string();
        out.push(DeclFragment {
            name: name.clone(),
            kind: "sql".into(),
            fields: Vec::new(),
            relations: Vec::new(),
            table: Some(name),
        });
    }
}

fn extract_mongoose(text: &str, out: &mut Vec<DeclFragment>) {
    // mongoose.model('User', userSchema) is the declaration. The default
    // collection name is mongoose's own pluralizer — table stays None rather
    // than guessing an inflection engine's output.
    let model_re = Regex::new(r#"mongoose\.model\(\s*['"](\w+)['"]"#).unwrap();
    let ref_re = Regex::new(r#"ref:\s*['"](\w+)['"]"#).unwrap();
    let fields: Vec<String> = text
        .find("Schema({")
        .map(|i| {
            let body = &text[i..text.len().min(i + 4000)];
            Regex::new(r"(?m)^\s{2}(\w+)\s*:")
                .unwrap()
                .captures_iter(body)
                .map(|c| c[1].to_string())
                .take(30)
                .collect()
        })
        .unwrap_or_default();
    let mut relations: Vec<String> = ref_re.captures_iter(text).map(|c| c[1].to_string()).collect();
    relations.sort();
    relations.dedup();
    for cap in model_re.captures_iter(text) {
        out.push(DeclFragment {
            name: cap[1].to_string(),
            kind: "mongoose".into(),
            fields: fields.clone(),
            relations: relations.clone(),
            table: None,
        });
    }
}

fn extract_typeorm(text: &str, out: &mut Vec<DeclFragment>) {
    // @Entity() / @Entity('table_name') followed by `export class X`.
    // Explicit table names are read; the DEFAULT is a naming-strategy output
    // (varies per project), so absent an explicit name, table stays None.
    let ent_re =
        Regex::new(r#"@Entity\(\s*(?:['"]([^'"]+)['"])?\s*\)[\s\S]{0,200}?export class (\w+)"#)
            .unwrap();
    let col_re =
        Regex::new(r"(?m)@(?:Primary\w*Column|Column)\([^)]*\)\s*\n\s*(\w+)[?!]?\s*[:;]").unwrap();
    let rel_re = Regex::new(
        r"@(?:ManyToOne|OneToMany|OneToOne|ManyToMany)\(\s*(?:type\s*=>|\(\)\s*=>)\s*(\w+)",
    )
    .unwrap();
    let fields: Vec<String> = col_re.captures_iter(text).map(|c| c[1].to_string()).collect();
    let mut relations: Vec<String> = rel_re.captures_iter(text).map(|c| c[1].to_string()).collect();
    relations.sort();
    relations.dedup();
    for cap in ent_re.captures_iter(text) {
        out.push(DeclFragment {
            name: cap[2].to_string(),
            kind: "typeorm".into(),
            fields: fields.clone(),
            relations: relations.clone(),
            table: cap.get(1).map(|m| m.as_str().to_string()),
        });
    }
}

fn extract_rails(fname: &str, text: &str, out: &mut Vec<DeclFragment>) {
    if fname == "schema.rb" {
        // db/schema.rb is Rails' own generated DDL — `create_table "users"` is
        // an authoritative table declaration. Emitted as kind "sql" so the
        // same-table merge law and the ORM-outranks-sql ranking treat it as
        // what it is: strong about the TABLE, weak about the concept.
        let re = Regex::new(r#"create_table\s+"(\w+)""#).unwrap();
        for cap in re.captures_iter(text) {
            let name = cap[1].to_string();
            out.push(DeclFragment {
                name: name.clone(),
                kind: "sql".into(),
                fields: Vec::new(),
                relations: Vec::new(),
                table: Some(name),
            });
        }
        return;
    }
    // app/models/*.rb: class X < ApplicationRecord. The table name is Rails'
    // pluralizer output — None rather than imitating an inflection engine
    // (same refusal as mongoose).
    let class_re =
        Regex::new(r"(?m)^class\s+(\w+)\s*<\s*(?:ApplicationRecord|ActiveRecord::Base)").unwrap();
    let rel_re =
        Regex::new(r"(?m)^\s+(?:belongs_to|has_many|has_one|has_and_belongs_to_many)\s+:(\w+)")
            .unwrap();
    let mut relations: Vec<String> = rel_re.captures_iter(text).map(|c| c[1].to_string()).collect();
    relations.sort();
    relations.dedup();
    for cap in class_re.captures_iter(text) {
        out.push(DeclFragment {
            name: cap[1].to_string(),
            kind: "rails".into(),
            fields: Vec::new(), // columns live in schema.rb, not the model file
            relations: relations.clone(),
            table: None,
        });
    }
}

fn extract_drizzle(text: &str, out: &mut Vec<DeclFragment>) {
    // export const users = pgTable('users', {...}) — the concept is the
    // exported binding, the table name is EXPLICIT in the first argument.
    let tbl_re = Regex::new(
        r#"(?:export\s+)?const\s+(\w+)\s*=\s*(?:pg|sqlite|mysql)Table\(\s*['"](\w+)['"]"#,
    )
    .unwrap();
    let ref_re = Regex::new(r"references\(\s*\(\)\s*=>\s*(\w+)\.").unwrap();
    let mut relations: Vec<String> = ref_re.captures_iter(text).map(|c| c[1].to_string()).collect();
    relations.sort();
    relations.dedup();
    for cap in tbl_re.captures_iter(text) {
        let name = cap[1].to_string();
        out.push(DeclFragment {
            name: name.clone(),
            kind: "drizzle".into(),
            fields: Vec::new(),
            relations: relations.iter().filter(|r| **r != name).cloned().collect(),
            table: Some(cap[2].to_string()),
        });
    }
}

fn path_has_models_dir(p: &Path) -> bool {
    // Laravel convention: models live under a Models/ directory. BookStack's
    // Book extends a project base class (Entity), not Model directly — the
    // directory convention IS the declaration convention in Laravel, and it
    // is a stated framework norm, not a guess.
    p.components().any(|c| c.as_os_str().to_str() == Some("Models"))
}

fn extract_eloquent(text: &str, out: &mut Vec<DeclFragment>) {
    let class_re = Regex::new(r"(?m)^(?:abstract\s+)?class\s+(\w+)\s+extends\s+\w+").unwrap();
    let table_re = Regex::new(r#"protected\s+\$table\s*=\s*['"](\w+)['"]"#).unwrap();
    let rel_re = Regex::new(r"(?:hasMany|belongsTo|hasOne|belongsToMany|morphMany)\(\s*(\w+)::class").unwrap();
    let table = table_re.captures(text).map(|m| m[1].to_string());
    let mut relations: Vec<String> = rel_re.captures_iter(text).map(|c| c[1].to_string()).collect();
    relations.sort();
    relations.dedup();
    for cap in class_re.captures_iter(text) {
        let name = cap[1].to_string();
        if name.ends_with("Controller") || name.ends_with("Test") || name.ends_with("Exception") {
            continue;
        }
        out.push(DeclFragment {
            name,
            kind: "eloquent".into(),
            fields: Vec::new(),
            relations: relations.clone(),
            // explicit $table read; the default is Laravel's pluralizer —
            // an inflection engine we refuse to imitate (rails/mongoose rule)
            table: table.clone(),
        });
    }
}

fn extract_jpa(text: &str, out: &mut Vec<DeclFragment>) {
    // @Entity [@Table(name="owners")] public class Owner — table explicit
    // when @Table names it; the default is a naming strategy, so None.
    let ent_re = Regex::new(
        r#"@Entity[\s\S]{0,300}?(?:@Table\s*\(\s*name\s*=\s*"(\w+)"[\s\S]{0,120}?)?(?:public\s+)?class\s+(\w+)"#,
    )
    .unwrap();
    let rel_re = Regex::new(r"@(?:ManyToOne|OneToMany|OneToOne|ManyToMany)[\s\S]{0,200}?(?:private|protected)\s+(?:\w+<)?(\w+)>?\s+\w+").unwrap();
    let mut relations: Vec<String> = rel_re.captures_iter(text).map(|c| c[1].to_string())
        .filter(|r| !matches!(r.as_str(), "Set" | "List" | "Collection")).collect();
    relations.sort();
    relations.dedup();
    for cap in ent_re.captures_iter(text) {
        out.push(DeclFragment {
            name: cap[2].to_string(),
            kind: "jpa".into(),
            fields: Vec::new(),
            relations: relations.clone(),
            table: cap.get(1).map(|m| m.as_str().to_string()),
        });
    }
}

fn extract_gorm(text: &str, out: &mut Vec<DeclFragment>) {
    // type ArticleModel struct { ... `gorm:"..."` ... } — a struct is a gorm
    // model when its body carries gorm tags. Table name is gorm's pluralizer:
    // None (the standing refusal to imitate inflection engines).
    let struct_re = Regex::new(r"(?ms)^type\s+(\w+)\s+struct\s*\{(.*?)^\}").unwrap();
    let field_re = Regex::new(r"(?m)^\s+(\w+)\s+\S+").unwrap();
    for cap in struct_re.captures_iter(text) {
        let (name, body) = (&cap[1], &cap[2]);
        if !body.contains("gorm:") {
            continue;
        }
        let fields: Vec<String> = field_re.captures_iter(body).map(|f| f[1].to_string()).take(30).collect();
        out.push(DeclFragment {
            name: name.to_string(),
            kind: "gorm".into(),
            fields,
            relations: Vec::new(),
            table: None,
        });
    }
}

fn extract_ecto(text: &str, out: &mut Vec<DeclFragment>) {
    // defmodule Plausible.Site do ... schema "sites" do — the concept is the
    // last module segment; the table is EXPLICIT in the schema macro.
    let mod_re = Regex::new(r"(?m)^\s*defmodule\s+([\w.]+)\s+do").unwrap();
    let schema_re = Regex::new(r#"(?m)^\s*schema\s+"(\w+)"\s+do"#).unwrap();
    let rel_re = Regex::new(r"(?m)^\s*(?:belongs_to|has_many|has_one)\s+:(\w+)").unwrap();
    let Some(sc) = schema_re.captures(text) else { return };
    let table = sc[1].to_string();
    let name = mod_re
        .captures(text)
        .map(|m| m[1].rsplit('.').next().unwrap_or(&m[1]).to_string())
        .unwrap_or_else(|| table.clone());
    let mut relations: Vec<String> = rel_re.captures_iter(text).map(|c| c[1].to_string()).collect();
    relations.sort();
    relations.dedup();
    out.push(DeclFragment {
        name,
        kind: "ecto".into(),
        fields: Vec::new(),
        relations,
        table: Some(table),
    });
}

fn extract_rust(text: &str, out: &mut Vec<DeclFragment>) {
    // Extract `pub struct Name` declarations from Rust source.
    // Only public structs — private structs are implementation detail, not
    // architectural concepts visible to callers or agents.
    // Fields: `pub field_name: Type` lines inside the struct body.
    // No table (Rust structs have no storage by declaration).
    //
    // The brace-counting approach is deliberate over a regex: nested generics
    // and where-clauses make balanced-brace matching far more reliable than
    // any regex that tries to capture the whole body.
    let struct_re = Regex::new(r"(?m)^pub struct\s+(\w+)").unwrap();
    let field_re = Regex::new(r"(?m)^\s+pub\s+(\w+)\s*:").unwrap();

    for cap in struct_re.captures_iter(text) {
        let name = cap[1].to_string();
        // Skip derives and marker structs with no body (unit structs end in `;`
        // or are tuple structs) — we only care about named-field structs.
        let after = &text[cap.get(0).unwrap().end()..];
        // Walk forward past whitespace and generic params to find `{` or `;`
        let trimmed = after.trim_start_matches(|c: char| c != '{' && c != ';');
        if !trimmed.starts_with('{') {
            continue; // unit struct or tuple struct — skip
        }

        // Collect fields from the struct body (up to the matching `}`).
        let mut fields = Vec::new();
        let mut depth = 0usize;
        let mut body_start = None;
        let body_chars: Vec<char> = trimmed.chars().collect();
        for (i, ch) in body_chars.iter().enumerate() {
            match ch {
                '{' => {
                    if depth == 0 {
                        body_start = Some(i + 1);
                    }
                    depth += 1;
                }
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        if let Some(start) = body_start {
                            let body: String = body_chars[start..i].iter().collect();
                            for fc in field_re.captures_iter(&body) {
                                let f = fc[1].to_string();
                                if f != "_" {
                                    fields.push(f);
                                }
                            }
                        }
                        break;
                    }
                }
                _ => {}
            }
        }

        out.push(DeclFragment {
            name,
            kind: "rust".into(),
            fields,
            relations: Vec::new(),
            table: None,
        });
    }
}
