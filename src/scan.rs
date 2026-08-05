//! Extraction — declarations first, then observed usage. One walk, parallel.
//!
//! v0 extractors: Prisma, Django models, raw SQL (CREATE TABLE). These are
//! DECLARATION readers, not inference: schema.prisma and models.py are the
//! project asserting its own concepts, which is strictly better evidence than
//! anything derivable from application code. Everything v0 cannot parse
//! degrades HONESTLY to the NAMED tier rather than pretending.
//!
//! (The Python prototype died here: it recompiled per-concept regexes for
//! every file — O(concepts × files) compiles. All matchers are built ONCE
//! below, then files are scanned in parallel with cheap containment
//! prechecks before any regex runs.)

use crate::model::{Concept, Index};
use rayon::prelude::*;
use regex::{Regex, RegexBuilder};
use std::collections::BTreeMap;
use std::path::Path;
use walkdir::WalkDir;

const SKIP_DIRS: &[&str] = &[
    "node_modules", ".git", ".next", "target", "dist", "build", "__pycache__",
    ".venv", "venv", ".turbo", "coverage", ".cache", "vendor",
];
const SRC_EXTS: &[&str] = &["ts", "tsx", "js", "jsx", "py", "rs", "go", "java", "prisma", "sql"];
const MAX_FILE_BYTES: u64 = 2_000_000;

fn skip_dir(e: &walkdir::DirEntry) -> bool {
    e.file_type().is_dir()
        && e.file_name().to_str().map(|n| SKIP_DIRS.contains(&n)).unwrap_or(false)
}

pub fn scan(root: &Path) -> Index {
    let mut idx = Index {
        root: root.display().to_string(),
        ..Default::default()
    };

    // collect source files once
    let files: Vec<std::path::PathBuf> = WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !skip_dir(e))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| SRC_EXTS.contains(&x))
                .unwrap_or(false)
        })
        .filter(|e| e.metadata().map(|m| m.len() <= MAX_FILE_BYTES).unwrap_or(false))
        .map(|e| e.into_path())
        .collect();

    // ── pass 0: architect.toml — the project's OWN ontology and decisions ───
    // Aliases ("episode = stories") answer the question a name search cannot:
    // the concept exists under a different name. Decisions carry the WHY.
    // Both are DECLARED-tier: written by whoever owns the architecture,
    // reviewable in a diff — never inferred.
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
                    let arr = |k: &str| d.get(k).and_then(|x| x.as_array()).map(|a| {
                        a.iter().filter_map(|s| s.as_str().map(String::from)).collect()
                    }).unwrap_or_default();
                    idx.decisions.push(crate::model::Decision {
                        id: g("id"), decision: g("decision"), because: g("because"),
                        rejected: arr("rejected"), links: arr("links"),
                    });
                }
            }
            idx.declaration_files.push(("architect.toml".into(), "ontology".into()));
        }
    }

    // ── pass 1: declarations (sequential — order-stable) ────────────────────
    // Every file is read and checked for CREATE TABLE, not just .sql files:
    // real systems declare schema inside string literals (the source system
    // carries 110 CREATE TABLEs in Rust strings — a .sql-only extractor saw
    // 92 of ~200 concepts). Cheap containment precheck before the regex.
    for p in &files {
        let name = p.file_name().and_then(|f| f.to_str()).unwrap_or("");
        let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
        let Ok(text) = std::fs::read_to_string(p) else { continue };
        if ext == "prisma" {
            extract_prisma(&mut idx, root, p, &text);
        }
        if name == "models.py" && text.contains("models.Model") {
            extract_django(&mut idx, root, p, &text);
        }
        // pydantic models are concept declarations too (API contracts) —
        // found the hard way: a real service's models.py held only
        // pydantic.BaseModel classes and produced zero concepts.
        if ext == "py" && (text.contains("BaseModel") || text.contains("SQLModel")) {
            extract_pydantic(&mut idx, root, p, &text);
        }
        // SQLAlchemy: class X(db.Model) / __tablename__ — law from validation:
        // redash declares ~30 models this way and v0 saw 4.
        if ext == "py" && (text.contains("db.Model") || text.contains("__tablename__")) {
            extract_sqlalchemy(&mut idx, root, p, &text);
        }
        if text.contains("CREATE TABLE") || text.contains("create table") {
            extract_sql(&mut idx, root, p, &text);
        }
    }

    // ── merge: declarations that share a TABLE are one concept ──────────────
    // Law from validation: umami declares `website` in SQL migrations and
    // `Website` in Prisma mapped to the same table — one concept, counted
    // twice, reported as competing with itself. SQL-only declarations whose
    // name equals another concept's table mapping fold into that concept.
    let merges: Vec<(String, String)> = idx
        .concepts
        .iter()
        .filter(|(_, c)| c.declared_in.iter().all(|(_, k)| k == "sql"))
        .filter_map(|(name, _)| {
            idx.concepts
                .iter()
                .find(|(n2, c2)| {
                    *n2 != name
                        && c2
                            .table
                            .as_deref()
                            .map(|t| t.eq_ignore_ascii_case(name))
                            .unwrap_or(false)
                })
                .map(|(target, _)| (name.clone(), target.clone()))
        })
        .collect();
    for (dupe, target) in merges {
        if let Some(d) = idx.concepts.remove(&dupe) {
            let t = idx.concepts.get_mut(&target).unwrap();
            t.declared_in.extend(d.declared_in);
            if t.table.is_none() {
                t.table = d.table;
            }
        }
    }

    // ── pass 2: usage. Matchers built ONCE, files scanned in parallel. ──────
    struct Matcher {
        concept: String,
        /// cheap containment precheck before any regex
        needle_client: String, // "prisma-ish .name."
        client_re: Regex,
        needle_django: String, // "Name.objects."
        needle_construct: String, // "Name(" — construction is usage
        construct_re: Regex,
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
            let client_re = Regex::new(&format!(
                r"\b(?:prisma|db|tx|client)\.{}\.",
                regex::escape(&lname)
            ))
            .unwrap();
            let table = idx.concepts[name].table.clone();
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
                needle_table: table.map(|t| t.to_lowercase()),
                table_re,
            }
        })
        .collect();

    let usage: Vec<(String, String, String)> = files
        .par_iter()
        .filter(|p| p.extension().and_then(|x| x.to_str()) != Some("prisma"))
        .flat_map(|p| {
            let Ok(text) = std::fs::read_to_string(p) else {
                return Vec::new();
            };
            let lower = text.to_lowercase();
            let rel = p
                .strip_prefix(root)
                .unwrap_or(p)
                .display()
                .to_string();
            let mut hits = Vec::new();
            for m in &matchers {
                if text.contains(&m.needle_client) && m.client_re.is_match(&text) {
                    hits.push((m.concept.clone(), rel.clone(), "orm-client".to_string()));
                }
                if text.contains(&m.needle_django) {
                    hits.push((m.concept.clone(), rel.clone(), "django-orm".to_string()));
                }
                // construction only counts for real model names (len>=5 cuts
                // collision-prone short words); it is still labelled with its
                // own kind so a consumer can weigh it below orm access.
                if m.concept.len() >= 5
                    && text.contains(&m.needle_construct)
                    && m.construct_re.is_match(&text)
                {
                    hits.push((m.concept.clone(), rel.clone(), "constructed".to_string()));
                }
                if let (Some(needle), Some(re)) = (&m.needle_table, &m.table_re) {
                    if lower.contains(needle.as_str()) && re.is_match(&lower) {
                        hits.push((m.concept.clone(), rel.clone(), "raw-sql".to_string()));
                    }
                }
            }
            hits
        })
        .collect();

    idx.files_scanned = files.len();
    for (concept, file, kind) in usage {
        // a declaration file "using" its own concept is not usage
        if idx.concepts[&concept]
            .declared_in
            .iter()
            .any(|(f, _)| *f == file)
        {
            continue;
        }
        idx.concepts.get_mut(&concept).unwrap().usage.push((file, kind));
    }
    for c in idx.concepts.values_mut() {
        c.usage.sort();
        c.usage.dedup();
    }
    idx
}

fn concept_entry<'a>(concepts: &'a mut BTreeMap<String, Concept>, name: &str) -> &'a mut Concept {
    concepts.entry(name.to_string()).or_insert_with(|| Concept {
        name: name.to_string(),
        ..Default::default()
    })
}

const PRISMA_SCALARS: &[&str] = &[
    "String", "Int", "BigInt", "Float", "Decimal", "Boolean", "DateTime", "Json", "Bytes",
];

fn extract_prisma(idx: &mut Index, root: &Path, p: &Path, text: &str) {
    let rel = p.strip_prefix(root).unwrap_or(p).display().to_string();
    idx.declaration_files.push((rel.clone(), "prisma".into()));
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
        let table = map_re
            .captures(body)
            .map(|m| m[1].to_string())
            .unwrap_or_else(|| name.to_string());
        let c = concept_entry(&mut idx.concepts, name);
        c.declared_in.push((rel.clone(), "prisma".into()));
        c.fields = fields;
        c.relations = relations;
        c.table = Some(table);
    }
    let enum_re = Regex::new(r"(?m)^enum\s+(\w+)\s*\{").unwrap();
    for cap in enum_re.captures_iter(text) {
        let c = concept_entry(&mut idx.concepts, &cap[1]);
        c.declared_in.push((rel.clone(), "prisma-enum".into()));
    }
}

fn extract_django(idx: &mut Index, root: &Path, p: &Path, text: &str) {
    if !text.contains("models.Model") {
        return;
    }
    let rel = p.strip_prefix(root).unwrap_or(p).display().to_string();
    idx.declaration_files.push((rel.clone(), "django".into()));
    let class_re = Regex::new(r"(?m)^class\s+(\w+)\s*\([^)]*Model[^)]*\)\s*:").unwrap();
    let field_re = Regex::new(r"(?m)^    (\w+)\s*=\s*models\.").unwrap();
    let rel_re =
        Regex::new(r#"models\.(?:ForeignKey|OneToOneField|ManyToManyField)\(\s*['"]?(\w+)"#)
            .unwrap();
    let starts: Vec<(usize, String)> = class_re
        .captures_iter(text)
        .map(|c| (c.get(0).unwrap().end(), c[1].to_string()))
        .collect();
    let top_re = Regex::new(r"(?m)^\S").unwrap();
    for (start, name) in starts {
        let body_end = top_re
            .find_at(text, start)
            .map(|m| m.start())
            .unwrap_or(text.len());
        let body = &text[start..body_end];
        let fields: Vec<String> = field_re.captures_iter(body).map(|f| f[1].to_string()).collect();
        let mut relations: Vec<String> = rel_re
            .captures_iter(body)
            .map(|r| r[1].to_string())
            .filter(|r| r != "self")
            .collect();
        relations.sort();
        relations.dedup();
        let c = concept_entry(&mut idx.concepts, &name);
        c.declared_in.push((rel.clone(), "django".into()));
        c.fields = fields;
        c.relations = relations;
        // table stays None: Django's real table name needs the app label,
        // and we do not guess.
    }
}

fn extract_sql(idx: &mut Index, root: &Path, p: &Path, text: &str) {
    // For non-.sql sources, strip COMMENT lines before matching. Found the
    // hard way: the source system's guard carries a doc comment reading
    // "a patch proposing CREATE TABLE episodes is REJECTED" — the extractor
    // minted a phantom `episodes` concept from the guard's own documentation,
    // which then satisfied the guard's exact-name exemption and let the very
    // table it documents rejecting through. Prose about schema is not schema.
    let is_sql_file = p.extension().and_then(|x| x.to_str()) == Some("sql");
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
    // A real CREATE TABLE is followed by a column list `(` (or AS SELECT).
    // Law from validation: `logger.debug("CREATE TABLE query: %s", ...)` — a
    // LOG MESSAGE — minted a phantom `query` concept whose fake usage then
    // outranked the real Query model. Requiring the follower kills log/prose
    // strings structurally, with no blacklist to maintain.
    let re = RegexBuilder::new(
        r#"create\s+table\s+(?:if\s+not\s+exists\s+)?["'`]?(\w+)["'`]?\s*(?:\(|as\b)"#,
    )
    .case_insensitive(true)
    .build()
    .unwrap();
    let rel = p.strip_prefix(root).unwrap_or(p).display().to_string();
    let mut any = false;
    for cap in re.captures_iter(text) {
        any = true;
        let name = cap[1].to_string();
        let c = concept_entry(&mut idx.concepts, &name);
        c.declared_in.push((rel.clone(), "sql".into()));
        c.table = Some(name);
    }
    if any {
        idx.declaration_files.push((rel, "sql".into()));
    }
}

fn extract_pydantic(idx: &mut Index, root: &Path, p: &Path, text: &str) {
    let rel = p.strip_prefix(root).unwrap_or(p).display().to_string();
    // BaseModel (pydantic) and SQLModel both declare concepts. SQLModel with
    // table=True is STORAGE; its default table name is the lowercased class
    // name — a framework-documented rule, not a guess.
    // Bases are captured raw and filtered in the loop: `class Item(ItemBase,
    // table=True)` names neither BaseModel nor SQLModel, yet table=True IS the
    // storage declaration — law from validation, where the template's real
    // storage models were invisible while their API contracts were indexed.
    let class_re = Regex::new(r"(?m)^class\s+(\w+)\s*\(([^)]*)\)\s*:").unwrap();
    let field_re = Regex::new(r"(?m)^    (\w+)\s*:").unwrap();
    let top_re = Regex::new(r"(?m)^\S").unwrap();
    let mut any = false;
    let starts: Vec<(usize, String, String)> = class_re
        .captures_iter(text)
        .map(|c| (c.get(0).unwrap().end(), c[1].to_string(), c[2].to_string()))
        .collect();
    for (start, name, bases) in starts {
        let is_table = bases.contains("table=True") || bases.contains("table = True");
        if !is_table && !bases.contains("BaseModel") && !bases.contains("SQLModel") {
            continue;
        }
        any = true;
        let body_end = top_re
            .find_at(text, start)
            .map(|m| m.start())
            .unwrap_or(text.len());
        let body = &text[start..body_end];
        let fields: Vec<String> = field_re.captures_iter(body).map(|f| f[1].to_string()).collect();
        let kind = if is_table || bases.contains("SQLModel") { "sqlmodel" } else { "pydantic" };
        let c = concept_entry(&mut idx.concepts, &name);
        c.declared_in.push((rel.clone(), kind.into()));
        if c.fields.is_empty() {
            c.fields = fields;
        }
        if is_table && c.table.is_none() {
            c.table = Some(name.to_lowercase());
        }
        // otherwise table stays None: an API contract is not storage, and
        // inventing a table name for it would be a fabricated fact.
    }
    if any {
        idx.declaration_files.push((rel, "pydantic".into()));
    }
}

fn extract_sqlalchemy(idx: &mut Index, root: &Path, p: &Path, text: &str) {
    let rel = p.strip_prefix(root).unwrap_or(p).display().to_string();
    let class_re = Regex::new(r"(?m)^class\s+(\w+)\s*\(([^)]*)\)\s*:").unwrap();
    let tname_re = Regex::new(r#"__tablename__\s*=\s*["']([^"']+)["']"#).unwrap();
    let field_re = Regex::new(r"(?m)^    (\w+)\s*=\s*(?:db\.)?Column\(").unwrap();
    let top_re = Regex::new(r"(?m)^\S").unwrap();
    let mut any = false;
    let starts: Vec<(usize, String, String)> = class_re
        .captures_iter(text)
        .map(|c| (c.get(0).unwrap().end(), c[1].to_string(), c[2].to_string()))
        .collect();
    for (start, name, bases) in starts {
        let body_end = top_re
            .find_at(text, start)
            .map(|m| m.start())
            .unwrap_or(text.len());
        let body = &text[start..body_end];
        let tname = tname_re.captures(body).map(|m| m[1].to_string());
        // a class is a SQLAlchemy model if it states a table OR inherits db.Model;
        // bases named just "Base" are too ambiguous without __tablename__.
        if tname.is_none() && !bases.contains("db.Model") {
            continue;
        }
        any = true;
        let fields: Vec<String> = field_re.captures_iter(body).map(|f| f[1].to_string()).collect();
        let c = concept_entry(&mut idx.concepts, &name);
        c.declared_in.push((rel.clone(), "sqlalchemy".into()));
        if c.fields.is_empty() {
            c.fields = fields;
        }
        if c.table.is_none() {
            c.table = tname; // only what the class itself states — no guessed names
        }
    }
    if any {
        idx.declaration_files.push((rel, "sqlalchemy".into()));
    }
}
