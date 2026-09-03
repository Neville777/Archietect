//! `archietect seed` — a one-shot, human-invoked fix for the cold-start
//! problem on a fresh project: `tour`/`plan` return nothing useful when
//! archietect.toml has no `[[decision]]` entries yet, not because nothing
//! is known about the project, but because nobody has written it down in
//! the shape archietect reads. Most projects already HAVE this knowledge —
//! in their README — just not as a DECLARED decision.
//!
//! This scans README.md for constraint/decision-shaped bullet points under
//! a small, deliberately narrow set of headings and proposes them as
//! `[[decision]]` entries VERBATIM — the README's own sentence, never
//! rephrased, summarized, or interpreted. No LLM reads the README; this is
//! plain-text pattern matching, same discipline as every other extractor in
//! this codebase. `because` names the exact section it came from, so a
//! human reviewing archietect.toml later can trace every seeded entry back
//! to its source.
//!
//! CLI-only, same shape as `archietect history-archive` — a maintenance
//! action a human runs, never wired into REST or MCP (see those modules'
//! own "read-only" invariant) and never invoked automatically.
//!
//! Dry-run by default: prints candidates, touches nothing. `--write`
//! appends new (deduplicated) candidates to archietect.toml as a plain
//! TEXTUAL append — the file is never parsed-and-rewritten, so a human's
//! existing comments and formatting are untouched.

use crate::model::Index;
use std::path::Path;

/// Headings under which a bullet list is worth proposing as a decision.
/// Deliberately narrow: "Architecture" or "Overview" headings are usually
/// prose, not rules. A false positive here writes a bad `[[decision]]` into
/// a human's config file, which costs more than a missed real one.
const RULE_HEADINGS: &[&str] = &[
    "constraint",
    "convention",
    "rule",
    "guideline",
    "principle",
    "decision",
];

/// A README dumping hundreds of bullets under a matched heading is a sign
/// the heading matched something it shouldn't have, not a sign of hundreds
/// of real decisions — cut it off rather than flooding archietect.toml.
const MAX_CANDIDATES: usize = 30;

pub struct Candidate {
    pub id: String,
    pub decision: String,
    pub because: String,
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "section".to_string()
    } else {
        out
    }
}

/// Pure, deterministic extraction — no filesystem, no TOML — so it can be
/// tested directly against arbitrary README text.
pub fn extract_candidates(readme: &str) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut current_heading = "README".to_string();
    let mut in_rule_section = false;
    let mut idx_in_section = 0usize;

    for line in readme.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('#') {
            let heading_text = rest.trim_start_matches('#').trim().to_string();
            let lower = heading_text.to_lowercase();
            in_rule_section = RULE_HEADINGS.iter().any(|h| lower.contains(h));
            if !heading_text.is_empty() {
                current_heading = heading_text;
            }
            idx_in_section = 0;
            continue;
        }
        if !in_rule_section || out.len() >= MAX_CANDIDATES {
            continue;
        }
        let bullet = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "));
        if let Some(text) = bullet {
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            idx_in_section += 1;
            out.push(Candidate {
                id: format!("{}-{}", slugify(&current_heading), idx_in_section),
                decision: text.to_string(),
                because: format!(
                    "seeded verbatim from README.md, section '{current_heading}' — verify and edit before relying on this"
                ),
            });
        }
    }
    out
}

fn read_readme(root: &Path) -> Option<String> {
    for name in ["README.md", "readme.md", "Readme.md"] {
        if let Ok(t) = std::fs::read_to_string(root.join(name)) {
            return Some(t);
        }
    }
    None
}

fn escape_toml_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Orchestrates: read README, extract candidates, drop any whose exact text
/// OR id already exists among `idx.decisions` (a real prior scan — never
/// re-derived here), optionally append the rest to archietect.toml.
pub fn seed(root: &Path, idx: &Index, write: bool, proposed_by: Option<&str>) -> anyhow::Result<serde_json::Value> {
    let Some(readme) = read_readme(root) else {
        return Ok(serde_json::json!({
            "available": false,
            "reason": "no README.md (or readme.md) found at this project's root",
        }));
    };

    let existing_text: std::collections::HashSet<&str> = idx.decisions.iter().map(|d| d.decision.as_str()).collect();
    let existing_ids: std::collections::HashSet<&str> = idx.decisions.iter().map(|d| d.id.as_str()).collect();

    let new: Vec<Candidate> = extract_candidates(&readme)
        .into_iter()
        .filter(|c| !existing_text.contains(c.decision.as_str()) && !existing_ids.contains(c.id.as_str()))
        .collect();

    if new.is_empty() {
        return Ok(serde_json::json!({
            "available": true,
            "found": 0,
            "written": false,
            "note": "no new constraint/decision-shaped bullets found in README.md (or everything found is already declared)",
        }));
    }

    if write {
        let toml_path = root.join("archietect.toml");
        let mut block = String::new();
        if !toml_path.exists() {
            block.push_str("# Seeded by `archietect seed` from README.md — verify each entry.\n");
        } else {
            block.push('\n');
        }
        for c in &new {
            block.push_str(&format!(
                "[[decision]]\nid = \"{}\"\ndecision = \"{}\"\nbecause = \"{}\"\n",
                escape_toml_string(&c.id),
                escape_toml_string(&c.decision),
                escape_toml_string(&c.because)
            ));
            // Omitted entirely when not given — an absent field means "not
            // specified," never a written-but-empty "" that could later be
            // mistaken for a deliberate "no one" attribution.
            if let Some(who) = proposed_by {
                block.push_str(&format!("proposed_by = \"{}\"\n", escape_toml_string(who)));
            }
            block.push('\n');
        }
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&toml_path)?;
        f.write_all(block.as_bytes())?;
    }

    Ok(serde_json::json!({
        "available": true,
        "found": new.len(),
        "written": write,
        "proposed_by": proposed_by,
        "candidates": new.iter().map(|c| serde_json::json!({
            "id": c.id, "decision": c.decision, "because": c.because,
        })).collect::<Vec<_>>(),
        "note": if write {
            "appended to archietect.toml as new [[decision]] entries — re-run `archietect tour`/`register` to see them, and edit or delete any that don't hold up."
        } else {
            "dry run — nothing written. Re-run with --write to append these to archietect.toml."
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_bullets_only_under_rule_shaped_headings() {
        let readme = "\
# My Project

Some prose about what this does.

## Architecture

- This is prose about architecture, not a rule.

## Constraints

- Never call the payment API from a background job.
- All migrations must be reversible.

## Conventions
- PRs require one reviewer.
";
        let cands = extract_candidates(readme);
        let texts: Vec<&str> = cands.iter().map(|c| c.decision.as_str()).collect();
        assert!(texts.contains(&"Never call the payment API from a background job."));
        assert!(texts.contains(&"All migrations must be reversible."));
        assert!(texts.contains(&"PRs require one reviewer."));
        assert!(!texts.iter().any(|t| t.contains("prose about architecture")), "{texts:?}");
        assert_eq!(cands.len(), 3);
    }

    #[test]
    fn ids_are_stable_and_scoped_to_their_heading() {
        let readme = "## Constraints\n- First rule.\n- Second rule.\n";
        let cands = extract_candidates(readme);
        assert_eq!(cands[0].id, "constraints-1");
        assert_eq!(cands[1].id, "constraints-2");
    }

    #[test]
    fn because_names_the_real_source_section() {
        let readme = "## Rules\n- Do the thing.\n";
        let cands = extract_candidates(readme);
        assert!(cands[0].because.contains("README.md"));
        assert!(cands[0].because.contains("'Rules'"));
    }

    #[test]
    fn no_readme_is_unavailable_not_an_empty_result() {
        let root = std::env::temp_dir().join(format!("archietect-seed-test-noreadme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let idx = Index::default();
        let out = seed(&root, &idx, false, None).unwrap();
        assert_eq!(out["available"], serde_json::json!(false));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dry_run_finds_candidates_but_writes_nothing() {
        let root = std::env::temp_dir().join(format!("archietect-seed-test-dryrun-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("README.md"), "## Constraints\n- Never do X.\n").unwrap();
        let idx = Index::default();

        let out = seed(&root, &idx, false, None).unwrap();
        assert_eq!(out["found"], serde_json::json!(1));
        assert_eq!(out["written"], serde_json::json!(false));
        assert!(!root.join("archietect.toml").exists(), "dry run must not create archietect.toml");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn write_appends_and_second_run_is_idempotent() {
        let root = std::env::temp_dir().join(format!("archietect-seed-test-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("README.md"), "## Constraints\n- Never do X.\n- Always do Y.\n").unwrap();
        let idx = Index::default();

        let out = seed(&root, &idx, true, None).unwrap();
        assert_eq!(out["found"], serde_json::json!(2));
        assert_eq!(out["written"], serde_json::json!(true));

        let toml_text = std::fs::read_to_string(root.join("archietect.toml")).unwrap();
        assert!(toml_text.contains("Never do X."));
        assert!(toml_text.contains("Always do Y."));

        // Re-scan for real (through the actual TOML parser, not a hand-built
        // Index) and seed again — the two decisions just written must now
        // be recognized as already-declared and NOT duplicated.
        let (idx2, _graph) = crate::scan::scan(&root);
        assert_eq!(idx2.decisions.len(), 2, "the two seeded decisions must round-trip through the real TOML parser");
        let out2 = seed(&root, &idx2, true, None).unwrap();
        assert_eq!(out2["found"], serde_json::json!(0), "{out2}");

        let toml_text_after = std::fs::read_to_string(root.join("archietect.toml")).unwrap();
        assert_eq!(toml_text.matches("Never do X.").count(), toml_text_after.matches("Never do X.").count(), "must not duplicate on a second run");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn write_preserves_existing_file_content() {
        let root = std::env::temp_dir().join(format!("archietect-seed-test-preserve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("README.md"), "## Constraints\n- New rule here.\n").unwrap();
        std::fs::write(
            root.join("archietect.toml"),
            "# A human's own comment, must survive.\nexclude = [\"target\"]\n",
        )
        .unwrap();
        let idx = Index::default();

        seed(&root, &idx, true, None).unwrap();

        let toml_text = std::fs::read_to_string(root.join("archietect.toml")).unwrap();
        assert!(toml_text.contains("A human's own comment, must survive."), "{toml_text}");
        assert!(toml_text.contains("exclude = [\"target\"]"), "{toml_text}");
        assert!(toml_text.contains("New rule here."), "{toml_text}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn proposed_by_is_written_and_round_trips_through_the_real_toml_parser() {
        let root = std::env::temp_dir().join(format!("archietect-seed-test-proposedby-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("README.md"), "## Constraints\n- Attributed rule.\n").unwrap();
        let idx = Index::default();

        let out = seed(&root, &idx, true, Some("claude-sonnet-5")).unwrap();
        assert_eq!(out["proposed_by"], serde_json::json!("claude-sonnet-5"));

        let toml_text = std::fs::read_to_string(root.join("archietect.toml")).unwrap();
        assert!(toml_text.contains("proposed_by = \"claude-sonnet-5\""), "{toml_text}");

        let (idx2, _graph) = crate::scan::scan(&root);
        let d = idx2.decisions.iter().find(|d| d.decision == "Attributed rule.").expect("seeded decision must round-trip");
        assert_eq!(d.proposed_by, "claude-sonnet-5");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_proposed_by_omits_the_field_entirely_rather_than_writing_an_empty_one() {
        let root = std::env::temp_dir().join(format!("archietect-seed-test-noattrib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("README.md"), "## Constraints\n- Unattributed rule.\n").unwrap();
        let idx = Index::default();

        seed(&root, &idx, true, None).unwrap();

        let toml_text = std::fs::read_to_string(root.join("archietect.toml")).unwrap();
        assert!(!toml_text.contains("proposed_by"), "{toml_text}");

        let (idx2, _graph) = crate::scan::scan(&root);
        let d = idx2.decisions.iter().find(|d| d.decision == "Unattributed rule.").expect("seeded decision must round-trip");
        assert_eq!(d.proposed_by, "", "absent field must parse as empty, not a guessed value");

        let _ = std::fs::remove_dir_all(&root);
    }
}
