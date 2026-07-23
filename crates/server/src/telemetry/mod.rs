//! Observability contracts: correlation, redaction, metrics, bounded export (P1B-O01).

pub mod config;
pub mod correlation;
pub mod exporter;
pub mod metrics;

pub use config::{OtelExporterKind, TelemetryConfig};
pub use correlation::{
    apply_to_job_payload, enrich_actor, from_job_payload, scope, validate_traceparent,
    CorrelationContext, WorkerIds,
};
pub use exporter::otlp_span_kind;
pub use exporter::{
    assert_otlp_batch_parent_graph, complete_current_span, emit_span, inc_drift, inc_quota,
    inject_latency_for_tests, record_conversion, record_embedding_batch, record_http_request,
    record_provider_call, record_retrieval_leg, set_backup_age_seconds, set_queue_age_seconds,
    set_queue_depth, start_span, ExportRecord, MetricsRegistry, SpanGuard, METRIC_EXPORTER_DROPPED,
    METRIC_EXPORTER_EXPORT,
};
// MetricsRegistry::shutdown_flush is used by the API binary SIGTERM path.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

const SENSITIVE_FRAGMENTS: &[&str] = &[
    "authorization",
    "cookie",
    "database_url",
    "document_content",
    "password",
    "pii",
    "prompt",
    "question",
    "answer",
    "secret",
    "signed_url",
    "token",
];

/// Value fragments that must never appear in emitted diagnostics.
pub const CANARY_FRAGMENTS: &[&str] = &[
    "CANARY_SECRET_TOKEN",
    "CANARY_DOCUMENT_TEXT",
    "CANARY_DOC_TEXT",
    "CANARY_PROMPT_TEXT",
    "CANARY_ANSWER_TEXT",
    "CANARY_API_KEY",
    "canary@example.com",
    "Bearer sk-canary",
    "postgres://canary:",
];

const FORBIDDEN_METRIC_LABELS: &[&str] = &[
    "actor_id",
    "document_id",
    "filename",
    "job_id",
    "org_id",
    "request_id",
    "url",
    "user_id",
    "version_id",
    "trace_id",
];

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
            let normalized = key.to_ascii_lowercase();
            let value = if SENSITIVE_FRAGMENTS
                .iter()
                .any(|fragment| normalized.contains(fragment))
            {
                "[REDACTED]".to_string()
            } else {
                value.clone()
            };
            (key.clone(), value)
        })
        .collect()
}

pub fn contains_canary(text: &str) -> bool {
    CANARY_FRAGMENTS
        .iter()
        .any(|fragment| text.contains(fragment))
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

/// Initialise process-wide telemetry from config (safe to call once).
pub fn init(config: &TelemetryConfig) {
    MetricsRegistry::configure(config);
    if config.exporter_enabled() {
        tracing::info!(
            target: "telemetry",
            service = %config.service_name,
            metrics_enabled = config.metrics_enabled,
            queue_capacity = config.export_queue_capacity,
            "telemetry exporter configured"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        contains_canary, redacted_fields, validate_metric, AuditEvent, CorrelationContext,
        CANARY_FRAGMENTS,
    };
    use std::collections::BTreeMap;

    #[test]
    fn propagates_request_to_job_without_sensitive_content() {
        let request = CorrelationContext::new("550e8400-e29b-41d4-a716-446655440000");
        let job = CorrelationContext {
            job_id: Some("990e8400-e29b-41d4-a716-446655440000".into()),
            document_version_id: Some("aa0e8400-e29b-41d4-a716-446655440000".into()),
            index_signature: Some("sig-1".into()),
            ..request.clone()
        };
        assert_eq!(job.request_id, request.request_id);
        assert_eq!(job.trace_id, request.trace_id);
        let json = serde_json::to_string(&job).unwrap();
        assert!(!json.contains("documentContent"));
        assert!(!json.contains("prompt"));
        assert!(!contains_canary(&json));
    }

    #[test]
    fn redacts_canaries_and_rejects_high_cardinality_labels() {
        let fields = BTreeMap::from([
            ("request_id".into(), "req-1".into()),
            ("authorization".into(), "Bearer CANARY_SECRET_TOKEN".into()),
            ("document_content".into(), "CANARY_DOCUMENT_TEXT".into()),
            ("question".into(), "CANARY_PROMPT_TEXT".into()),
            ("answer".into(), "CANARY_ANSWER_TEXT".into()),
            (
                "database_url".into(),
                "postgres://canary:hunter2@db/markhand".into(),
            ),
        ]);
        let redacted = redacted_fields(&fields);
        assert_eq!(redacted["request_id"], "req-1");
        assert!(redacted.values().all(|value| !contains_canary(value)));
        for fragment in CANARY_FRAGMENTS {
            let _ = fragment;
        }
        assert!(validate_metric("markhand_job_duration_seconds", &["job_type", "outcome"]).is_ok());
        assert!(validate_metric("markhand_job_total", &["org_id"]).is_err());
        assert!(validate_metric("Bad-Metric", &[]).is_err());
    }

    #[test]
    fn audit_envelope_round_trips_as_version_one() {
        let event = AuditEvent {
            version: 1,
            occurred_at: "2026-07-17T00:00:00Z".into(),
            request_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            org_id: "770e8400-e29b-41d4-a716-446655440000".into(),
            actor_id: "880e8400-e29b-41d4-a716-446655440000".into(),
            action: "document.delete".into(),
            target_type: "document".into(),
            target_id: "990e8400-e29b-41d4-a716-446655440000".into(),
            outcome: "success".into(),
            metadata: BTreeMap::from([("reason".into(), "user_requested".into())]),
        };
        let json = serde_json::to_string(&event).unwrap();
        let decoded: AuditEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, event);
    }
}
