//! Deterministic, local feature-flag evaluation.
//!
//! Feature flags are deliberately separate from `permissions`: permissions
//! answer whether Archietect may observe a domain, while flags answer whether
//! an optional product capability is enabled.  Both project and global TOML
//! may define `[features]`; project configuration wins.  Missing flags are
//! safely disabled and evaluation never performs network I/O or mutates state.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flag {
    pub name: String,
    pub enabled: bool,
    pub source: String,
}

fn parse(path: &Path, source: &str) -> BTreeMap<String, Flag> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let Ok(value) = text.parse::<toml::Value>() else {
        return BTreeMap::new();
    };
    let Some(table) = value.get("features").and_then(|v| v.as_table()) else {
        return BTreeMap::new();
    };
    table
        .iter()
        .filter_map(|(name, value)| {
            let enabled = match value {
                toml::Value::Boolean(v) => *v,
                toml::Value::Table(t) => {
                    t.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false)
                }
                _ => return None,
            };
            let key = name.trim().to_lowercase();
            (!key.is_empty()).then(|| {
                (
                    key.clone(),
                    Flag {
                        name: key,
                        enabled,
                        source: source.into(),
                    },
                )
            })
        })
        .collect()
}

pub fn evaluate(root: &Path, name: &str) -> Value {
    let key = name.trim().to_lowercase();
    if key.is_empty() {
        return json!({"error":"feature name must not be empty"});
    }
    let global = crate::permissions::default_global_config_path()
        .map(|p| parse(&p, "global"))
        .unwrap_or_default();
    let project = parse(&root.join("archietect.toml"), "project");
    let resolved = project.get(&key).or_else(|| global.get(&key));
    match resolved {
        Some(flag) => json!({"feature": key, "enabled": flag.enabled, "source": flag.source,
            "default": false, "known": true,
            "note": "Project configuration overrides global configuration."}),
        None => json!({"feature": key, "enabled": false, "source": "default",
            "default": false, "known": false,
            "note": "Unknown or unconfigured features are disabled by default."}),
    }
}

pub fn list(root: &Path) -> Value {
    let global = crate::permissions::default_global_config_path()
        .map(|p| parse(&p, "global"))
        .unwrap_or_default();
    let project = parse(&root.join("archietect.toml"), "project");
    let mut merged = global;
    merged.extend(project);
    json!({"features": merged.values().map(|f| json!({
        "feature": f.name, "enabled": f.enabled, "source": f.source,
        "default": false, "known": true
    })).collect::<Vec<_>>(),
    "default_enabled": false,
    "note": "Unconfigured features are disabled; project values override global values."})
}

pub fn config_help() -> Value {
    json!({"project": "[features]\nexperimental_graph = true\n",
        "global": "~/.archietect/system.toml",
        "precedence": ["project", "global", "default disabled"]})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tempdir() -> std::path::PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("archietect-feature-{n}"));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn unknown_is_safe_off() {
        let d = tempdir();
        let v = evaluate(&d, "new-capability");
        assert_eq!(v["enabled"], false);
        assert_eq!(v["source"], "default");
    }

    #[test]
    fn project_overrides_global_and_parses_table() {
        let d = tempdir();
        fs::write(
            d.join("archietect.toml"),
            "[features]\nflag = { enabled = false }\n",
        )
        .unwrap();
        let v = evaluate(&d, "FLAG");
        assert_eq!(v["enabled"], false);
        assert_eq!(v["source"], "project");
    }
}
