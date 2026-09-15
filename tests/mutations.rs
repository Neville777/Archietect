//! Exercise the public CI transport against real staged Git patches.
use std::{io::Write, path::PathBuf, process::{Command, Stdio}, sync::atomic::{AtomicU64, Ordering}};
use serde_json::Value;

static NEXT: AtomicU64 = AtomicU64::new(0);
const POLICY: &str = "[policy]\ndecision_required_paths = [\"src\", \"backend\", \"apps\"]\n";
const DECISION: &str = "\n[[decision]]\nid = \"contribution-storage\"\ndecision = \"Persist contributions independently\"\nbecause = \"Contributions have an independent audit lifecycle\"\nrejected = [\"Embed contributions on members\"]\nlinks = [\n  \"Contribution\",\n]\n";

struct Repo(PathBuf);
impl Repo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("archietect-mutation-cli-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&root).unwrap();
        let repo = Self(root);
        repo.git(&["init", "-q"]);
        repo.write("archietect.toml", POLICY);
        repo.write("src/lib.rs", "fn helper() -> u32 { 1 }\n");
        repo.write("backend/models.py", "from django.db import models\n");
        repo.commit_base();
        repo
    }
    fn write(&self, path: &str, content: &str) {
        let path = self.0.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git").arg("-C").arg(&self.0).args(args).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    }
    fn commit_base(&self) {
        self.git(&["add", "."]);
        self.git(&["-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "core.hooksPath=/dev/null", "commit", "-qm", "baseline"]);
    }
    fn stage(&self, path: &str) { self.git(&["add", path]); }
    fn ci(&self) -> (i32, Value) {
        let diff = self.git(&["diff", "--cached", "--full-index", "--no-ext-diff", "--no-textconv"]);
        let mut child = Command::new(env!("CARGO_BIN_EXE_archietect"))
            .arg("--root").arg(&self.0).arg("ci")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(diff.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        let json = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("invalid CLI output: {e}: {} stderr: {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)));
        (output.status.code().unwrap(), json)
    }
    fn contribution(&self) {
        self.write("backend/models.py", "from django.db import models\n\nclass Contribution(models.Model):\n    amount = models.IntegerField()\n");
        self.stage("backend/models.py");
    }
}
impl Drop for Repo { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

#[test]
fn routine_function_body_and_comment_pass() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "// Fix a routine implementation detail.\nfn helper() -> u32 { 2 }\n");
    repo.stage("src/lib.rs");
    let (code, out) = repo.ci();
    assert_eq!(code, 0, "{out:#}");
    assert_eq!(out["pass"], true);
}

#[test]
fn new_django_model_requires_applicable_multiline_decision() {
    let repo = Repo::new();
    repo.contribution();
    let (code, out) = repo.ci();
    assert_eq!(code, 1, "{out:#}");
    assert_eq!(out["pass"], false);
    repo.write("archietect.toml", &format!("{POLICY}{DECISION}"));
    repo.stage("archietect.toml");
    let (code, out) = repo.ci();
    assert_eq!(code, 0, "{out:#}");
}

#[test]
fn unstaged_decision_cannot_authorize_staged_model() {
    let repo = Repo::new();
    repo.contribution();
    repo.write("archietect.toml", &format!("{POLICY}{DECISION}"));
    let (code, out) = repo.ci();
    assert_eq!(code, 1, "{out:#}");
    assert_eq!(out["pass"], false);
}

#[test]
fn removal_of_persistent_concept_requires_decision() {
    let repo = Repo::new();
    repo.contribution();
    repo.commit_base();
    repo.write("backend/models.py", "from django.db import models\n");
    repo.stage("backend/models.py");
    let (code, out) = repo.ci();
    assert_eq!(code, 1, "{out:#}");
}

#[test]
fn malformed_source_cannot_be_classified_safe() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "fn helper( {\n");
    repo.stage("src/lib.rs");
    let (code, out) = repo.ci();
    assert_eq!(code, 1, "{out:#}");
    assert!(out.to_string().contains("unknown_structural_mutation"), "{out:#}");
}

#[test]
fn duplicate_symbol_identity_is_unknown() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "fn helper() -> u32 { 1 }\nfn helper() -> u32 { 2 }\n");
    repo.stage("src/lib.rs");
    let (code, out) = repo.ci();
    assert_eq!(code, 1, "{out:#}");
    assert!(out.to_string().contains("unknown_structural_mutation"), "{out:#}");
}
