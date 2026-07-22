//! Request/job correlation context with W3C Trace Context propagation.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;

use opentelemetry::propagation::{Extractor, Injector, TextMapPropagator};
use opentelemetry::trace::TraceContextExt;
use opentelemetry::Context;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use serde::{Deserialize, Serialize};
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use uuid::Uuid;

/// Correlation fields shared across HTTP, jobs, and workers.
///
/// High-cardinality IDs may appear on spans/logs but never as metric labels.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrelationContext {
    pub request_id: String,
    /// W3C trace-id (32 lowercase hex) when known.
    pub trace_id: String,
    /// Full W3C `traceparent` header value for async job propagation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traceparent: Option<String>,
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

/// Optional worker identity fields (bounded UUID / digest strings only).
#[derive(Debug, Clone, Default)]
pub struct WorkerIds {
    pub org_id: Option<Uuid>,
    pub actor_id: Option<Uuid>,
    pub index_signature: Option<String>,
}

tokio::task_local! {
    static CURRENT: Mutex<CorrelationContext>;
}

impl CorrelationContext {
    pub fn new(request_id: impl Into<String>) -> Self {
        let request_id = request_id.into();
        Self {
            request_id,
            trace_id: String::new(),
            traceparent: None,
            ..Self::default()
        }
    }

    pub fn with_ids(request_id: impl Into<String>, trace_id: impl Into<String>) -> Self {
        let trace_id = trace_id.into();
        let traceparent = synthesize_traceparent(&trace_id);
        Self {
            request_id: request_id.into(),
            trace_id,
            traceparent,
            ..Self::default()
        }
    }

    pub fn current() -> Option<Self> {
        CURRENT
            .try_with(|ctx| ctx.lock().ok().map(|guard| guard.clone()))
            .ok()
            .flatten()
    }

    pub fn request_uuid(&self) -> Option<Uuid> {
        Uuid::parse_str(&self.request_id).ok()
    }
}

/// Enrich the task-local correlation + current span with authenticated ids.
///
/// Only accepts UUID-shaped values; never content/secrets.
pub fn enrich_actor(org_id: Uuid, actor_id: Uuid) {
    let org = org_id.to_string();
    let actor = actor_id.to_string();
    let _ = CURRENT.try_with(|ctx| {
        if let Ok(mut guard) = ctx.lock() {
            guard.org_id = Some(org.clone());
            guard.actor_id = Some(actor.clone());
        }
    });
    let span = tracing::Span::current();
    span.record("org_id", tracing::field::display(&org));
    span.record("actor_id", tracing::field::display(&actor));
}

/// Run `future` with `ctx` installed as the task-local correlation context.
pub async fn scope<F, T>(ctx: CorrelationContext, future: F) -> T
where
    F: Future<Output = T>,
{
    CURRENT.scope(Mutex::new(ctx), future).await
}

/// Attach current correlation + live OTel `traceparent` onto a job payload.
pub fn apply_to_job_payload(payload: &mut crate::jobs::JobPayload, ctx: &CorrelationContext) {
    if payload.request_id.is_none() {
        payload.request_id = ctx.request_uuid();
    }
    if payload.traceparent.is_none() {
        payload.traceparent = inject_current_traceparent()
            .or_else(|| ctx.traceparent.clone())
            .filter(|value| validate_traceparent(value).is_ok());
    }
}

/// Restore a worker correlation context from a claimed job payload.
pub fn from_job_payload(
    job_id: Uuid,
    payload: &crate::jobs::JobPayload,
    ids: WorkerIds,
) -> CorrelationContext {
    let request_id = payload
        .request_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let traceparent = payload
        .traceparent
        .clone()
        .filter(|value| validate_traceparent(value).is_ok());
    let trace_id = traceparent
        .as_deref()
        .and_then(trace_id_from_traceparent)
        .unwrap_or_default();
    let index_signature = ids
        .index_signature
        .filter(|value| is_safe_index_signature(value));
    CorrelationContext {
        request_id,
        trace_id,
        traceparent,
        org_id: ids.org_id.map(|id| id.to_string()),
        actor_id: ids.actor_id.map(|id| id.to_string()),
        job_id: Some(job_id.to_string()),
        document_version_id: payload.version_id.map(|id| id.to_string()),
        index_signature,
    }
}

fn is_safe_index_signature(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 128
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-' || byte == b'_')
        && !contains_secret_fragment(trimmed)
}

fn contains_secret_fragment(value: &str) -> bool {
    value.contains("mh1.")
        || value.contains("Bearer ")
        || value.contains("CANARY_")
        || value.starts_with("eyJ")
}

/// Open a worker span with W3C parent/link from the job `traceparent`.
pub fn worker_span(operation: &'static str, ctx: &CorrelationContext) -> tracing::Span {
    let span = tracing::info_span!(
        "worker",
        otel.name = operation,
        request_id = %ctx.request_id,
        trace_id = %ctx.trace_id,
        job_id = ctx.job_id.as_deref().unwrap_or(""),
        org_id = ctx.org_id.as_deref().unwrap_or(""),
        actor_id = ctx.actor_id.as_deref().unwrap_or(""),
        document_version_id = ctx.document_version_id.as_deref().unwrap_or(""),
        index_signature = ctx.index_signature.as_deref().unwrap_or(""),
        operation = operation,
    );
    if let Some(parent) = ctx
        .traceparent
        .as_deref()
        .and_then(extract_context_from_traceparent)
    {
        let span_context = parent.span().span_context().clone();
        if span_context.is_valid() {
            let _ = span.set_parent(parent);
            span.add_link(span_context);
        }
    }
    span
}

/// Run a worker future under correlation scope + `.instrument(span)` (never `.enter()` across await).
pub async fn run_worker<F, T>(
    operation: &'static str,
    job_id: Uuid,
    payload: &crate::jobs::JobPayload,
    ids: WorkerIds,
    future: F,
) -> T
where
    F: Future<Output = T>,
{
    let correlation = from_job_payload(job_id, payload, ids);
    let span = worker_span(operation, &correlation);
    scope(correlation, future).instrument(span).await
}

/// Validate a W3C `traceparent` value (`version-traceid-spanid-flags`).
pub fn validate_traceparent(value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.len() > 55 || trimmed.len() < 55 {
        return Err("traceparent length invalid".into());
    }
    let mut parts = trimmed.split('-');
    let (Some(version), Some(trace_id), Some(span_id), Some(flags), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return Err("traceparent format invalid".into());
    };
    if version.len() != 2 || !version.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("traceparent version invalid".into());
    }
    if version == "ff" {
        return Err("traceparent version forbidden".into());
    }
    if trace_id.len() != 32
        || !trace_id.chars().all(|ch| ch.is_ascii_hexdigit())
        || trace_id.chars().all(|ch| ch == '0')
    {
        return Err("traceparent trace-id invalid".into());
    }
    if span_id.len() != 16
        || !span_id.chars().all(|ch| ch.is_ascii_hexdigit())
        || span_id.chars().all(|ch| ch == '0')
    {
        return Err("traceparent span-id invalid".into());
    }
    if flags.len() != 2 || !flags.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("traceparent flags invalid".into());
    }
    Ok(())
}

pub fn trace_id_from_traceparent(value: &str) -> Option<String> {
    validate_traceparent(value).ok()?;
    value.split('-').nth(1).map(str::to_ascii_lowercase)
}

pub fn extract_context_from_headers<'a, I>(headers: I) -> Context
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut map = HashMap::new();
    for (key, value) in headers {
        map.insert(key.to_ascii_lowercase(), value.to_string());
    }
    TraceContextPropagator::new().extract(&MapCarrier(&map))
}

pub fn extract_context_from_traceparent(traceparent: &str) -> Option<Context> {
    validate_traceparent(traceparent).ok()?;
    let mut map = HashMap::new();
    map.insert("traceparent".to_string(), traceparent.to_string());
    let cx = TraceContextPropagator::new().extract(&MapCarrier(&map));
    cx.span().span_context().is_valid().then_some(cx)
}

pub fn inject_current_traceparent() -> Option<String> {
    inject_traceparent_from_context(&tracing::Span::current().context())
}

pub fn inject_traceparent_from_span(span: &tracing::Span) -> Option<String> {
    inject_traceparent_from_context(&span.context())
}

pub fn inject_traceparent_from_context(cx: &Context) -> Option<String> {
    let mut map = HashMap::new();
    TraceContextPropagator::new().inject_context(cx, &mut MapCarrierMut(&mut map));
    map.get("traceparent")
        .cloned()
        .filter(|value| validate_traceparent(value).is_ok())
}

fn synthesize_traceparent(trace_id: &str) -> Option<String> {
    let hex = trace_id.replace('-', "").to_ascii_lowercase();
    if hex.len() != 32 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    // Deterministic non-zero span id derived from the trace id for test helpers only.
    let span_id = format!("{:0>16}", &hex[..16.min(hex.len())]);
    let candidate = format!("00-{hex}-{span_id}-01");
    validate_traceparent(&candidate).ok()?;
    Some(candidate)
}

struct MapCarrier<'a>(&'a HashMap<String, String>);

impl Extractor for MapCarrier<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(&key.to_ascii_lowercase()).map(String::as_str)
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(String::as_str).collect()
    }
}

struct MapCarrierMut<'a>(&'a mut HashMap<String, String>);

impl Injector for MapCarrierMut<'_> {
    fn set(&mut self, key: &str, value: String) {
        self.0.insert(key.to_ascii_lowercase(), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::JobPayload;

    #[tokio::test]
    async fn async_scope_propagates_traceparent_to_child_payload() {
        let parent = CorrelationContext::with_ids(
            "550e8400-e29b-41d4-a716-446655440000",
            "4bf92f3577b34da6a3ce929d0e0e4736",
        );
        let observed = scope(parent.clone(), async {
            let current = CorrelationContext::current().expect("task local");
            let mut payload = JobPayload::default();
            apply_to_job_payload(&mut payload, &current);
            let child = from_job_payload(Uuid::new_v4(), &payload, WorkerIds::default());
            let span = worker_span("convert", &child);
            async { (current.request_id, child.request_id, child.trace_id) }
                .instrument(span)
                .await
        })
        .await;
        assert_eq!(observed.0, parent.request_id);
        assert_eq!(observed.1, parent.request_id);
        assert_eq!(observed.2, parent.trace_id);
    }

    #[test]
    fn payload_round_trip_keeps_request_and_traceparent() {
        let ctx = CorrelationContext::with_ids(
            Uuid::new_v4().to_string(),
            "4bf92f3577b34da6a3ce929d0e0e4736",
        );
        let mut payload = JobPayload::default();
        apply_to_job_payload(&mut payload, &ctx);
        assert!(payload.traceparent.is_some());
        let restored = from_job_payload(
            Uuid::new_v4(),
            &payload,
            WorkerIds {
                org_id: Some(Uuid::new_v4()),
                actor_id: Some(Uuid::new_v4()),
                index_signature: Some("a".repeat(64)),
            },
        );
        assert_eq!(restored.request_id, ctx.request_id);
        assert_eq!(restored.trace_id, ctx.trace_id);
        assert!(restored.job_id.is_some());
        assert!(restored.org_id.is_some());
        assert!(restored.actor_id.is_some());
        assert!(restored.index_signature.is_some());
    }

    #[test]
    fn rejects_invalid_traceparent() {
        assert!(
            validate_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").is_ok()
        );
        assert!(validate_traceparent("not-a-traceparent").is_err());
        assert!(
            validate_traceparent("00-00000000000000000000000000000000-00f067aa0ba902b7-01")
                .is_err()
        );
    }

    #[tokio::test]
    async fn enrich_actor_updates_task_local_safely() {
        let ctx = CorrelationContext::new(Uuid::new_v4().to_string());
        let org = Uuid::new_v4();
        let actor = Uuid::new_v4();
        scope(ctx, async {
            enrich_actor(org, actor);
            let current = CorrelationContext::current().expect("ctx");
            assert_eq!(current.org_id.as_deref(), Some(org.to_string().as_str()));
            assert_eq!(
                current.actor_id.as_deref(),
                Some(actor.to_string().as_str())
            );
        })
        .await;
    }
}
