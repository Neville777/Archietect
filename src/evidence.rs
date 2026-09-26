//! Versioned evidence envelopes for consumers such as QAForge.
//!
//! The inner payload remains tool-specific.  The envelope is deliberately
//! stable: consumers can persist and correlate evidence without knowing which
//! Archietect probe produced it.

use serde_json::{json, Value};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const CONTRACT_VERSION: &str = "qaforge.evidence.v1";

/// Wrap a probe result in the public QAForge ingestion contract.
pub fn qaforge_envelope(payload: Value, source: &str, target: Value, root: Option<&Path>) -> Value {
    let observed_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let evidence_type = payload
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("architectural_observation");
    let limitations = payload
        .get("note")
        .and_then(Value::as_str)
        .map(|note| vec![note.to_string()])
        .unwrap_or_default();
    let confidence = match evidence_type {
        "browser_page" => "medium",
        "http_response" => "high",
        _ => "low",
    };
    let commit = root.and_then(crate::scan::current_commit_sha);
    json!({
        "contract_version": CONTRACT_VERSION,
        "evidence_type": evidence_type,
        "source": source,
        "observed_at_ms": observed_at_ms,
        "commit": commit,
        "target": target,
        "confidence": confidence,
        "limitations": limitations,
        "payload": payload,
    })
}

/// Convert a JSON object into an envelope while preserving arbitrary evidence
/// payloads. Used by integrations that already have a probe result.
pub fn envelope_from_json(
    payload: Value,
    source: &str,
    target: Value,
    confidence: Option<&str>,
    root: Option<&Path>,
) -> Value {
    let mut envelope = qaforge_envelope(payload, source, target, root);
    if let Some(value) = confidence {
        envelope["confidence"] = Value::String(value.to_string());
    }
    envelope
}

/// A small schema check suitable for QAForge adapters before persistence.
pub fn validate_envelope(value: &Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| "evidence envelope must be a JSON object".to_string())?;
    for key in [
        "contract_version",
        "evidence_type",
        "source",
        "observed_at_ms",
        "target",
        "confidence",
        "limitations",
        "payload",
    ] {
        if !object.contains_key(key) {
            return Err(format!("evidence envelope missing required field: {key}"));
        }
    }
    if object["contract_version"] != CONTRACT_VERSION {
        return Err("unsupported evidence contract version".to_string());
    }
    if !object["observed_at_ms"].is_i64() && !object["observed_at_ms"].is_u64() {
        return Err("observed_at_ms must be an integer".to_string());
    }
    if !object["limitations"].is_array() {
        return Err("limitations must be an array".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_contains_qaforge_fields_and_validates() {
        let value = qaforge_envelope(
            json!({ "kind": "http_response", "status": 200, "note": "one GET" }),
            "archietect.runtime.verify_http",
            json!({ "url": "http://127.0.0.1:3000" }),
            None,
        );
        assert_eq!(value["contract_version"], CONTRACT_VERSION);
        assert_eq!(value["evidence_type"], "http_response");
        assert_eq!(value["source"], "archietect.runtime.verify_http");
        assert_eq!(value["target"]["url"], "http://127.0.0.1:3000");
        assert_eq!(value["confidence"], "high");
        validate_envelope(&value).unwrap();
    }

    #[test]
    fn validation_rejects_wrong_version() {
        let mut value = qaforge_envelope(json!({}), "test", json!({}), None);
        value["contract_version"] = json!("qaforge.evidence.v0");
        assert!(validate_envelope(&value).is_err());
    }
}
