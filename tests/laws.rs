//! The law regression suite — the enforcement arm of src/laws.rs.
//!
//! One test per law, each with a SELF-CONTAINED fixture built in a temp dir:
//! the validation corpus (37 real repos) taught the laws, but it cannot BE the
//! regression suite — it is gitignored, mutable, and hundreds of megabytes.
//! These fixtures are the distilled, minimal reproduction of each original
//! wrong answer, checked in forever. If a refactor re-introduces a bug the
//! corpus already paid for, the law's own test fails by name.

use architect::{query, scan};
use std::path::PathBuf;

/// Build a throwaway fixture repo from (relative path, contents) pairs.
struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "architect-law-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, content) in files {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }
        Fixture { root }
    }
    fn index(&self) -> architect::model::Index {
        scan::scan_with_prior(&self.root, None)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn law_001_word_boundary() {
    // 'story' must not claim harvest_history as a competing implementation.
    let f = Fixture::new(
        "001",
        &[
            ("db/a.sql", "CREATE TABLE story (id INT);"),
            ("db/b.sql", "CREATE TABLE harvest_history (id INT);"),
        ],
    );
    let idx = f.index();
    let r = query::concept(&idx, "story");
    assert_eq!(r["canonical"], "story");
    let competing: Vec<String> = r["competing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(
        !competing.iter().any(|c| c.contains("history")),
        "substring matching resurfaced: {competing:?}"
    );
}

#[test]
fn law_002_exact_exemption() {
    // ghosts must BLOCK beside Ghost; re-declaring Ghost itself must not.
    let f = Fixture::new(
        "002",
        &[(
            "prisma/schema.prisma",
            "model Ghost {\n  id String @id\n}\n",
        ),
        ("src/use.ts", "const g = await prisma.ghost.findMany();\n")],
    );
    let idx = f.index();
    let blocked = query::guard(&idx, "CREATE TABLE ghosts (id SERIAL);");
    assert_eq!(blocked["allowed"], false, "near-name must block");
    let allowed = query::guard(&idx, "CREATE TABLE \"Ghost\" (id TEXT);");
    assert_eq!(allowed["allowed"], true, "exact re-declaration is exempt");
}

#[test]
fn law_003_comment_prose() {
    // A doc comment mentioning CREATE TABLE must not mint a concept.
    let f = Fixture::new(
        "003",
        &[(
            "src/guard.rs",
            "//! a patch proposing CREATE TABLE episodes (id) is REJECTED here\n\
             // also: CREATE TABLE phantoms (id INT);\n\
             fn real() {}\n",
        )],
    );
    let idx = f.index();
    assert!(
        !idx.concepts.contains_key("episodes") && !idx.concepts.contains_key("phantoms"),
        "prose in comments minted a concept: {:?}",
        idx.concepts.keys().collect::<Vec<_>>()
    );
}

#[test]
fn law_004_exact_over_token() {
    // Website must beat WebsiteEvent for the query 'website', even when
    // WebsiteEvent has more observed usage.
    let f = Fixture::new(
        "004",
        &[
            (
                "prisma/schema.prisma",
                "model Website {\n  id String @id\n}\nmodel WebsiteEvent {\n  id String @id\n}\n",
            ),
            ("src/a.ts", "await prisma.websiteEvent.findMany();"),
            ("src/b.ts", "await prisma.websiteEvent.create({});"),
            ("src/c.ts", "await prisma.websiteEvent.count();"),
        ],
    );
    let idx = f.index();
    let r = query::concept(&idx, "website");
    assert_eq!(
        r["canonical"], "Website",
        "usage-heavy token match outranked the exact name"
    );
}

#[test]
fn law_005_same_table_merge() {
    // Prisma model mapped to table `website` + migration declaring the same
    // table = ONE concept, not a concept competing with itself.
    let f = Fixture::new(
        "005",
        &[
            (
                "prisma/schema.prisma",
                "model Website {\n  id String @id\n  @@map(\"website\")\n}\n",
            ),
            ("prisma/migrations/001/migration.sql", "CREATE TABLE website (id TEXT);"),
        ],
    );
    let idx = f.index();
    assert!(
        idx.concepts.contains_key("Website") && !idx.concepts.contains_key("website"),
        "same-table declarations failed to merge: {:?}",
        idx.concepts.keys().collect::<Vec<_>>()
    );
    // and the merged concept carries BOTH declarations as evidence
    let c = &idx.concepts["Website"];
    assert!(c.declared_in.iter().any(|(_, k)| k == "prisma"));
    assert!(c.declared_in.iter().any(|(_, k)| k == "sql"));
}

#[test]
fn law_006_table_true() {
    // class Item(ItemBase, table=True) is storage even though no base is
    // literally SQLModel/BaseModel.
    let f = Fixture::new(
        "006",
        &[(
            "app/models.py",
            "from sqlmodel import SQLModel\n\n\
             class ItemBase(SQLModel):\n    title: str\n\n\
             class Item(ItemBase, table=True):\n    id: int\n",
        )],
    );
    let idx = f.index();
    let r = query::concept(&idx, "item");
    assert_eq!(r["canonical"], "Item");
    assert_eq!(r["table"], "item", "SQLModel default table name rule");
}

#[test]
fn law_007_orm_over_sql() {
    // A legit SQL table named `query` must not outrank the SQLAlchemy Query
    // model on an exact tie.
    let f = Fixture::new(
        "007",
        &[
            (
                "app/models.py",
                "class Query(db.Model):\n    __tablename__ = \"queries\"\n    id = db.Column(Integer)\n",
            ),
            ("db/legacy.sql", "CREATE TABLE query (id INT);"),
            ("app/service.py", "rows = Query.objects.all()\n"),
        ],
    );
    let idx = f.index();
    let r = query::concept(&idx, "query");
    assert_eq!(
        r["canonical"], "Query",
        "sql-string concept outranked the ORM declaration"
    );
}

#[test]
fn law_008_follower_required() {
    // logger.debug("CREATE TABLE query: %s") must not mint a concept;
    // a real CREATE TABLE with its column list must.
    let f = Fixture::new(
        "008",
        &[
            (
                "app/runner.py",
                "logger.debug(\"CREATE TABLE query: %s\", create_table)\n",
            ),
            ("db/real.sql", "CREATE TABLE results (id INT);"),
        ],
    );
    let idx = f.index();
    assert!(
        !idx.concepts.contains_key("query"),
        "log-string prose minted a concept"
    );
    assert!(idx.concepts.contains_key("results"), "real DDL must still extract");
}

#[test]
fn law_009_alias_resolution() {
    // The declared ontology resolves what no name search can see, and the
    // guard blocks through it citing the governing decision.
    let f = Fixture::new(
        "009",
        &[
            (
                "architect.toml",
                "[aliases]\nepisode = \"stories\"\n\n\
                 [[decision]]\nid = \"stories-own-episodes\"\n\
                 decision = \"Episodes are stored as stories\"\n\
                 because = \"they always shared identity\"\n\
                 rejected = [\"separate episodes table\"]\n\
                 links = [\"episode\", \"stories\"]\n",
            ),
            ("db/schema.sql", "CREATE TABLE stories (id BIGSERIAL);"),
            ("src/use.rs", "let x = sqlx::query(\"SELECT * FROM stories\");\n"),
        ],
    );
    let idx = f.index();
    let r = query::concept(&idx, "episode");
    assert_eq!(r["canonical"], "stories", "alias resolution failed");
    assert_eq!(r["resolved_via"], "alias");
    let g = query::guard(&idx, "CREATE TABLE episodes (id BIGSERIAL);");
    assert_eq!(g["allowed"], false, "guard must block through the ontology");
    assert!(
        g["reason"].as_str().unwrap().contains("stories-own-episodes"),
        "rejection must cite the governing decision, got: {}",
        g["reason"]
    );
}
