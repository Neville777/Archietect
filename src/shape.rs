//! Output shaping — let a caller ask for the slice it needs instead of the
//! whole bag.
//!
//! Found by measurement, not by taste. A controlled A/B on a real repository
//! (fresh agent answering three questions with grep vs. with archietect)
//! showed archietect answering more completely — all three importers of a
//! class where grep found one — but costing ~35% MORE tokens. The cause was
//! not the memory, it was the output shape: to learn the current git branch
//! the caller received the entire `status` payload — structural coverage,
//! docker section, every explanatory `note` — for three fields it needed.
//! Every new domain wired into `status` makes that worse for every question
//! that doesn't involve it.
//!
//! Two independent, composable knobs, applied at the single output point of
//! each transport (CLI print, REST body, MCP tool result) so that NO existing
//! output changes unless a caller explicitly asks:
//!
//!   `only`     — keep just these top-level keys of an object result.
//!   `compact`  — recursively drop the explanatory prose fields (`note`,
//!                `evidence_note`), which exist for humans reading the JSON
//!                cold and are pure overhead for a caller that already knows
//!                the vocabulary. Nothing evidentiary is removed: tiers,
//!                `what` strings, files, lines, and verdicts all stay.
//!
//! Deliberately NOT a general JSON-path language. Top-level keys and one
//! well-known prose class cover the measured problem; anything richer is
//! speculation until a second measurement says otherwise.

use serde_json::Value;

/// Prose-only keys `compact` removes. These carry explanation, never
/// evidence — a caller that strips them loses nothing it could act on.
const PROSE_KEYS: &[&str] = &["note", "evidence_note"];

/// Apply `only` (top-level key selection) and `compact` (prose removal).
/// `only` is applied first, then `compact`, so a selected key's subtree is
/// still compacted. A non-object value is returned unchanged by `only`
/// (there are no top-level keys to select); `compact` still descends into
/// arrays.
pub fn apply(mut v: Value, only: Option<&[String]>, compact: bool) -> Value {
    if let Some(keys) = only {
        if !keys.is_empty() {
            if let Value::Object(map) = v {
                let mut kept = serde_json::Map::new();
                for k in keys {
                    if let Some(val) = map.get(k) {
                        kept.insert(k.clone(), val.clone());
                    }
                }
                v = Value::Object(kept);
            }
        }
    }
    if compact {
        strip_prose(&mut v);
    }
    v
}

/// Parse a comma-separated `only` list as it arrives from a CLI flag or a
/// query string. Empty/whitespace-only input means "no selection".
pub fn parse_only(s: Option<&str>) -> Option<Vec<String>> {
    let s = s?.trim();
    if s.is_empty() {
        return None;
    }
    let keys: Vec<String> = s
        .split(',')
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .collect();
    if keys.is_empty() { None } else { Some(keys) }
}

fn strip_prose(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for k in PROSE_KEYS {
                map.remove(*k);
            }
            for child in map.values_mut() {
                strip_prose(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                strip_prose(item);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_keeps_requested_top_level_keys_in_requested_order() {
        let v = json!({ "a": 1, "b": 2, "c": 3 });
        let out = apply(v, Some(&["c".to_string(), "a".to_string()]), false);
        assert_eq!(out, json!({ "c": 3, "a": 1 }));
    }

    #[test]
    fn only_ignores_missing_keys_rather_than_erroring() {
        let v = json!({ "a": 1 });
        let out = apply(v, Some(&["a".to_string(), "zzz".to_string()]), false);
        assert_eq!(out, json!({ "a": 1 }));
    }

    #[test]
    fn empty_only_is_a_no_op() {
        let v = json!({ "a": 1, "note": "x" });
        assert_eq!(apply(v.clone(), Some(&[]), false), v);
        assert_eq!(apply(v.clone(), None, false), v);
    }

    #[test]
    fn compact_strips_prose_recursively_but_keeps_evidence() {
        let v = json!({
            "note": "top-level prose",
            "verdict": "STRUCTURAL",
            "evidence": [ { "tier": "Declared", "what": "Class declared in a.ts:1", "note": "inner prose" } ],
            "git": { "enabled": true, "note": "nested prose", "resources": [] },
            "evidence_note": "more prose",
        });
        let out = apply(v, None, true);
        assert_eq!(out, json!({
            "verdict": "STRUCTURAL",
            "evidence": [ { "tier": "Declared", "what": "Class declared in a.ts:1" } ],
            "git": { "enabled": true, "resources": [] },
        }));
    }

    #[test]
    fn only_then_compact_compose() {
        let v = json!({ "git": { "enabled": true, "note": "p" }, "docker": { "note": "q" }, "note": "r" });
        let out = apply(v, Some(&["git".to_string()]), true);
        assert_eq!(out, json!({ "git": { "enabled": true } }));
    }

    #[test]
    fn parse_only_handles_whitespace_and_empties() {
        assert_eq!(parse_only(None), None);
        assert_eq!(parse_only(Some("")), None);
        assert_eq!(parse_only(Some(" , ,")), None);
        assert_eq!(parse_only(Some(" git , docker ")), Some(vec!["git".to_string(), "docker".to_string()]));
    }
}
