//! In-memory observability and audit contracts.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod http;
pub mod logging;
pub mod metrics;

pub const SENSITIVE_FRAGMENTS: &[&str] = &[
    "authorization",
    "cookie",
    "database_url",
    "document_content",
    "password",
    "pii",
    "prompt",
    "secret",
    "signed_url",
    "token",
];

pub const FORBIDDEN_METRIC_LABELS: &[&str] = &[
    "actor_id",
    "document_id",
    "filename",
    "job_id",
    "org_id",
    "request_id",
    "url",
    "user_id",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrelationContext {
    pub request_id: String,
    pub trace_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_version_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEvent {
    pub version: u16,
    pub occurred_at: String,
    pub request_id: String,
    pub org_id: String,
    pub actor_id: String,
    pub action: String,
    pub target_type: String,
    pub target_id: String,
    pub outcome: String,
    pub metadata: BTreeMap<String, String>,
}

pub fn redacted_fields(fields: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    fields
        .iter()
        .map(|(key, value)| {
            let value = if is_sensitive_field_name(key) {
                "[REDACTED]".to_string()
            } else {
                value.clone()
            };
            (key.clone(), value)
        })
        .collect()
}

pub fn redacted_json_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(object) => serde_json::Value::Object(
            object
                .into_iter()
                .map(|(key, value)| {
                    let value = if is_sensitive_field_name(&key) {
                        serde_json::Value::String("[REDACTED]".into())
                    } else {
                        redacted_json_value(value)
                    };
                    (key, value)
                })
                .collect(),
        ),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(redacted_json_value).collect())
        }
        other => other,
    }
}

pub fn redacted_field_value(key: &str, value: impl Into<String>) -> String {
    if is_sensitive_field_name(key) {
        "[REDACTED]".into()
    } else {
        value.into()
    }
}

fn is_sensitive_field_name(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    let compact = normalized
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .collect::<Vec<_>>();
    SENSITIVE_FRAGMENTS.iter().any(|fragment| {
        let fragment_compact = fragment
            .bytes()
            .filter(|byte| byte.is_ascii_alphanumeric())
            .collect::<Vec<_>>();
        normalized.contains(fragment)
            || compact
                .windows(fragment_compact.len())
                .any(|window| window == fragment_compact.as_slice())
    })
}

pub fn validate_metric(name: &str, labels: &[&str]) -> Result<(), String> {
    if !name.starts_with("markhand_")
        || name.is_empty()
        || name
            .bytes()
            .any(|byte| !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'))
    {
        return Err("metric name must be markhand_ prefixed snake_case".into());
    }
    if let Some(label) = labels
        .iter()
        .find(|label| FORBIDDEN_METRIC_LABELS.contains(label))
    {
        return Err(format!("metric label has unbounded cardinality: {label}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        redacted_fields, redacted_json_value, validate_metric, AuditEvent, CorrelationContext,
    };
    use std::collections::BTreeMap;

    #[test]
    fn propagates_request_to_job_without_sensitive_content() {
        let request = CorrelationContext {
            request_id: "req-1".into(),
            trace_id: "trace-1".into(),
            org_id: Some("org-1".into()),
            actor_id: Some("user-1".into()),
            ..CorrelationContext::default()
        };
        let job = CorrelationContext {
            job_id: Some("job-1".into()),
            document_version_id: Some("version-1".into()),
            index_signature: Some("sig-1".into()),
            ..request.clone()
        };
        assert_eq!(job.request_id, request.request_id);
        assert_eq!(job.org_id, request.org_id);
        let json = serde_json::to_string(&job).unwrap();
        assert!(!json.contains("documentContent"));
        assert!(!json.contains("prompt"));
    }

    #[test]
    fn redacts_canaries_and_rejects_high_cardinality_labels() {
        let fields = BTreeMap::from([
            ("request_id".into(), "req-1".into()),
            ("authorization".into(), "Bearer canary-token".into()),
            ("document_content".into(), "private text".into()),
            (
                "database_url".into(),
                "postgres://user:secret@host/db".into(),
            ),
        ]);
        let redacted = redacted_fields(&fields);
        assert_eq!(redacted["request_id"], "req-1");
        assert!(redacted
            .values()
            .all(|value| !value.contains("canary-token") && !value.contains("private text")));
        assert!(validate_metric("markhand_job_duration_seconds", &["job_type", "outcome"]).is_ok());
        assert!(validate_metric("markhand_job_total", &["org_id"]).is_err());
        assert!(validate_metric("Bad-Metric", &[]).is_err());
    }

    #[test]
    fn redacts_camel_case_sensitive_metadata() {
        let metadata = serde_json::json!({
            "documentContent": "never expose this private text",
            "safeId": "safe",
            "nested": { "signedUrl": "https://example.test/token" }
        });
        let redacted = redacted_json_value(metadata);
        let rendered = redacted.to_string();
        assert!(!rendered.contains("private text"));
        assert!(!rendered.contains("example.test/token"));
        assert!(rendered.contains("safe"));
        assert!(rendered.contains("[REDACTED]"));
    }

    #[test]
    fn audit_envelope_round_trips_as_version_one() {
        let event = AuditEvent {
            version: 1,
            occurred_at: "2026-07-17T00:00:00Z".into(),
            request_id: "req-1".into(),
            org_id: "org-1".into(),
            actor_id: "actor-1".into(),
            action: "document.delete".into(),
            target_type: "document".into(),
            target_id: "doc-1".into(),
            outcome: "allowed".into(),
            metadata: BTreeMap::from([("reason".into(), "user_requested".into())]),
        };
        let json = serde_json::to_string(&event).unwrap();
        let decoded: AuditEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, event);
    }
}
