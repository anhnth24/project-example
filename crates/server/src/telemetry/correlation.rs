//! Request/job correlation with W3C Trace Context propagation (P1B-O01).

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::jobs::JobPayload;

/// Correlation fields shared across HTTP, jobs, and workers.
///
/// High-cardinality IDs may appear on spans/logs but never as metric labels.
/// Durable audit uses only the server-minted `request_id` UUID.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrelationContext {
    pub request_id: String,
    /// W3C trace-id (32 lowercase hex) when known.
    pub trace_id: String,
    /// Current span id (16 lowercase hex).
    pub span_id: String,
    /// Parent span id when this context is a child.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
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
    /// Task-local correlation (exported for span-id advance on emit).
    pub(crate) static CURRENT: Mutex<CorrelationContext>;
}

impl CorrelationContext {
    pub fn new(request_id: impl Into<String>) -> Self {
        let request_id = request_id.into();
        let (trace_id, span_id, traceparent) = mint_trace_context();
        Self {
            request_id,
            trace_id,
            span_id,
            parent_span_id: None,
            traceparent: Some(traceparent),
            ..Self::default()
        }
    }

    /// Continue a remote parent: keep trace-id, mint a new child span-id.
    pub fn child_of(request_id: impl Into<String>, parent_traceparent: &str) -> Option<Self> {
        validate_traceparent(parent_traceparent).ok()?;
        let lower = parent_traceparent.trim().to_ascii_lowercase();
        let parts: Vec<&str> = lower.split('-').collect();
        let trace_id = parts[1].to_string();
        let parent_span_id = parts[2].to_string();
        let span_id = mint_span_id();
        let flags = parts[3];
        let traceparent = format!("00-{trace_id}-{span_id}-{flags}");
        Some(Self {
            request_id: request_id.into(),
            trace_id,
            span_id,
            parent_span_id: Some(parent_span_id),
            traceparent: Some(traceparent),
            ..Self::default()
        })
    }

    /// Mint a child span under this context (same trace, new span id).
    pub fn child_span(&self, name_hint: &str) -> Self {
        let _ = name_hint;
        let span_id = mint_span_id();
        let flags = self
            .traceparent
            .as_deref()
            .and_then(|tp| tp.split('-').nth(3))
            .unwrap_or("01");
        let traceparent = format!("00-{}-{}-{flags}", self.trace_id, span_id);
        Self {
            request_id: self.request_id.clone(),
            trace_id: self.trace_id.clone(),
            span_id,
            parent_span_id: Some(self.span_id.clone()),
            traceparent: Some(traceparent),
            org_id: self.org_id.clone(),
            actor_id: self.actor_id.clone(),
            job_id: self.job_id.clone(),
            document_version_id: self.document_version_id.clone(),
            index_signature: self.index_signature.clone(),
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

/// Run `future` with `ctx` installed as the task-local correlation context.
pub async fn scope<F, T>(ctx: CorrelationContext, future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT.scope(Mutex::new(ctx), future).await
}

/// Enrich the task-local correlation with authenticated ids (UUID only).
pub fn enrich_actor(org_id: Uuid, actor_id: Uuid) {
    let org = org_id.to_string();
    let actor = actor_id.to_string();
    let _ = CURRENT.try_with(|ctx| {
        if let Ok(mut guard) = ctx.lock() {
            guard.org_id = Some(org);
            guard.actor_id = Some(actor);
        }
    });
}

/// Attach current correlation onto a job payload for async workers.
pub fn apply_to_job_payload(payload: &mut JobPayload, ctx: &CorrelationContext) {
    if payload.request_id.is_none() {
        payload.request_id = ctx.request_uuid();
    }
    if payload.traceparent.is_none() {
        payload.traceparent = ctx
            .traceparent
            .clone()
            .or_else(|| synthesize_traceparent(&ctx.trace_id, &ctx.span_id))
            .filter(|value| validate_traceparent(value).is_ok());
    }
}

/// Restore a worker correlation context from a claimed job payload.
pub fn from_job_payload(job_id: Uuid, payload: &JobPayload, ids: WorkerIds) -> CorrelationContext {
    let request_id = payload
        .request_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let parent_tp = payload
        .traceparent
        .clone()
        .filter(|value| validate_traceparent(value).is_ok());
    let mut ctx = if let Some(ref tp) = parent_tp {
        CorrelationContext::child_of(request_id.clone(), tp)
            .unwrap_or_else(|| CorrelationContext::new(request_id))
    } else {
        CorrelationContext::new(request_id)
    };
    let index_signature = ids
        .index_signature
        .filter(|value| is_safe_index_signature(value));
    ctx.org_id = ids.org_id.map(|id| id.to_string());
    ctx.actor_id = ids.actor_id.map(|id| id.to_string());
    ctx.job_id = Some(job_id.to_string());
    ctx.document_version_id = payload.version_id.map(|id| id.to_string());
    ctx.index_signature = index_signature;
    ctx
}

/// Validate a W3C `traceparent` header value (strict lowercase hex).
pub fn validate_traceparent(value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.chars().any(|ch| ch.is_ascii_uppercase()) {
        return Err("traceparent must be lowercase".into());
    }
    let parts: Vec<&str> = trimmed.split('-').collect();
    if parts.len() != 4 {
        return Err("traceparent must have 4 dash-separated fields".into());
    }
    if parts[0] != "00" {
        return Err("traceparent version must be 00".into());
    }
    if parts[1].len() != 32 || !parts[1].bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("traceparent trace-id must be 32 hex chars".into());
    }
    if parts[1].bytes().all(|b| b == b'0') {
        return Err("traceparent trace-id must be non-zero".into());
    }
    if parts[2].len() != 16 || !parts[2].bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("traceparent parent-id must be 16 hex chars".into());
    }
    if parts[2].bytes().all(|b| b == b'0') {
        return Err("traceparent parent-id must be non-zero".into());
    }
    if parts[3].len() != 2 || !parts[3].bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("traceparent flags must be 2 hex chars".into());
    }
    Ok(())
}

pub fn trace_id_from_traceparent(value: &str) -> Option<String> {
    validate_traceparent(value).ok()?;
    value.split('-').nth(1).map(|id| id.to_ascii_lowercase())
}

pub fn span_id_from_traceparent(value: &str) -> Option<String> {
    validate_traceparent(value).ok()?;
    value.split('-').nth(2).map(|id| id.to_ascii_lowercase())
}

fn mint_trace_context() -> (String, String, String) {
    let trace_id = format!("{:032x}", Uuid::new_v4().as_u128());
    let span_id = mint_span_id();
    let traceparent = format!("00-{trace_id}-{span_id}-01");
    (trace_id, span_id, traceparent)
}

fn mint_span_id() -> String {
    let mut id = format!("{:016x}", Uuid::new_v4().as_u128() & 0xffff_ffff_ffff_ffff);
    if id.bytes().all(|b| b == b'0') {
        id = "00f067aa0ba902b7".into();
    }
    id
}

fn synthesize_traceparent(trace_id: &str, span_id: &str) -> Option<String> {
    let trace_id = normalize_trace_id(trace_id)?;
    let span_id = if span_id.len() == 16 && span_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        span_id.to_ascii_lowercase()
    } else {
        mint_span_id()
    };
    Some(format!("00-{trace_id}-{span_id}-01"))
}

fn normalize_trace_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim().to_ascii_lowercase();
    if trimmed.len() != 32 || !trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    if trimmed.bytes().all(|b| b == b'0') {
        return None;
    }
    Some(trimmed)
}

fn is_safe_index_signature(value: &str) -> bool {
    (16..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_request_id_and_traceparent_to_job_payload() {
        let ctx = CorrelationContext::new("550e8400-e29b-41d4-a716-446655440000");
        let mut payload = JobPayload::default();
        apply_to_job_payload(&mut payload, &ctx);
        assert_eq!(
            payload.request_id.unwrap().to_string(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
        let tp = payload.traceparent.clone().expect("traceparent");
        assert!(validate_traceparent(&tp).is_ok());
        let restored = from_job_payload(Uuid::new_v4(), &payload, WorkerIds::default());
        assert_eq!(restored.request_id, ctx.request_id);
        assert_eq!(restored.trace_id, ctx.trace_id);
        assert_ne!(restored.span_id, ctx.span_id); // worker creates child span
        assert_eq!(
            restored.parent_span_id.as_deref(),
            Some(ctx.span_id.as_str())
        );
    }

    #[test]
    fn child_of_requires_lowercase_and_mints_new_span() {
        let parent = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let child = CorrelationContext::child_of("550e8400-e29b-41d4-a716-446655440000", parent)
            .expect("child");
        assert_eq!(child.trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(child.parent_span_id.as_deref(), Some("00f067aa0ba902b7"));
        assert_ne!(child.span_id, "00f067aa0ba902b7");
        assert!(validate_traceparent(child.traceparent.as_deref().unwrap()).is_ok());
        assert!(CorrelationContext::child_of(
            "550e8400-e29b-41d4-a716-446655440000",
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00F067AA0BA902B7-01"
        )
        .is_none());
    }

    #[test]
    fn rejects_invalid_traceparent() {
        assert!(validate_traceparent("not-a-trace").is_err());
        assert!(
            validate_traceparent("00-00000000000000000000000000000000-00f067aa0ba902b7-01")
                .is_err()
        );
    }
}
