//! End-to-end telemetry: correlation, allowlisted metrics, redaction, optional OTLP.

pub mod config;
pub mod correlation;
pub mod init;
pub mod metrics;
pub mod redact;

pub use config::{OtelExporterKind, TelemetryConfig};
pub use correlation::{
    apply_to_job_payload, enrich_actor, extract_context_from_headers, from_job_payload,
    inject_current_traceparent, inject_traceparent_from_span, run_worker, scope,
    validate_traceparent, worker_span, CorrelationContext, WorkerIds,
};
pub use init::{force_flush, init, init_from_env, runtime, shutdown};
pub use metrics::{
    defer_job_transition, normalize_http_method, normalize_route, observe_duration,
    record_api_request, record_auth_decision, record_conversion, record_drift, record_embedding,
    record_job_transition, record_queue_depth, record_quota, record_reconcile, record_retrieval,
    status_class, validate_metric, Timer,
};
pub use redact::{
    contains_canary, redacted_fields, sanitize_audit_metadata, AUDIT_METADATA_ALLOWLIST,
    CANARY_FRAGMENTS, LOG_FIELD_ALLOWLIST,
};

/// Envelope retained for F-11 contract compatibility (in-memory / fixtures).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    pub metadata: std::collections::BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn propagates_request_to_job_without_sensitive_content() {
        let request = CorrelationContext {
            request_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
            traceparent: Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".into()),
            org_id: Some("770e8400-e29b-41d4-a716-446655440000".into()),
            actor_id: Some("880e8400-e29b-41d4-a716-446655440000".into()),
            ..CorrelationContext::default()
        };
        let job = CorrelationContext {
            job_id: Some("990e8400-e29b-41d4-a716-446655440000".into()),
            document_version_id: Some("aa0e8400-e29b-41d4-a716-446655440000".into()),
            index_signature: Some("sig-1".into()),
            ..request.clone()
        };
        assert_eq!(job.request_id, request.request_id);
        assert_eq!(job.org_id, request.org_id);
        let json = serde_json::to_string(&job).unwrap();
        assert!(!json.contains("documentContent"));
        assert!(!json.contains("prompt"));
        assert!(!contains_canary(&json));
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
        assert_ne!(decoded.outcome, "allowed");
    }
}
