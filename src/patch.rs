//! Reconstruct exactly the source described by a Git patch. Working-tree
//! contents are never evidence for a staged or remotely supplied patch.
use std::{io::Write, path::Path, process::{Command, Stdio}};

#[derive(Debug, Clone)]
pub struct FileChange {
    pub before_path: Option<String>,
    pub after_path: Option<String>,
    pub before: String,
    pub after: String,
}

fn path(value: &str, prefix: &str) -> Result<Option<String>, String> {
    if value == "/dev/null" {
        return Ok(None);
    }
    let value = value.strip_prefix(prefix).ok_or("unsupported patch path prefix")?;
    if value.is_empty()
        || value.chars().any(|c| c.is_whitespace() || c == '\\' || c == '"' || c == '\0')
        || value.split('/').any(|c| c.is_empty() || c == "." || c == "..")
    {
        return Err("unsafe or unsupported quoted patch path".into());
    }
    Ok(Some(value.to_string()))
}

fn blob(root: &Path, oid: &str) -> Result<String, String> {
    if !(4..=64).contains(&oid.len()) || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid Git blob identity".into());
    }
    let result = Command::new("git")
        .arg("--no-replace-objects")
        .arg("-C").arg(root)
        .args(["cat-file", "blob", oid])
        .output().map_err(|e| format!("cannot read patch base blob: {e}"))?;
    if !result.status.success() {
        return Err(format!("patch base blob {oid} is unavailable or ambiguous"));
    }
    let text = String::from_utf8(result.stdout).map_err(|_| "non-UTF-8 patch base")?;
    if text.contains('\0') {
        return Err("binary patch base is unsupported".into());
    }
    Ok(text)
}

/// Missing base identities, unsupported formats and mismatched hunks are
/// explicit errors. Callers must preserve them as unknown change evidence.
pub fn materialize(root: &Path, diff: &str) -> Result<Vec<FileChange>, String> {
    if diff.trim().is_empty() {
        return Ok(Vec::new());
    }
    let lines: Vec<&str> = diff.split_terminator('\n').collect();
    if !lines.first().is_some_and(|l| l.starts_with("diff --git ")) {
        return Err("expected Git unified diff with blob identities".into());
    }
    let starts: Vec<usize> = lines.iter().enumerate()
        .filter_map(|(i, l)| l.starts_with("diff --git ").then_some(i)).collect();
    let mut result = Vec::new();
    for (number, start) in starts.iter().enumerate() {
        let end = starts.get(number + 1).copied().unwrap_or(lines.len());
        let section = &lines[*start..end];
        let header = section[0].strip_prefix("diff --git ").unwrap();
        let (old, new) = header.split_once(" b/").ok_or("unsupported Git path header")?;
        let header_before = path(old, "a/")?;
        let header_after = path(new, "")?;
        let mut before_path = header_before.clone();
        let mut after_path = header_after.clone();
        let mut old_oid = None;
        let mut new_oid = None;
        let mut saw_before = false;
        let mut saw_after = false;
        let mut i = 1;
        while i < section.len() && !section[i].starts_with("@@ ") {
            let line = section[i];
            if let Some(ids) = line.strip_prefix("index ") {
                if old_oid.is_some() { return Err("duplicate blob identity header".into()); }
                let mut fields = ids.split_whitespace();
                let (a, b) = fields.next().and_then(|v| v.split_once(".."))
                    .ok_or("invalid blob identity header")?;
                if [a, b].iter().any(|v| !(4..=64).contains(&v.len()) || !v.bytes().all(|b| b.is_ascii_hexdigit())) {
                    return Err("invalid Git blob identity".into());
                }
                if fields.next().is_some_and(|mode| !matches!(mode, "100644" | "100755")) {
                    return Err("symlink or submodule patch is unsupported".into());
                }
                old_oid = Some(a);
                new_oid = Some(b);
            } else if let Some(value) = line.strip_prefix("--- ") {
                if saw_before { return Err("duplicate old path header".into()); }
                before_path = path(value, "a/")?;
                saw_before = true;
            } else if let Some(value) = line.strip_prefix("+++ ") {
                if saw_after { return Err("duplicate new path header".into()); }
                after_path = path(value, "b/")?;
                saw_after = true;
            } else if let Some(mode) = line.strip_prefix("new file mode ") {
                if !matches!(mode, "100644" | "100755") { return Err("unsupported new file mode".into()); }
                before_path = None;
            } else if let Some(mode) = line.strip_prefix("deleted file mode ") {
                if !matches!(mode, "100644" | "100755") { return Err("unsupported deleted file mode".into()); }
                after_path = None;
            } else if line.starts_with("old mode ") || line.starts_with("new mode ") {
                return Err("file mode change requires separate observation".into());
            } else if !(line.starts_with("similarity index ") || line.starts_with("rename from ") || line.starts_with("rename to ")) {
                return Err(format!("unsupported patch metadata: {line}"));
            }
            i += 1;
        }
        if before_path.is_some() && before_path != header_before || after_path.is_some() && after_path != header_after {
            return Err("patch path headers disagree".into());
        }
        if i < section.len() && (!saw_before || !saw_after) {
            return Err("hunks lack old/new path headers".into());
        }
        let before = if before_path.is_none() {
            if old_oid.is_some_and(|oid| !oid.bytes().all(|b| b == b'0')) {
                return Err("new file has nonempty base identity".into());
            }
            String::new()
        } else {
            blob(root, old_oid.ok_or("existing file patch lacks base blob identity")?)?
        };
        let base: Vec<&str> = before.split_inclusive('\n').collect();
        let mut cursor = 0;
        let mut output: Vec<String> = Vec::new();
        let hunk = regex::Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(?: .*)?$").unwrap();
        while i < section.len() {
            let cap = hunk.captures(section[i]).ok_or("malformed or unsupported hunk header")?;
            let number = |n: usize, default: usize| -> Result<usize, String> {
                cap.get(n).map(|m| m.as_str().parse::<usize>().map_err(|_| "invalid hunk range".into())).unwrap_or(Ok(default))
            };
            let old_start = number(1, 0)?;
            let old_count = number(2, 1)?;
            let new_start = number(3, 0)?;
            let new_count = number(4, 1)?;
            let old_offset = if old_count == 0 { old_start } else { old_start.checked_sub(1).ok_or("invalid old hunk position")? };
            let new_offset = if new_count == 0 { new_start } else { new_start.checked_sub(1).ok_or("invalid new hunk position")? };
            if old_offset < cursor || old_offset > base.len() { return Err("overlapping or out-of-range hunk".into()); }
            output.extend(base[cursor..old_offset].iter().map(|s| s.to_string()));
            cursor = old_offset;
            if output.len() != new_offset { return Err("new hunk position does not match reconstructed content".into()); }
            i += 1;
            let (mut consumed, mut produced) = (0, 0);
            while consumed < old_count || produced < new_count {
                let line = section.get(i).ok_or("truncated hunk")?;
                let kind = *line.as_bytes().first().ok_or("empty hunk line")?;
                if !matches!(kind, b' ' | b'+' | b'-') { return Err("unexpected hunk line".into()); }
                let payload = &line[1..];
                let no_newline = section.get(i + 1) == Some(&"\\ No newline at end of file");
                let text = if no_newline { payload.to_string() } else { format!("{payload}\n") };
                if kind != b'+' {
                    if consumed >= old_count || base.get(cursor).copied() != Some(text.as_str()) {
                        return Err("hunk context/removal does not match base blob".into());
                    }
                    cursor += 1;
                    consumed += 1;
                }
                if kind != b'-' {
                    if produced >= new_count { return Err("hunk new-line count mismatch".into()); }
                    output.push(text);
                    produced += 1;
                }
                i += if no_newline { 2 } else { 1 };
            }
        }
        output.extend(base[cursor..].iter().map(|s| s.to_string()));
        if output.iter().take(output.len().saturating_sub(1)).any(|s| !s.ends_with('\n')) {
            return Err("no-newline marker occurs before end of file".into());
        }
        let after = output.concat();
        if after.contains('\0') { return Err("binary content is unsupported".into()); }
        if after_path.is_none() && (!after.is_empty() || new_oid.is_some_and(|oid| !oid.bytes().all(|b| b == b'0'))) {
            return Err("deleted file patch leaves content or nonempty identity".into());
        }
        // An omitted entire final hunk still has valid local counts. The Git
        // postimage identity detects that truncation without consulting files.
        if before_path.is_some() && after_path.is_some() {
            let expected = new_oid.ok_or("modified file lacks new blob identity")?;
            let mut child = Command::new("git").arg("-C").arg(root)
                .args(["hash-object", "--stdin"])
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
                .spawn().map_err(|e| format!("cannot verify patch postimage: {e}"))?;
            child.stdin.take().ok_or("missing hash input")?.write_all(after.as_bytes())
                .map_err(|e| format!("cannot hash patch postimage: {e}"))?;
            let out = child.wait_with_output().map_err(|e| format!("cannot verify patch postimage: {e}"))?;
            if !out.status.success() || !String::from_utf8_lossy(&out.stdout).trim().starts_with(expected) {
                return Err("reconstructed content does not match patch postimage identity".into());
            }
        }
        result.push(FileChange { before_path, after_path, before, after });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Repo(std::path::PathBuf);
    impl Repo {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("archietect-patch-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir_all(&root).unwrap();
            let repo = Self(root);
            repo.git(&["init", "-q"]);
            repo
        }
        fn git(&self, args: &[&str]) -> String {
            let out = Command::new("git").arg("-C").arg(&self.0).args(args).output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8(out.stdout).unwrap()
        }
    }
    impl Drop for Repo { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

    #[test]
    fn staged_snapshot_ignores_unstaged_content() {
        let repo = Repo::new();
        let file = repo.0.join("model.py");
        std::fs::write(&file, "class Before:\n    pass\n").unwrap();
        repo.git(&["add", "model.py"]);
        repo.git(&["-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "core.hooksPath=/dev/null", "commit", "-qm", "base"]);
        std::fs::write(&file, "class After:\n    pass\n").unwrap();
        repo.git(&["add", "model.py"]);
        let diff = repo.git(&["diff", "--cached", "--full-index"]);
        std::fs::write(&file, "unrelated unstaged content").unwrap();
        let files = materialize(&repo.0, &diff).unwrap();
        assert_eq!(files[0].before, "class Before:\n    pass\n");
        assert_eq!(files[0].after, "class After:\n    pass\n");
        assert!(materialize(&repo.0, &diff.replace("-class Before:", "-class Wrong:")).is_err());
    }

    #[test]
    fn new_file_and_missing_final_newline_need_no_repository() {
        let diff = "diff --git a/a.sql b/a.sql\nnew file mode 100644\nindex 0000000..1234567\n--- /dev/null\n+++ b/a.sql\n@@ -0,0 +1 @@\n+CREATE TABLE example(id int);\n\\ No newline at end of file\n";
        let files = materialize(Path::new("/missing"), diff).unwrap();
        assert_eq!(files[0].before_path, None);
        assert_eq!(files[0].after, "CREATE TABLE example(id int);");
        assert!(materialize(Path::new("/missing"), &diff.replace("+1 @@", "+1,2 @@")).is_err());
        assert!(materialize(Path::new("/missing"), &diff.replace("a.sql", "../escape")).is_err());
    }

    #[test]
    fn removal_and_newline_transition_use_blob_evidence() {
        let repo = Repo::new();
        let file = repo.0.join("file.rs");
        std::fs::write(&file, "fn old() {}").unwrap();
        repo.git(&["add", "file.rs"]);
        std::fs::write(&file, "fn new() {}\n").unwrap();
        let diff = repo.git(&["diff"]);
        let files = materialize(&repo.0, &diff).unwrap();
        assert_eq!(files[0].before, "fn old() {}");
        assert_eq!(files[0].after, "fn new() {}\n");
        std::fs::remove_file(&file).unwrap();
        let files = materialize(&repo.0, &repo.git(&["diff"])).unwrap();
        assert_eq!(files[0].after_path, None);
        assert!(files[0].after.is_empty());
    }

    #[test]
    fn staged_new_file_is_independent_of_worktree() {
        let repo = Repo::new();
        let file = repo.0.join("new.py");
        std::fs::write(&file, "class Account:\n    pass\n").unwrap();
        repo.git(&["add", "new.py"]);
        let diff = repo.git(&["diff", "--cached", "--full-index"]);
        std::fs::write(file, "unstaged").unwrap();
        let files = materialize(&repo.0, &diff).unwrap();
        assert_eq!(files[0].after, "class Account:\n    pass\n");
    }

    #[test]
    fn omitted_complete_hunk_is_rejected_by_postimage_identity() {
        let repo = Repo::new();
        let file = repo.0.join("file.py");
        let before = (0..20).map(|i| format!("value_{i} = {i}\n")).collect::<String>();
        std::fs::write(&file, &before).unwrap();
        repo.git(&["add", "file.py"]);
        std::fs::write(&file, before.replace("value_0 = 0", "value_0 = 5").replace("value_19 = 19", "value_19 = 25")).unwrap();
        let diff = repo.git(&["diff", "--full-index"]);
        assert!(materialize(&repo.0, &diff).is_ok());
        let second = diff.match_indices("\n@@").nth(1).unwrap().0;
        assert!(materialize(&repo.0, &diff[..second + 1]).is_err());
    }
}
