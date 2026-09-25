use archietect::{proposal, query, scan};
use std::{
    path::Path,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn git(root: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn decision_proposal_tests_against_pending_new_django_model() {
    let root = std::env::temp_dir().join(format!(
        "archietect-proposal-pending-model-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    write(
        &root,
        "archietect.toml",
        "[policy]\ndecision_required_paths = [\"backend\"]\n",
    );
    write(&root, "backend/models.py", "from django.db import models\n");
    git(&root, &["add", "."]);
    git(
        &root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-qm",
            "baseline",
        ],
    );

    write(&root, "backend/models.py", "from django.db import models\n\nclass PendingInvoice(models.Model):\n    amount = models.IntegerField()\n");
    let patch = root.join("decision.patch");
    std::fs::write(&patch, "diff --git a/archietect.toml b/archietect.toml\n--- a/archietect.toml\n+++ b/archietect.toml\n@@ -2,0 +3,7 @@\n+\n+[[decision]]\n+id = \"pending-invoice-storage\"\n+decision = \"Pending invoices are persisted independently.\"\n+because = \"The model has its own audit lifecycle.\"\n+rejected = [\"Embedding pending invoices in another record\"]\n+links = [\"PendingInvoice\"]\n").unwrap();

    let submitted = proposal::submit(
        &root,
        proposal::Kind::Decision,
        "Govern pending invoice",
        "",
        None,
        None,
        "test",
        &patch,
    )
    .unwrap();
    let id = submitted["id"].as_u64().unwrap();
    let tested = proposal::test(&root, id).unwrap();
    assert_eq!(
        tested["status"], "passed",
        "the pending model must seed the proposal worktree: {tested:#}"
    );
    proposal::accept(&root, id).unwrap();

    let diff = String::from_utf8(git(
        &root,
        &[
            "diff",
            "HEAD",
            "--full-index",
            "--no-ext-diff",
            "--no-textconv",
        ],
    ))
    .unwrap();
    let (idx, graph) = scan::scan(&root);
    let receipt = query::ci(&idx, &graph, &diff, false);
    assert_eq!(
        receipt["pass"], true,
        "accepted decision must authorize the pending model: {receipt:#}"
    );
    let _ = std::fs::remove_dir_all(root);
}
