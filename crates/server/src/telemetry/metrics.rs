//! Allowlisted OpenTelemetry metrics (no private-only registry).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use opentelemetry::metrics::{Counter, Histogram, Meter, ObservableGauge};
use opentelemetry::KeyValue;
use serde::{Deserialize, Serialize};

/// Job transitions deferred until the enclosing org txn commits (or savepoint releases).
#[derive(Debug, Clone, Copy)]
struct DeferredJobTransition {
    job_type: &'static str,
    transition: &'static str,
    result: &'static str,
}

tokio::task_local! {
    static DEFERRED_JOB_TRANSITIONS: RefCell<Vec<DeferredJobTransition>>;
    static DEFERRED_SAVEPOINT_MARKS: RefCell<Vec<usize>>;
}

const FORBIDDEN_LABEL_KEYS: &[&str] = &[
    "actor_id",
    "document_id",
    "email",
    "filename",
    "job_id",
    "object_key",
    "org_id",
    "path",
    "query",
    "request_id",
    "trace_id",
    "url",
    "user_id",
    "version_id",
];

/// Canonical metric names (must stay low-cardinality).
pub mod names {
    pub const API_REQUEST_DURATION_SECONDS: &str = "markhand_api_request_duration_seconds";
    pub const API_REQUESTS_TOTAL: &str = "markhand_api_requests_total";
    pub const QUEUE_DEPTH: &str = "markhand_queue_depth";
    pub const QUEUE_OLDEST_AGE_SECONDS: &str = "markhand_queue_oldest_age_seconds";
    pub const CONVERSION_DURATION_SECONDS: &str = "markhand_conversion_duration_seconds";
    pub const CONVERSION_TOTAL: &str = "markhand_conversion_total";
    pub const EMBEDDING_DURATION_SECONDS: &str = "markhand_embedding_duration_seconds";
    pub const EMBEDDING_TOTAL: &str = "markhand_embedding_total";
    pub const RETRIEVAL_DURATION_SECONDS: &str = "markhand_retrieval_duration_seconds";
    pub const RETRIEVAL_TOTAL: &str = "markhand_retrieval_total";
    pub const DRIFT_TOTAL: &str = "markhand_drift_total";
    pub const RECONCILE_TOTAL: &str = "markhand_reconcile_total";
    pub const QUOTA_DECISIONS_TOTAL: &str = "markhand_quota_decisions_total";
    pub const JOB_TRANSITIONS_TOTAL: &str = "markhand_job_transitions_total";
    pub const AUTH_DECISIONS_TOTAL: &str = "markhand_auth_decisions_total";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricSample {
    pub name: String,
    pub labels: BTreeMap<String, String>,
    pub value: u64,
}

struct Instruments {
    api_requests: Counter<u64>,
    api_duration: Histogram<f64>,
    conversion_total: Counter<u64>,
    conversion_duration: Histogram<f64>,
    embedding_total: Counter<u64>,
    embedding_duration: Histogram<f64>,
    retrieval_total: Counter<u64>,
    retrieval_duration: Histogram<f64>,
    drift_total: Counter<u64>,
    reconcile_total: Counter<u64>,
    quota_total: Counter<u64>,
    job_transitions: Counter<u64>,
    auth_decisions: Counter<u64>,
    /// Keeps ObservableGauge registrations alive for the process lifetime.
    _queue_depth_gauge: ObservableGauge<u64>,
    _queue_age_gauge: ObservableGauge<u64>,
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static INSTRUMENTS: OnceLock<Instruments> = OnceLock::new();
static QUEUE_OBSERVABLES: OnceLock<Mutex<BTreeMap<&'static str, (AtomicU64, AtomicU64)>>> =
    OnceLock::new();

fn queue_observables() -> &'static Mutex<BTreeMap<&'static str, (AtomicU64, AtomicU64)>> {
    QUEUE_OBSERVABLES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Install instruments on `meter`. Called once from telemetry init on success.
pub fn install(meter: &Meter, metrics_enabled: bool) -> Result<(), String> {
    ENABLED.store(metrics_enabled, Ordering::SeqCst);
    if !metrics_enabled {
        return Ok(());
    }
    if INSTRUMENTS.get().is_some() {
        return Ok(());
    }

    let queue_depth_gauge = meter
        .u64_observable_gauge(names::QUEUE_DEPTH)
        .with_description("Durable job queue depth by queue name")
        .with_callback(|observer| {
            if let Ok(guard) = queue_observables().lock() {
                for (queue, (depth, _)) in guard.iter() {
                    observer.observe(
                        depth.load(Ordering::Relaxed),
                        &[KeyValue::new("queue", *queue)],
                    );
                }
            }
        })
        .build();
    let queue_age_gauge = meter
        .u64_observable_gauge(names::QUEUE_OLDEST_AGE_SECONDS)
        .with_description("Age in seconds of the oldest pending job")
        .with_callback(|observer| {
            if let Ok(guard) = queue_observables().lock() {
                for (queue, (_, age)) in guard.iter() {
                    observer.observe(
                        age.load(Ordering::Relaxed),
                        &[KeyValue::new("queue", *queue)],
                    );
                }
            }
        })
        .build();

    let instruments = Instruments {
        api_requests: meter.u64_counter(names::API_REQUESTS_TOTAL).build(),
        api_duration: meter
            .f64_histogram(names::API_REQUEST_DURATION_SECONDS)
            .build(),
        conversion_total: meter.u64_counter(names::CONVERSION_TOTAL).build(),
        conversion_duration: meter
            .f64_histogram(names::CONVERSION_DURATION_SECONDS)
            .build(),
        embedding_total: meter.u64_counter(names::EMBEDDING_TOTAL).build(),
        embedding_duration: meter
            .f64_histogram(names::EMBEDDING_DURATION_SECONDS)
            .build(),
        retrieval_total: meter.u64_counter(names::RETRIEVAL_TOTAL).build(),
        retrieval_duration: meter
            .f64_histogram(names::RETRIEVAL_DURATION_SECONDS)
            .build(),
        drift_total: meter.u64_counter(names::DRIFT_TOTAL).build(),
        reconcile_total: meter.u64_counter(names::RECONCILE_TOTAL).build(),
        quota_total: meter.u64_counter(names::QUOTA_DECISIONS_TOTAL).build(),
        job_transitions: meter.u64_counter(names::JOB_TRANSITIONS_TOTAL).build(),
        auth_decisions: meter.u64_counter(names::AUTH_DECISIONS_TOTAL).build(),
        _queue_depth_gauge: queue_depth_gauge,
        _queue_age_gauge: queue_age_gauge,
    };
    INSTRUMENTS
        .set(instruments)
        .map_err(|_| "metrics instruments already installed".to_string())?;
    Ok(())
}

fn instruments() -> Option<&'static Instruments> {
    if !ENABLED.load(Ordering::SeqCst) {
        return None;
    }
    INSTRUMENTS.get()
}

pub fn metrics_enabled() -> bool {
    ENABLED.load(Ordering::SeqCst) && INSTRUMENTS.get().is_some()
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
        .find(|label| FORBIDDEN_LABEL_KEYS.contains(label))
    {
        return Err(format!("metric label has unbounded cardinality: {label}"));
    }
    Ok(())
}

fn attrs(labels: &[(&str, &str)]) -> Option<Vec<KeyValue>> {
    for (key, value) in labels {
        if FORBIDDEN_LABEL_KEYS.contains(key) {
            return None;
        }
        if key.is_empty()
            || key
                .bytes()
                .any(|byte| !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'))
        {
            return None;
        }
        if value.len() > 64 || value.chars().any(|ch| ch.is_control()) {
            return None;
        }
    }
    Some(
        labels
            .iter()
            .map(|(k, v)| KeyValue::new((*k).to_string(), (*v).to_string()))
            .collect(),
    )
}

/// Exact label schema + bounded enums for API metrics.
pub fn normalize_http_method(method: &str) -> &'static str {
    match method.trim().to_ascii_uppercase().as_str() {
        "GET" => "GET",
        "POST" => "POST",
        "PUT" => "PUT",
        "PATCH" => "PATCH",
        "DELETE" => "DELETE",
        "HEAD" => "HEAD",
        "OPTIONS" => "OPTIONS",
        _ => "OTHER",
    }
}

/// Templated route allowlist (never raw paths).
pub fn normalize_route(route: &str) -> &'static str {
    match route {
        "health" => "health",
        "auth" => "auth",
        "upload" => "upload",
        "search" => "search",
        "stream" => "stream",
        "documents" => "documents",
        "collections" => "collections",
        "jobs" => "jobs",
        "openapi" => "openapi",
        _ => "other",
    }
}

pub fn status_class(status: u16) -> &'static str {
    match status {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "unknown",
    }
}

fn normalize_format(format: &str) -> &'static str {
    match format.trim().to_ascii_lowercase().as_str() {
        "pdf" => "pdf",
        "docx" => "docx",
        "pptx" => "pptx",
        "xlsx" | "xls" | "xlsb" | "ods" => "spreadsheet",
        "csv" => "csv",
        "html" => "html",
        "txt" => "txt",
        "png" | "jpeg" | "jpg" | "webp" | "tiff" | "bmp" => "image",
        "wav" | "mp3" | "ogg" | "flac" | "m4a" => "audio",
        "zip" => "zip",
        "document" => "document",
        _ => "other",
    }
}

fn normalize_result(result: &str) -> &'static str {
    match result {
        "success" | "ok" | "completed" | "succeeded" => "success",
        "failed" | "fail" | "error" => "error",
        "deny" | "denied" => "deny",
        "retry" => "retry",
        "dead_letter" => "dead_letter",
        "cancelled" | "canceled" => "cancelled",
        "leased" => "leased",
        "accepted" => "accepted",
        "other" => "other",
        _ => "other",
    }
}

fn normalize_job_type(job_type: &str) -> &'static str {
    match job_type {
        "convert" => "convert",
        "embed" | "embedding" | "embedding_batch" => "embed",
        "index" => "index",
        "reconcile" => "reconcile",
        "delete" => "delete",
        _ => "other",
    }
}

fn normalize_transition(transition: &str) -> &'static str {
    match transition {
        "enqueue" => "enqueue",
        "claim" => "claim",
        "finish" => "finish",
        _ => "other",
    }
}

fn normalize_queue(queue: &str) -> Option<&'static str> {
    match queue {
        "convert" => Some("convert"),
        "embed" | "embedding" | "embedding_batch" => Some("embed"),
        "index" => Some("index"),
        "reconcile" => Some("reconcile"),
        "delete" => Some("delete"),
        _ => None,
    }
}

fn normalize_reconcile_mode(mode: &str) -> &'static str {
    match mode {
        "detect" | "dry-run" | "dry_run" | "dryrun" => "detect",
        "repair" => "repair",
        _ => "other",
    }
}

fn normalize_reconcile_result(result: &str) -> &'static str {
    match result {
        "success" | "ok" | "clean" => "success",
        "drift" => "drift",
        "error" | "failed" => "error",
        "noop" => "noop",
        _ => "other",
    }
}

fn normalize_quota_decision(decision: &str) -> &'static str {
    match decision {
        "reserve" => "reserve",
        "deny" => "deny",
        "refund" | "release" => "refund",
        "commit" | "finalize" => "commit",
        "error" => "error",
        _ => "other",
    }
}

fn normalize_resource_kind(kind: &str) -> &'static str {
    match kind {
        "documents" => "documents",
        "storage_bytes" => "storage_bytes",
        "concurrent_jobs" => "jobs",
        "tokens" => "tokens",
        "embeddings" => "embeddings",
        "unknown" => "unknown",
        _ => "other",
    }
}

fn normalize_auth_code(code: &str) -> &'static str {
    match code {
        "permission_denied" => "permission_denied",
        "membership_missing" => "membership_missing",
        "user_disabled" => "user_disabled",
        "collection_denied" => "collection_denied",
        "unauthorized" => "unauthorized",
        "invalid_credentials" => "invalid_credentials",
        _ => "other",
    }
}

fn normalize_leg(leg: &str) -> &'static str {
    match leg {
        "hybrid" => "hybrid",
        "lexical" => "lexical",
        "vector" => "vector",
        "qa_provider" => "qa_provider",
        _ => "other",
    }
}

fn normalize_drift_kind(kind: &str) -> &'static str {
    match kind {
        "object" => "object",
        "vector" => "vector",
        "index" => "index",
        _ => "other",
    }
}

fn normalize_drift_state(state: &str) -> &'static str {
    match state {
        "orphan" => "orphan",
        "missing" => "missing",
        "stale" => "stale",
        _ => "other",
    }
}

pub fn record_api_request(route: &str, method: &str, status: u16, duration: Duration) {
    let Some(instr) = instruments() else {
        return;
    };
    // Custom/canary methods collapse to OTHER; raw method text is never labeled.
    let route = normalize_route(route);
    let method = normalize_http_method(method);
    let class = status_class(status);
    let labels = [
        ("route", route),
        ("method", method),
        ("status_class", class),
    ];
    let Some(kv) = attrs(&labels) else {
        return;
    };
    let _ = validate_metric(
        names::API_REQUESTS_TOTAL,
        &["route", "method", "status_class"],
    );
    instr.api_requests.add(1, &kv);
    instr.api_duration.record(duration.as_secs_f64(), &kv);
}

pub fn record_job_transition(job_type: &str, transition: &str, result: &str) {
    let Some(instr) = instruments() else {
        return;
    };
    let labels = [
        ("job_type", normalize_job_type(job_type)),
        ("transition", normalize_transition(transition)),
        ("result", normalize_result(result)),
    ];
    let Some(kv) = attrs(&labels) else {
        return;
    };
    instr.job_transitions.add(1, &kv);
}

/// Defer a job transition until [`flush_deferred_job_transitions`] (txn commit).
///
/// Outside a deferred scope this records immediately (claim paths after commit).
pub fn defer_job_transition(job_type: &str, transition: &str, result: &str) {
    let job_type = normalize_job_type(job_type);
    let transition = normalize_transition(transition);
    let result = normalize_result(result);
    let deferred = DeferredJobTransition {
        job_type,
        transition,
        result,
    };
    let queued = DEFERRED_JOB_TRANSITIONS
        .try_with(|cell| {
            cell.borrow_mut().push(deferred);
            true
        })
        .unwrap_or(false);
    if !queued {
        record_job_transition(job_type, transition, result);
    }
}

/// Run `future` with a deferred job-metric buffer flushed only on `Ok`.
pub async fn scope_deferred_job_metrics<F, T, E>(future: F) -> Result<T, E>
where
    F: std::future::Future<Output = Result<T, E>>,
{
    DEFERRED_JOB_TRANSITIONS
        .scope(RefCell::new(Vec::new()), async {
            DEFERRED_SAVEPOINT_MARKS
                .scope(RefCell::new(Vec::new()), async {
                    match future.await {
                        Ok(value) => {
                            flush_deferred_job_transitions();
                            Ok(value)
                        }
                        Err(error) => {
                            discard_deferred_job_transitions();
                            Err(error)
                        }
                    }
                })
                .await
        })
        .await
}

pub fn deferred_savepoint_push() {
    let _ = DEFERRED_SAVEPOINT_MARKS.try_with(|marks| {
        let len = DEFERRED_JOB_TRANSITIONS
            .try_with(|cell| cell.borrow().len())
            .unwrap_or(0);
        marks.borrow_mut().push(len);
    });
}

pub fn deferred_savepoint_release() {
    let _ = DEFERRED_SAVEPOINT_MARKS.try_with(|marks| {
        marks.borrow_mut().pop();
    });
}

pub fn deferred_savepoint_rollback() {
    let _ = DEFERRED_SAVEPOINT_MARKS.try_with(|marks| {
        if let Some(mark) = marks.borrow_mut().pop() {
            let _ = DEFERRED_JOB_TRANSITIONS.try_with(|cell| {
                cell.borrow_mut().truncate(mark);
            });
        }
    });
}

fn flush_deferred_job_transitions() {
    let pending = DEFERRED_JOB_TRANSITIONS
        .try_with(|cell| std::mem::take(&mut *cell.borrow_mut()))
        .unwrap_or_default();
    for item in pending {
        record_job_transition(item.job_type, item.transition, item.result);
    }
}

fn discard_deferred_job_transitions() {
    let _ = DEFERRED_JOB_TRANSITIONS.try_with(|cell| cell.borrow_mut().clear());
    let _ = DEFERRED_SAVEPOINT_MARKS.try_with(|marks| marks.borrow_mut().clear());
}

pub fn record_conversion(format: &str, result: &str, duration: Duration) {
    let Some(instr) = instruments() else {
        return;
    };
    let labels = [
        ("format", normalize_format(format)),
        ("result", normalize_result(result)),
    ];
    let Some(kv) = attrs(&labels) else {
        return;
    };
    instr.conversion_total.add(1, &kv);
    instr
        .conversion_duration
        .record(duration.as_secs_f64(), &kv);
}

pub fn record_embedding(result: &str, duration: Duration) {
    let Some(instr) = instruments() else {
        return;
    };
    let labels = [("result", normalize_result(result))];
    let Some(kv) = attrs(&labels) else {
        return;
    };
    instr.embedding_total.add(1, &kv);
    instr.embedding_duration.record(duration.as_secs_f64(), &kv);
}

pub fn record_retrieval(leg: &str, result: &str, duration: Duration) {
    let Some(instr) = instruments() else {
        return;
    };
    let labels = [
        ("leg", normalize_leg(leg)),
        ("result", normalize_result(result)),
    ];
    let Some(kv) = attrs(&labels) else {
        return;
    };
    instr.retrieval_total.add(1, &kv);
    instr.retrieval_duration.record(duration.as_secs_f64(), &kv);
}

pub fn record_drift(kind: &str, state: &str) {
    let Some(instr) = instruments() else {
        return;
    };
    let labels = [
        ("kind", normalize_drift_kind(kind)),
        ("state", normalize_drift_state(state)),
    ];
    let Some(kv) = attrs(&labels) else {
        return;
    };
    instr.drift_total.add(1, &kv);
}

/// Reconcile metrics keep mode and result as separate bounded labels.
pub fn record_reconcile(mode: &str, result: &str) {
    let Some(instr) = instruments() else {
        return;
    };
    let labels = [
        ("mode", normalize_reconcile_mode(mode)),
        ("result", normalize_reconcile_result(result)),
    ];
    let Some(kv) = attrs(&labels) else {
        return;
    };
    instr.reconcile_total.add(1, &kv);
}

pub fn record_quota(decision: &str, resource_kind: &str) {
    let Some(instr) = instruments() else {
        return;
    };
    let labels = [
        ("decision", normalize_quota_decision(decision)),
        ("resource_kind", normalize_resource_kind(resource_kind)),
    ];
    let Some(kv) = attrs(&labels) else {
        return;
    };
    instr.quota_total.add(1, &kv);
}

pub fn record_auth_decision(result: &str, code: &str) {
    let Some(instr) = instruments() else {
        return;
    };
    let labels = [
        ("result", normalize_result(result)),
        ("code", normalize_auth_code(code)),
    ];
    let Some(kv) = attrs(&labels) else {
        return;
    };
    instr.auth_decisions.add(1, &kv);
}

/// Update observable queue gauges from a caller that measured depth/age.
pub fn record_queue_depth(queue: &str, depth: u64, oldest_age: Duration) {
    if !ENABLED.load(Ordering::SeqCst) {
        return;
    }
    let Some(queue) = normalize_queue(queue) else {
        return;
    };
    if let Ok(mut guard) = queue_observables().lock() {
        let entry = guard
            .entry(queue)
            .or_insert_with(|| (AtomicU64::new(0), AtomicU64::new(0)));
        entry.0.store(depth, Ordering::Relaxed);
        entry.1.store(oldest_age.as_secs(), Ordering::Relaxed);
    }
}

/// Compatibility helper retained for call sites that still observe durations generically.
pub fn observe_duration(name: &str, labels: &[(&str, &str)], duration: Duration) {
    match name {
        names::API_REQUEST_DURATION_SECONDS => {
            let route = labels
                .iter()
                .find(|(k, _)| *k == "route")
                .map(|(_, v)| *v)
                .unwrap_or("other");
            let method = labels
                .iter()
                .find(|(k, _)| *k == "method")
                .map(|(_, v)| *v)
                .unwrap_or("OTHER");
            let status = labels
                .iter()
                .find(|(k, _)| *k == "status_class")
                .map(|(_, v)| match *v {
                    "2xx" => 200,
                    "4xx" => 400,
                    "5xx" => 500,
                    _ => 0,
                })
                .unwrap_or(0);
            record_api_request(route, method, status, duration);
        }
        names::CONVERSION_DURATION_SECONDS => {
            let format = labels
                .iter()
                .find(|(k, _)| *k == "format")
                .map(|(_, v)| *v)
                .unwrap_or("other");
            let result = labels
                .iter()
                .find(|(k, _)| *k == "result")
                .map(|(_, v)| *v)
                .unwrap_or("other");
            record_conversion(format, result, duration);
        }
        _ => {}
    }
}

/// RAII timer that records duration on drop via allowlisted helpers.
pub struct Timer {
    name: &'static str,
    labels: Vec<(String, String)>,
    start: std::time::Instant,
}

impl Timer {
    pub fn start(name: &'static str, labels: &[(&str, &str)]) -> Self {
        Self {
            name,
            labels: labels
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            start: std::time::Instant::now(),
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        let refs: Vec<(&str, &str)> = self
            .labels
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        observe_duration(self.name, &refs, self.start.elapsed());
    }
}

/// Test-only counter snapshot reconstructed from queue observables + no private registry.
/// Production metrics are exported via OTLP / in-memory OTel reader.
#[cfg(test)]
pub fn snapshot_counters() -> Vec<MetricSample> {
    // Without forcing a metric reader flush, counters live in the SDK pipeline.
    // Unit tests assert schema via validate/normalize helpers; live exporter tests
    // use `telemetry::init::test_metric_exporter`.
    Vec::new()
}

#[cfg(test)]
pub fn reset_for_tests() {
    if let Ok(mut guard) = queue_observables().lock() {
        guard.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_high_cardinality_labels() {
        assert!(validate_metric("markhand_job_total", &["job_type", "outcome"]).is_ok());
        assert!(validate_metric("markhand_job_total", &["org_id"]).is_err());
        assert!(validate_metric("markhand_job_total", &["request_id"]).is_err());
        assert!(validate_metric("Bad-Metric", &[]).is_err());
        assert!(attrs(&[("path", "/a")]).is_none());
        assert!(attrs(&[("query", "q=1")]).is_none());
    }

    #[test]
    fn http_method_and_route_are_bounded() {
        assert_eq!(normalize_http_method("get"), "GET");
        assert_eq!(normalize_http_method("CANARY_CUSTOM_METHOD"), "OTHER");
        assert_eq!(normalize_route("documents"), "documents");
        assert_eq!(normalize_route("/api/v1/documents/abc"), "other");
        assert_eq!(normalize_format("PDF"), "pdf");
        assert_eq!(normalize_format("weird"), "other");
    }
}
