//! Allowlisted field redaction for logs, spans, and audit metadata.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

/// Substrings that mark a field key as sensitive (case-insensitive).
const SENSITIVE_KEY_FRAGMENTS: &[&str] = &[
    "authorization",
    "cookie",
    "database_url",
    "document_content",
    "email",
    "object_key",
    "password",
    "pii",
    "prompt",
    "question",
    "answer",
    "refresh",
    "secret",
    "signed_url",
    "api_key",
    "apikey",
    "access_token",
    "capability",
    "token",
];

/// Value fragments that must never appear in emitted diagnostics.
pub const CANARY_FRAGMENTS: &[&str] = &[
    "CANARY_SECRET_TOKEN",
    "CANARY_DOCUMENT_TEXT",
    "CANARY_PROMPT_TEXT",
    "CANARY_ANSWER_TEXT",
    "CANARY_API_KEY",
    "canary@example.com",
    "mh1.",
    "Bearer ",
];

/// Stable keys permitted on structured log/span fields (beyond correlation).
pub const LOG_FIELD_ALLOWLIST: &[&str] = &[
    "action",
    "actor_id",
    "attempt",
    "code",
    "duration_ms",
    "endpoint_class",
    "error_class",
    "format",
    "index_signature",
    "job_id",
    "job_type",
    "leg",
    "method",
    "operation",
    "org_id",
    "outcome",
    "queue",
    "request_id",
    "resource_kind",
    "resource_type",
    "result",
    "route",
    "status_class",
    "target_type",
    "trace_id",
];

/// Metadata keys permitted on durable audit rows.
pub const AUDIT_METADATA_ALLOWLIST: &[&str] = &[
    "attempt",
    "cancelled_writer_jobs",
    "deleted_chunks",
    "document_id",
    "drift_kind",
    "error_class",
    "family_id",
    "format",
    "job_id",
    "job_type",
    "object_count",
    "orphan_objects",
    "orphan_vectors",
    "permission",
    "phase",
    "reason",
    "rebuilt_vector_jobs",
    "refresh_id",
    "replaced_id",
    "resource_kind",
    "result",
    "stale_vectors",
    "token_id",
    "version_id",
];

const REDACTED: &str = "[REDACTED]";

pub fn is_sensitive_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    // Exact forbidden carriers (avoid false positives on opaque ids like token_id).
    const EXACT: &[&str] = &[
        "authorization",
        "cookie",
        "database_url",
        "document_content",
        "email",
        "object_key",
        "object_keys",
        "password",
        "pii",
        "prompt",
        "question",
        "answer",
        "refresh_token",
        "secret",
        "signed_url",
        "api_key",
        "apikey",
        "access_token",
        "capability",
        "token",
        "raw_body",
        "body",
        "text",
        "markdown",
    ];
    if EXACT.contains(&normalized.as_str()) {
        return true;
    }
    SENSITIVE_KEY_FRAGMENTS
        .iter()
        .any(|fragment| normalized.contains(fragment) && !normalized.ends_with("_id"))
}

pub fn contains_canary(value: &str) -> bool {
    CANARY_FRAGMENTS
        .iter()
        .any(|fragment| value.contains(fragment))
}

/// Redact map values whose keys look sensitive; drop unknown keys when `allowlist` is set.
pub fn redacted_fields(fields: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    fields
        .iter()
        .filter_map(|(key, value)| {
            if !LOG_FIELD_ALLOWLIST.iter().any(|allowed| *allowed == key) && is_sensitive_key(key) {
                return Some((key.clone(), REDACTED.to_string()));
            }
            if is_sensitive_key(key) || contains_canary(value) {
                return Some((key.clone(), REDACTED.to_string()));
            }
            if LOG_FIELD_ALLOWLIST.iter().any(|allowed| *allowed == key) {
                Some((key.clone(), value.clone()))
            } else if is_sensitive_key(key) {
                Some((key.clone(), REDACTED.to_string()))
            } else {
                // Unknown non-sensitive keys are omitted from structured logs.
                None
            }
        })
        .collect()
}

/// Filter audit metadata to the allowlist and refuse secret/canary material.
pub fn sanitize_audit_metadata(metadata: &Value) -> Result<Value, String> {
    let Value::Object(map) = metadata else {
        return Err("audit_metadata_must_be_object".into());
    };
    let mut out = Map::new();
    for (key, value) in map {
        if !AUDIT_METADATA_ALLOWLIST
            .iter()
            .any(|allowed| *allowed == key)
        {
            return Err(format!("audit_metadata_key_not_allowlisted:{key}"));
        }
        if is_sensitive_key(key) {
            return Err(format!("audit_metadata_sensitive_key:{key}"));
        }
        match value {
            Value::Null | Value::Bool(_) | Value::Number(_) => {
                out.insert(key.clone(), value.clone());
            }
            Value::String(text) => {
                if contains_canary(text)
                    || text.contains("mh1.")
                    || text.contains("Bearer ")
                    || text.starts_with("eyJ")
                {
                    return Err("audit_metadata_contains_secret".into());
                }
                if text.len() > 128 {
                    return Err("audit_metadata_value_too_long".into());
                }
                out.insert(key.clone(), Value::String(text.clone()));
            }
            _ => return Err("audit_metadata_value_must_be_scalar".into()),
        }
    }
    let rendered = Value::Object(out.clone()).to_string();
    if contains_canary(&rendered) {
        return Err("audit_metadata_contains_canary".into());
    }
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_canaries_and_omits_unallowlisted_fields() {
        let fields = BTreeMap::from([
            ("request_id".into(), "req-1".into()),
            ("authorization".into(), "Bearer CANARY_SECRET_TOKEN".into()),
            ("document_content".into(), "CANARY_DOCUMENT_TEXT".into()),
            ("raw_path".into(), "/secret/path".into()),
            ("outcome".into(), "success".into()),
        ]);
        let redacted = redacted_fields(&fields);
        assert_eq!(
            redacted.get("request_id").map(String::as_str),
            Some("req-1")
        );
        assert_eq!(redacted.get("outcome").map(String::as_str), Some("success"));
        assert!(!redacted.contains_key("raw_path"));
        assert!(redacted
            .values()
            .all(|value| !value.contains("CANARY_") && !value.contains("/secret/path")));
    }

    #[test]
    fn audit_metadata_allowlist_rejects_secrets() {
        assert!(sanitize_audit_metadata(&json!({"reason": "user_requested"})).is_ok());
        assert!(sanitize_audit_metadata(&json!({"prompt": "CANARY_PROMPT_TEXT"})).is_err());
        assert!(sanitize_audit_metadata(&json!({"reason": "Bearer abc"})).is_err());
        assert!(sanitize_audit_metadata(&json!({"email": "canary@example.com"})).is_err());
    }
}
