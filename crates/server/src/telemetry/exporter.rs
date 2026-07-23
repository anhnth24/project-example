//! Process-wide Prometheus text exporter + bounded OTLP span queue (P1B-O01).
//!
//! Emits only allowlisted metric names and low-cardinality labels. No
//! `org_id` / `user_id` / `document_id` / `request_id` / filenames.
//!
//! Span/event export uses a fixed-capacity queue: when full, new records are
//! dropped and `markhand_exporter_dropped_total` increments (backpressure).

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value as JsonValue};

use super::correlation::CURRENT;
use super::metrics::{
    METRIC_BACKUP_AGE, METRIC_CONVERSION, METRIC_DRIFT, METRIC_EMBEDDING_BATCH, METRIC_QUEUE_AGE,
    METRIC_QUEUE_DEPTH, METRIC_QUOTA, METRIC_REQUEST_LATENCY, METRIC_RETRIEVAL_LEG,
};
use super::validate_metric;
use super::TelemetryConfig;

const LATENCY_BUCKETS_SECONDS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0,
];
const LABEL_SET_CAP: usize = 128;
pub const METRIC_EXPORTER_DROPPED: &str = "markhand_exporter_dropped_total";
pub const METRIC_EXPORTER_EXPORT: &str = "markhand_exporter_export_total";

#[derive(Debug, Default)]
struct CounterSeries {
    values: BTreeMap<Vec<(String, String)>, u64>,
}

#[derive(Debug, Default)]
struct GaugeSeries {
    values: BTreeMap<Vec<(String, String)>, f64>,
}

#[derive(Debug, Default)]
struct HistogramSeries {
    values: BTreeMap<Vec<(String, String)>, HistogramState>,
}

#[derive(Debug, Clone)]
struct HistogramState {
    buckets: Vec<u64>,
    sum: f64,
    count: u64,
}

impl Default for HistogramState {
    fn default() -> Self {
        Self {
            buckets: vec![0; LATENCY_BUCKETS_SECONDS.len() + 1],
            sum: 0.0,
            count: 0,
        }
    }
}

#[derive(Debug, Default)]
struct RegistryInner {
    counters: BTreeMap<&'static str, CounterSeries>,
    gauges: BTreeMap<&'static str, GaugeSeries>,
    histograms: BTreeMap<&'static str, HistogramSeries>,
}

/// One allowlisted span/event record for bounded export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRecord {
    pub name: String,
    pub request_id: String,
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub span_kind: String,
    pub outcome: String,
    pub start_time_unix_nano: u64,
    pub end_time_unix_nano: u64,
}

impl ExportRecord {
    pub fn duration_ms(&self) -> u64 {
        self.end_time_unix_nano
            .saturating_sub(self.start_time_unix_nano)
            / 1_000_000
    }
}

#[derive(Debug)]
struct ExportQueue {
    capacity: usize,
    queue: VecDeque<ExportRecord>,
    dropped: AtomicU64,
    exported_ok: AtomicU64,
    exported_err: AtomicU64,
}

impl ExportQueue {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(16),
            queue: VecDeque::with_capacity(capacity.max(16)),
            dropped: AtomicU64::new(0),
            exported_ok: AtomicU64::new(0),
            exported_err: AtomicU64::new(0),
        }
    }

    fn try_push(&mut self, record: ExportRecord) -> Result<(), ()> {
        if self.queue.len() >= self.capacity {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return Err(());
        }
        self.queue.push_back(record);
        Ok(())
    }

    fn drain(&mut self, max: usize) -> Vec<ExportRecord> {
        let n = max.min(self.queue.len());
        self.queue.drain(..n).collect()
    }
}

static METRICS_ENABLED: AtomicBool = AtomicBool::new(true);
static EXPORT_NETWORK: AtomicBool = AtomicBool::new(false);
static SAMPLE_RATIO_MILLI: AtomicU64 = AtomicU64::new(1000);
static OTLP_ENDPOINT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static SERVICE_NAME: OnceLock<Mutex<String>> = OnceLock::new();
static FLUSHER_STARTED: AtomicBool = AtomicBool::new(false);
static SHUTDOWN: AtomicBool = AtomicBool::new(false);
static RETRY_BUDGET: AtomicU64 = AtomicU64::new(0);

/// Process-wide metrics registry + bounded export queue.
pub struct MetricsRegistry {
    inner: Mutex<RegistryInner>,
    export: Mutex<ExportQueue>,
}

impl MetricsRegistry {
    fn global() -> &'static MetricsRegistry {
        static REGISTRY: OnceLock<MetricsRegistry> = OnceLock::new();
        REGISTRY.get_or_init(|| MetricsRegistry {
            inner: Mutex::new(RegistryInner::default()),
            export: Mutex::new(ExportQueue::new(
                TelemetryConfig::DEFAULT_EXPORT_QUEUE_CAPACITY,
            )),
        })
    }

    /// Apply process telemetry config (metrics gate + exporter queue/endpoint).
    pub fn configure(config: &TelemetryConfig) {
        METRICS_ENABLED.store(config.metrics_enabled, Ordering::SeqCst);
        EXPORT_NETWORK.store(config.exporter_enabled(), Ordering::SeqCst);
        SAMPLE_RATIO_MILLI.store(u64::from(config.sample_ratio_milli), Ordering::SeqCst);
        SHUTDOWN.store(false, Ordering::SeqCst);
        *OTLP_ENDPOINT
            .get_or_init(|| Mutex::new(None))
            .lock()
            .expect("otlp endpoint lock") = config.otlp_endpoint.clone();
        *SERVICE_NAME
            .get_or_init(|| Mutex::new("markhand".into()))
            .lock()
            .expect("service name lock") = config.service_name.clone();
        let mut guard = Self::global().export.lock().expect("export lock");
        // Preserve dropped counters across resize; drop queued records on shrink.
        let dropped = guard.dropped.load(Ordering::Relaxed);
        let ok = guard.exported_ok.load(Ordering::Relaxed);
        let err = guard.exported_err.load(Ordering::Relaxed);
        let mut next = ExportQueue::new(config.export_queue_capacity);
        next.dropped.store(dropped, Ordering::Relaxed);
        next.exported_ok.store(ok, Ordering::Relaxed);
        next.exported_err.store(err, Ordering::Relaxed);
        while let Some(record) = guard.queue.pop_front() {
            let _ = next.try_push(record);
        }
        *guard = next;
        if config.exporter_enabled() {
            start_background_flusher();
        }
    }

    /// Reset all series (integration tests only).
    pub fn reset_for_tests() {
        let mut guard = Self::global().inner.lock().expect("metrics lock");
        *guard = RegistryInner::default();
        let mut export = Self::global().export.lock().expect("export lock");
        *export = ExportQueue::new(TelemetryConfig::DEFAULT_EXPORT_QUEUE_CAPACITY);
        METRICS_ENABLED.store(true, Ordering::SeqCst);
        EXPORT_NETWORK.store(false, Ordering::SeqCst);
    }

    pub fn metrics_enabled() -> bool {
        METRICS_ENABLED.load(Ordering::SeqCst)
    }

    pub fn add_counter(name: &'static str, labels: &[(&str, &str)], by: u64) {
        if !Self::metrics_enabled() {
            return;
        }
        if validate_metric(name, &labels.iter().map(|(k, _)| *k).collect::<Vec<_>>()).is_err() {
            return;
        }
        let key = normalize_labels(labels);
        let mut guard = Self::global().inner.lock().expect("metrics lock");
        let series = guard.counters.entry(name).or_default();
        let key = bounded_key(&mut series.values, key, 0u64);
        let entry = series.values.entry(key).or_insert(0);
        *entry = entry.saturating_add(by);
    }

    pub fn set_gauge(name: &'static str, labels: &[(&str, &str)], value: f64) {
        if !Self::metrics_enabled() {
            return;
        }
        if validate_metric(name, &labels.iter().map(|(k, _)| *k).collect::<Vec<_>>()).is_err() {
            return;
        }
        let key = normalize_labels(labels);
        let mut guard = Self::global().inner.lock().expect("metrics lock");
        let series = guard.gauges.entry(name).or_default();
        let key = bounded_key(&mut series.values, key, 0.0f64);
        series.values.insert(key, value);
    }

    pub fn observe_histogram(name: &'static str, labels: &[(&str, &str)], value_seconds: f64) {
        if !Self::metrics_enabled() {
            return;
        }
        if validate_metric(name, &labels.iter().map(|(k, _)| *k).collect::<Vec<_>>()).is_err() {
            return;
        }
        let key = normalize_labels(labels);
        let mut guard = Self::global().inner.lock().expect("metrics lock");
        let series = guard.histograms.entry(name).or_default();
        let key = bounded_key(&mut series.values, key, HistogramState::default());
        let state = series.values.entry(key).or_default();
        state.count = state.count.saturating_add(1);
        state.sum += value_seconds;
        for (idx, edge) in LATENCY_BUCKETS_SECONDS.iter().enumerate() {
            if value_seconds <= *edge {
                state.buckets[idx] = state.buckets[idx].saturating_add(1);
            }
        }
        let inf = state.buckets.len() - 1;
        state.buckets[inf] = state.buckets[inf].saturating_add(1);
    }

    /// Enqueue a span/event for optional OTLP export (bounded; may drop).
    pub fn enqueue_export(record: ExportRecord) -> bool {
        // Sampling (parts-per-thousand). Always keep errors for debugging? No —
        // cardinality-safe: sample uniformly on trace_id hash.
        let ratio = SAMPLE_RATIO_MILLI.load(Ordering::SeqCst);
        if ratio < 1000 {
            let hash = trace_sample_hash(&record.trace_id);
            if hash >= ratio {
                return false;
            }
        }
        // Never put secrets/content into export records — caller responsibility.
        // Lock order: export only (never hold export while taking metrics).
        let pushed = {
            let mut guard = Self::global().export.lock().expect("export lock");
            guard.try_push(record).is_ok()
        };
        // Dropped count is owned by ExportQueue::dropped and rendered as the
        // single `markhand_exporter_dropped_total` series (no duplicate counter).
        let _ = pushed;
        pushed
    }

    pub fn export_queue_len() -> usize {
        Self::global()
            .export
            .lock()
            .expect("export lock")
            .queue
            .len()
    }

    pub fn export_dropped() -> u64 {
        Self::global()
            .export
            .lock()
            .expect("export lock")
            .dropped
            .load(Ordering::Relaxed)
    }

    /// Drain up to `max` records (tests / sync flush helpers).
    pub fn drain_export_for_tests(max: usize) -> Vec<ExportRecord> {
        Self::global()
            .export
            .lock()
            .expect("export lock")
            .drain(max)
    }

    /// Attempt one export batch. When network export is disabled, drains to
    /// "ok" local sink (records still leave the queue — no unbounded growth).
    pub async fn flush_export_batch(max: usize) -> Result<usize, String> {
        Self::flush_export_batch_within(max, Duration::from_secs(5)).await
    }

    /// Flush one batch; each HTTP attempt is bounded by `http_timeout`.
    pub async fn flush_export_batch_within(
        max: usize,
        http_timeout: Duration,
    ) -> Result<usize, String> {
        let batch = {
            let mut guard = Self::global().export.lock().expect("export lock");
            guard.drain(max)
        };
        if batch.is_empty() {
            return Ok(0);
        }
        let count = batch.len();
        if !EXPORT_NETWORK.load(Ordering::SeqCst) {
            {
                let guard = Self::global().export.lock().expect("export lock");
                guard.exported_ok.fetch_add(count as u64, Ordering::Relaxed);
            }
            Self::add_counter(
                METRIC_EXPORTER_EXPORT,
                &[("outcome", "local")],
                count as u64,
            );
            return Ok(count);
        }
        let endpoint = OTLP_ENDPOINT
            .get_or_init(|| Mutex::new(None))
            .lock()
            .expect("otlp endpoint lock")
            .clone();
        let Some(endpoint) = endpoint else {
            {
                let guard = Self::global().export.lock().expect("export lock");
                guard
                    .exported_err
                    .fetch_add(count as u64, Ordering::Relaxed);
            }
            Self::add_counter(
                METRIC_EXPORTER_EXPORT,
                &[("outcome", "error")],
                count as u64,
            );
            return Err("otlp endpoint missing".into());
        };
        let http_timeout = if http_timeout.is_zero() {
            Duration::from_millis(1)
        } else {
            http_timeout
        };
        match post_otlp_traces_within(&endpoint, &batch, http_timeout).await {
            Ok(()) => {
                {
                    let guard = Self::global().export.lock().expect("export lock");
                    guard.exported_ok.fetch_add(count as u64, Ordering::Relaxed);
                }
                Self::add_counter(METRIC_EXPORTER_EXPORT, &[("outcome", "ok")], count as u64);
                Ok(count)
            }
            Err(error) => {
                // Bounded retry: re-queue only while under retry budget and capacity.
                let mut requeued = 0usize;
                {
                    let mut guard = Self::global().export.lock().expect("export lock");
                    guard
                        .exported_err
                        .fetch_add(count as u64, Ordering::Relaxed);
                    for record in batch.into_iter().rev() {
                        if RETRY_BUDGET.load(Ordering::Relaxed) == 0 {
                            break;
                        }
                        if guard.try_push(record).is_ok() {
                            RETRY_BUDGET.fetch_sub(1, Ordering::Relaxed);
                            requeued += 1;
                        }
                    }
                }
                Self::add_counter(
                    METRIC_EXPORTER_EXPORT,
                    &[("outcome", "error")],
                    count as u64,
                );
                if requeued > 0 {
                    Self::add_counter(
                        METRIC_EXPORTER_EXPORT,
                        &[("outcome", "requeued")],
                        requeued as u64,
                    );
                }
                Err(error)
            }
        }
    }

    /// Drain remaining export queue during process shutdown.
    ///
    /// Each HTTP attempt is bounded by the **remaining** deadline so a blackhole
    /// collector cannot stall past `timeout`.
    pub async fn shutdown_flush(timeout: Duration) -> usize {
        SHUTDOWN.store(true, Ordering::SeqCst);
        let deadline = std::time::Instant::now() + timeout;
        let mut total = 0usize;
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match Self::flush_export_batch_within(64, remaining).await {
                Ok(0) => break,
                Ok(n) => total += n,
                Err(_) => {
                    let sleep_for = remaining.min(Duration::from_millis(25));
                    if sleep_for.is_zero() {
                        break;
                    }
                    tokio::time::sleep(sleep_for).await;
                }
            }
        }
        total
    }

    pub fn render_prometheus() -> String {
        // Lock order: export briefly for gauges, drop, then metrics — never nest.
        let (queue_depth, dropped) = {
            let export = Self::global().export.lock().expect("export lock");
            (export.queue.len(), export.dropped.load(Ordering::Relaxed))
        };
        let guard = Self::global().inner.lock().expect("metrics lock");
        let mut out = String::new();
        out.push_str("# HELP markhand_metrics_build Phase 1B bounded metrics exporter\n");
        out.push_str("# TYPE markhand_metrics_build gauge\n");
        out.push_str("markhand_metrics_build{component=\"api\"} 1\n");
        out.push_str("# HELP markhand_exporter_queue_depth Bounded OTLP export queue depth\n");
        out.push_str("# TYPE markhand_exporter_queue_depth gauge\n");
        out.push_str(&format!(
            "markhand_exporter_queue_depth{{}} {queue_depth}\n"
        ));
        out.push_str("# TYPE markhand_exporter_dropped_total counter\n");
        out.push_str(&format!(
            "markhand_exporter_dropped_total{{reason=\"queue_full\"}} {dropped}\n"
        ));

        if !Self::metrics_enabled() {
            return out;
        }

        for (name, series) in &guard.counters {
            // Dropped total is emitted once above from the export queue atomic.
            if *name == METRIC_EXPORTER_DROPPED {
                continue;
            }
            out.push_str(&format!("# TYPE {name} counter\n"));
            for (labels, value) in &series.values {
                out.push_str(&format!("{name}{{{}}} {value}\n", format_labels(labels)));
            }
        }
        for (name, series) in &guard.gauges {
            out.push_str(&format!("# TYPE {name} gauge\n"));
            for (labels, value) in &series.values {
                out.push_str(&format!("{name}{{{}}} {value}\n", format_labels(labels)));
            }
        }
        for (name, series) in &guard.histograms {
            out.push_str(&format!("# TYPE {name} histogram\n"));
            for (labels, state) in &series.values {
                let base = format_labels(labels);
                for (idx, edge) in LATENCY_BUCKETS_SECONDS.iter().enumerate() {
                    let label = if base.is_empty() {
                        format!("le=\"{edge}\"")
                    } else {
                        format!("{base},le=\"{edge}\"")
                    };
                    out.push_str(&format!(
                        "{name}_bucket{{{label}}} {}\n",
                        state.buckets[idx]
                    ));
                }
                let inf_label = if base.is_empty() {
                    "le=\"+Inf\"".to_string()
                } else {
                    format!("{base},le=\"+Inf\"")
                };
                out.push_str(&format!(
                    "{name}_bucket{{{inf_label}}} {}\n",
                    state.buckets[state.buckets.len() - 1]
                ));
                out.push_str(&format!("{name}_sum{{{base}}} {}\n", state.sum));
                out.push_str(&format!("{name}_count{{{base}}} {}\n", state.count));
            }
        }
        out
    }
}

fn bounded_key<T: Clone>(
    entries: &mut BTreeMap<Vec<(String, String)>, T>,
    labels: Vec<(String, String)>,
    _default: T,
) -> Vec<(String, String)> {
    if entries.contains_key(&labels) || entries.len() < LABEL_SET_CAP.saturating_sub(1) {
        return labels;
    }
    labels
        .into_iter()
        .map(|(name, _)| (name, "other".into()))
        .collect()
}

fn normalize_labels(labels: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = labels
        .iter()
        .map(|(k, v)| ((*k).to_string(), sanitize_label_value(v)))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn trace_sample_hash(trace_id: &str) -> u64 {
    let mut acc = 0u64;
    for b in trace_id.bytes().take(16) {
        acc = acc.wrapping_mul(31).wrapping_add(u64::from(b));
    }
    acc % 1000
}

fn mint_span_id_hex() -> String {
    let mut id = format!(
        "{:016x}",
        uuid::Uuid::new_v4().as_u128() & 0xffff_ffff_ffff_ffff
    );
    if id.bytes().all(|b| b == b'0') {
        id = "00f067aa0ba902b7".into();
    }
    id
}

fn unix_time_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn start_background_flusher() {
    if FLUSHER_STARTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    RETRY_BUDGET.store(256, Ordering::Relaxed);
    tokio::spawn(async move {
        let mut attempt: u32 = 0;
        loop {
            if SHUTDOWN.load(Ordering::SeqCst) {
                let _ = MetricsRegistry::flush_export_batch(128).await;
                break;
            }
            match MetricsRegistry::flush_export_batch(64).await {
                Ok(0) => {
                    attempt = 0;
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                Ok(_) => {
                    attempt = 0;
                    let cur = RETRY_BUDGET.load(Ordering::Relaxed);
                    if cur < 256 {
                        RETRY_BUDGET.store((cur + 16).min(256), Ordering::Relaxed);
                    }
                }
                Err(_) => {
                    attempt = attempt.saturating_add(1).min(6);
                    let base_ms = 50u64 << attempt.min(5);
                    let jitter = unix_time_nanos() % 37;
                    tokio::time::sleep(Duration::from_millis(base_ms + jitter)).await;
                }
            }
        }
        FLUSHER_STARTED.store(false, Ordering::SeqCst);
    });
}

fn sanitize_label_value(value: &str) -> String {
    let trimmed = value.trim();
    let mut out = String::new();
    for ch in trimmed.chars().take(64) {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '/' | '.') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "unknown".into()
    } else {
        out
    }
}

fn format_labels(labels: &[(String, String)]) -> String {
    labels
        .iter()
        .map(|(k, v)| format!("{k}=\"{v}\""))
        .collect::<Vec<_>>()
        .join(",")
}

/// Exact OTLP/HTTP JSON encode used by the exporter (testable without network).
pub(crate) fn encode_otlp_traces_json(service: &str, batch: &[ExportRecord]) -> JsonValue {
    let spans: Vec<JsonValue> = batch
        .iter()
        .map(|record| {
            let kind = otlp_span_kind::to_otlp_enum(&record.span_kind);
            let mut span = json!({
                "traceId": record.trace_id,
                "spanId": record.span_id,
                "name": record.name,
                "kind": kind,
                "startTimeUnixNano": record.start_time_unix_nano.to_string(),
                "endTimeUnixNano": record.end_time_unix_nano.to_string(),
                "attributes": [
                    {"key": "request_id", "value": {"stringValue": record.request_id}},
                    {"key": "span_kind", "value": {"stringValue": record.span_kind}},
                    {"key": "outcome", "value": {"stringValue": record.outcome}},
                ]
            });
            if let Some(parent) = record.parent_span_id.as_deref() {
                span["parentSpanId"] = json!(parent);
            }
            span
        })
        .collect();
    json!({
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    {"key": "service.name", "value": {"stringValue": service}}
                ]
            },
            "scopeSpans": [{
                "spans": spans
            }]
        }]
    })
}

async fn post_otlp_traces_within(
    endpoint: &str,
    batch: &[ExportRecord],
    http_timeout: Duration,
) -> Result<(), String> {
    let service = SERVICE_NAME
        .get_or_init(|| Mutex::new("markhand".into()))
        .lock()
        .expect("service name lock")
        .clone();
    let body = encode_otlp_traces_json(&service, batch);
    let url = if endpoint.contains("/v1/traces") {
        endpoint.to_string()
    } else {
        format!("{}/v1/traces", endpoint.trim_end_matches('/'))
    };
    let client = reqwest::Client::builder()
        .timeout(http_timeout)
        .connect_timeout(http_timeout)
        .build()
        .map_err(|error| format!("otlp client: {error}"))?;
    let response = client
        .post(&url)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("otlp export failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("otlp export HTTP {}", response.status()));
    }
    Ok(())
}

/// Record one HTTP request observation + export span.
pub fn record_http_request(route: &str, status_class: &str, elapsed: Duration) {
    MetricsRegistry::observe_histogram(
        METRIC_REQUEST_LATENCY,
        &[("route", route), ("status", status_class)],
        elapsed.as_secs_f64(),
    );
}

pub fn set_queue_depth(job_type: &str, depth: f64) {
    MetricsRegistry::set_gauge(METRIC_QUEUE_DEPTH, &[("job_type", job_type)], depth);
}

pub fn set_queue_age_seconds(job_type: &str, age: f64) {
    MetricsRegistry::set_gauge(METRIC_QUEUE_AGE, &[("job_type", job_type)], age);
}

pub fn record_conversion(outcome: &str, elapsed: Duration) {
    MetricsRegistry::observe_histogram(
        METRIC_CONVERSION,
        &[("outcome", outcome)],
        elapsed.as_secs_f64(),
    );
}

pub fn record_embedding_batch(outcome: &str, elapsed: Duration) {
    MetricsRegistry::observe_histogram(
        METRIC_EMBEDDING_BATCH,
        &[("outcome", outcome)],
        elapsed.as_secs_f64(),
    );
}

pub fn record_retrieval_leg(leg: &str, outcome: &str, elapsed: Duration) {
    MetricsRegistry::observe_histogram(
        METRIC_RETRIEVAL_LEG,
        &[("leg", leg), ("outcome", outcome)],
        elapsed.as_secs_f64(),
    );
}

pub fn record_provider_call(provider: &str, outcome: &str, elapsed: Duration) {
    MetricsRegistry::observe_histogram(
        "markhand_provider_duration_seconds",
        &[("provider", provider), ("outcome", outcome)],
        elapsed.as_secs_f64(),
    );
}

pub fn inc_drift(kind: &str) {
    MetricsRegistry::add_counter(METRIC_DRIFT, &[("kind", kind)], 1);
}

pub fn inc_quota(outcome: &str) {
    MetricsRegistry::add_counter(METRIC_QUOTA, &[("outcome", outcome)], 1);
}

pub fn set_backup_age_seconds(store: &str, age: f64) {
    MetricsRegistry::set_gauge(METRIC_BACKUP_AGE, &[("store", store)], age);
}

/// Canonical OTLP span kind wire names (decoded to numeric enums on export).
pub mod otlp_span_kind {
    pub const INTERNAL: &str = "INTERNAL";
    pub const SERVER: &str = "SERVER";
    pub const CLIENT: &str = "CLIENT";
    pub const PRODUCER: &str = "PRODUCER";
    pub const CONSUMER: &str = "CONSUMER";

    /// Official OTLP protobuf SpanKind values:
    /// UNSPECIFIED=0, INTERNAL=1, SERVER=2, CLIENT=3, PRODUCER=4, CONSUMER=5.
    pub fn to_otlp_enum(kind: &str) -> i32 {
        match kind.trim().to_ascii_uppercase().as_str() {
            "INTERNAL" => 1,
            "SERVER" => 2,
            "CLIENT" => 3,
            "PRODUCER" => 4,
            "CONSUMER" => 5,
            _ => 1, // default INTERNAL (never emit UNSPECIFIED=0)
        }
    }

    pub fn normalize(kind: &str) -> &'static str {
        match kind.trim().to_ascii_uppercase().as_str() {
            "SERVER" => SERVER,
            "CLIENT" => CLIENT,
            "PRODUCER" => PRODUCER,
            "CONSUMER" => CONSUMER,
            _ => INTERNAL,
        }
    }
}

/// Start a real span lifecycle under the current correlation context.
///
/// Mints a unique span id, installs it as the task-local current span (so local
/// children parent to it), and returns a guard that exports on [`SpanGuard::end`].
/// Does **not** invent sequential nesting without an exported parent.
pub fn start_span(name: &str, span_kind: &str) -> Option<SpanGuard> {
    let corr = crate::telemetry::CorrelationContext::current()?;
    let span_id = mint_span_id_hex();
    let parent_span_id = Some(corr.span_id.clone());
    let previous = corr.clone();
    let _ = CURRENT.try_with(|guard| {
        if let Ok(mut current) = guard.lock() {
            current.parent_span_id = parent_span_id.clone();
            current.span_id = span_id.clone();
            let flags = current
                .traceparent
                .as_deref()
                .and_then(|tp| tp.split('-').nth(3))
                .unwrap_or("01");
            current.traceparent = Some(format!("00-{}-{}-{flags}", current.trace_id, span_id));
        }
    });
    Some(SpanGuard {
        name: sanitize_label_value(name),
        request_id: sanitize_label_value(&corr.request_id),
        trace_id: sanitize_label_value(&corr.trace_id),
        span_id,
        parent_span_id,
        span_kind: otlp_span_kind::normalize(span_kind).to_string(),
        start_time_unix_nano: unix_time_nanos(),
        previous,
        ended: false,
    })
}

/// RAII span handle — call [`end`](SpanGuard::end) to export (Drop is best-effort).
pub struct SpanGuard {
    name: String,
    request_id: String,
    trace_id: String,
    span_id: String,
    parent_span_id: Option<String>,
    span_kind: String,
    start_time_unix_nano: u64,
    previous: crate::telemetry::CorrelationContext,
    ended: bool,
}

impl SpanGuard {
    pub fn span_id(&self) -> &str {
        &self.span_id
    }

    pub fn end(mut self, outcome: &str) {
        if self.ended {
            return;
        }
        self.ended = true;
        self.export(outcome);
        let previous = self.previous.clone();
        let _ = CURRENT.try_with(|guard| {
            if let Ok(mut current) = guard.lock() {
                *current = previous;
            }
        });
    }

    fn export(&self, outcome: &str) {
        let end = unix_time_nanos();
        let _ = MetricsRegistry::enqueue_export(ExportRecord {
            name: self.name.clone(),
            request_id: self.request_id.clone(),
            trace_id: self.trace_id.clone(),
            span_id: self.span_id.clone(),
            parent_span_id: self.parent_span_id.clone(),
            span_kind: self.span_kind.clone(),
            outcome: sanitize_label_value(outcome),
            start_time_unix_nano: self.start_time_unix_nano,
            end_time_unix_nano: end,
        });
    }
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        if !self.ended {
            self.export("error");
            let _ = CURRENT.try_with(|guard| {
                if let Ok(mut current) = guard.lock() {
                    *current = self.previous.clone();
                }
            });
        }
    }
}

/// Complete the **current** task-local span (exports its span id — no new mint).
///
/// Use for API SERVER / worker CONSUMER roots so nested children parent to an
/// id that is actually exported (no placeholder/unexported parent).
pub fn complete_current_span(name: &str, span_kind: &str, outcome: &str, duration: Duration) {
    let Some(corr) = crate::telemetry::CorrelationContext::current() else {
        return;
    };
    let end = unix_time_nanos();
    let start = end.saturating_sub(duration.as_nanos() as u64);
    let _ = MetricsRegistry::enqueue_export(ExportRecord {
        name: sanitize_label_value(name),
        request_id: sanitize_label_value(&corr.request_id),
        trace_id: sanitize_label_value(&corr.trace_id),
        span_id: corr.span_id.clone(),
        parent_span_id: corr.parent_span_id.clone(),
        span_kind: otlp_span_kind::normalize(span_kind).to_string(),
        outcome: sanitize_label_value(outcome),
        start_time_unix_nano: start,
        end_time_unix_nano: end,
    });
}

/// Emit a completed leaf span with a freshly minted id under the current parent.
///
/// Prefer [`complete_current_span`] for request/job roots and [`start_span`] for
/// nested local work. Does **not** artificially rewrite the current span id.
pub fn emit_span(
    name: &str,
    request_id: &str,
    trace_id: &str,
    span_kind: &str,
    outcome: &str,
    duration: Duration,
) {
    let corr = crate::telemetry::CorrelationContext::current();
    let parent_span_id = corr.as_ref().map(|c| c.span_id.clone());
    let span_id = mint_span_id_hex();
    let end = unix_time_nanos();
    let start = end.saturating_sub(duration.as_nanos() as u64);
    let kind = otlp_span_kind::normalize(span_kind);
    let _ = MetricsRegistry::enqueue_export(ExportRecord {
        name: sanitize_label_value(name),
        request_id: sanitize_label_value(request_id),
        trace_id: sanitize_label_value(trace_id),
        span_id,
        parent_span_id,
        span_kind: kind.to_string(),
        outcome: sanitize_label_value(outcome),
        start_time_unix_nano: start,
        end_time_unix_nano: end,
    });
}

/// Decode OTLP/HTTP JSON (protobuf-equivalent field shape) and assert:
/// - unique spanIds
/// - canonical kinds INTERNAL=1 .. CONSUMER=5
/// - every parentSpanId that is **local** (appears nowhere as a remote-only
///   root) is exported as a spanId in the same batch
pub fn assert_otlp_batch_parent_graph(body: &JsonValue) -> Result<(), String> {
    let spans = body
        .pointer("/resourceSpans/0/scopeSpans/0/spans")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "otlp body missing spans array".to_string())?;
    let mut ids = std::collections::BTreeSet::new();
    let mut remote_roots = std::collections::BTreeSet::new();
    for span in spans {
        let id = span
            .get("spanId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "span missing spanId".to_string())?;
        if !ids.insert(id.to_string()) {
            return Err(format!("duplicate spanId {id}"));
        }
        let kind = span
            .get("kind")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| "span missing numeric kind".to_string())?;
        if !(1..=5).contains(&kind) {
            return Err(format!(
                "non-canonical otlp kind {kind} (want INTERNAL=1..CONSUMER=5)"
            ));
        }
        // SERVER/CONSUMER with a parent not in-batch is treated as remote root parent.
        if matches!(kind, 2 | 5) {
            if let Some(parent) = span.get("parentSpanId").and_then(|v| v.as_str()) {
                remote_roots.insert(parent.to_string());
            }
        }
    }
    for span in spans {
        let id = span.get("spanId").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(parent) = span.get("parentSpanId").and_then(|v| v.as_str()) {
            if parent == id {
                return Err("span parentSpanId must not equal spanId".into());
            }
            if ids.contains(parent) {
                continue; // local parent exported
            }
            if remote_roots.contains(parent) {
                continue; // remote W3C parent of SERVER/CONSUMER root
            }
            return Err(format!(
                "local parentSpanId {parent} not exported in batch (unexported/placeholder parent)"
            ));
        }
    }
    Ok(())
}

/// Helper for tests/tabletops to inject a known latency sample.
pub fn inject_latency_for_tests(route: &str, seconds: f64) {
    MetricsRegistry::observe_histogram(
        METRIC_REQUEST_LATENCY,
        &[("route", route), ("status", "5xx")],
        seconds,
    );
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)] // test_lock serializes process-wide registry across async tests
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn exporter_emits_canonical_metric_without_forbidden_labels() {
        let _guard = test_lock();
        MetricsRegistry::reset_for_tests();
        record_http_request("health.live", "2xx", Duration::from_millis(12));
        set_queue_depth("convert", 3.0);
        inc_drift("orphan_vector");
        let body = MetricsRegistry::render_prometheus();
        assert!(
            body.contains(METRIC_REQUEST_LATENCY),
            "missing latency metric in body:\n{body}"
        );
        assert!(body.contains(METRIC_QUEUE_DEPTH));
        assert!(body.contains(METRIC_DRIFT));
        assert!(body.contains("markhand_exporter_queue_depth"));
        assert!(!body.contains("org_id"));
        assert!(!body.contains("request_id=\""));
        assert!(!body.contains("document_id"));
        assert!(!body.contains("canary"));
        assert!(!body.contains("password"));
    }

    #[test]
    fn forbidden_label_observations_are_dropped() {
        let _guard = test_lock();
        MetricsRegistry::reset_for_tests();
        MetricsRegistry::add_counter(METRIC_QUOTA, &[("org_id", "org-1")], 1);
        let body = MetricsRegistry::render_prometheus();
        assert!(!body.contains("org_id=\"org-1\""));
    }

    #[test]
    fn export_queue_applies_backpressure_when_full() {
        let _guard = test_lock();
        MetricsRegistry::reset_for_tests();
        MetricsRegistry::configure(&TelemetryConfig {
            export_queue_capacity: 16,
            ..TelemetryConfig::disabled()
        });
        for i in 0..16 {
            assert!(MetricsRegistry::enqueue_export(ExportRecord {
                name: "span".into(),
                request_id: format!("req-{i}"),
                trace_id: format!("{i:032x}"),
                span_id: format!("{i:016x}"),
                parent_span_id: None,
                span_kind: "internal".into(),
                outcome: "ok".into(),
                start_time_unix_nano: 1_000_000_000,
                end_time_unix_nano: 1_001_000_000,
            }));
        }
        assert!(!MetricsRegistry::enqueue_export(ExportRecord {
            name: "span".into(),
            request_id: "overflow".into(),
            trace_id: "ffffffffffffffffffffffffffffffff".into(),
            span_id: "00f067aa0ba902b7".into(),
            parent_span_id: None,
            span_kind: "internal".into(),
            outcome: "ok".into(),
            start_time_unix_nano: 1_000_000_000,
            end_time_unix_nano: 1_001_000_000,
        }));
        assert_eq!(MetricsRegistry::export_dropped(), 1);
        assert_eq!(MetricsRegistry::export_queue_len(), 16);
    }

    #[tokio::test]
    async fn local_flush_drains_queue_without_network() {
        {
            let _guard = test_lock();
            MetricsRegistry::reset_for_tests();
            MetricsRegistry::configure(&TelemetryConfig::disabled());
            assert!(MetricsRegistry::enqueue_export(ExportRecord {
                name: "api.request".into(),
                request_id: "550e8400-e29b-41d4-a716-446655440000".into(),
                trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
                span_id: "00f067aa0ba902b7".into(),
                parent_span_id: None,
                span_kind: "server".into(),
                outcome: "ok".into(),
                start_time_unix_nano: 1_000_000_000,
                end_time_unix_nano: 1_001_000_000,
            }));
        }
        let n = MetricsRegistry::flush_export_batch(10).await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(MetricsRegistry::export_queue_len(), 0);
    }

    #[test]
    fn label_cardinality_caps_with_other_bucket() {
        let _guard = test_lock();
        MetricsRegistry::reset_for_tests();
        for i in 0..140 {
            record_http_request(&format!("route.{i}"), "2xx", Duration::from_millis(1));
        }
        let body = MetricsRegistry::render_prometheus();
        assert!(body.contains("route=\"other\"") || body.contains("status=\"other\""));
    }
    #[tokio::test]
    async fn mock_collector_export_outage_recovery_and_shutdown() {
        let _guard = test_lock();
        MetricsRegistry::reset_for_tests();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = std::sync::Arc::new(AtomicU64::new(0));
        let fail_first = std::sync::Arc::new(AtomicBool::new(true));
        let hits_c = hits.clone();
        let fail_c = fail_first.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 8192];
                let _ = sock.read(&mut buf).await;
                hits_c.fetch_add(1, Ordering::SeqCst);
                if fail_c.swap(false, Ordering::SeqCst) {
                    let _ = sock
                        .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
                        .await;
                } else {
                    let _ = sock
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                        .await;
                }
            }
        });
        let endpoint = format!("http://{addr}");
        MetricsRegistry::configure(&TelemetryConfig {
            exporter: crate::telemetry::OtelExporterKind::Otlp,
            otlp_endpoint: Some(endpoint),
            disable_network: false,
            export_queue_capacity: 32,
            sample_ratio_milli: 1000,
            ..TelemetryConfig::disabled()
        });
        // Prevent background flusher racing test flushes.
        SHUTDOWN.store(true, Ordering::SeqCst);
        assert!(MetricsRegistry::enqueue_export(ExportRecord {
            name: "api.request".into(),
            request_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
            span_id: "00f067aa0ba902b7".into(),
            parent_span_id: Some("00f067aa0ba902b8".into()),
            span_kind: "server".into(),
            outcome: "ok".into(),
            start_time_unix_nano: 1_700_000_000_000_000_000,
            end_time_unix_nano: 1_700_000_000_005_000_000,
        }));
        assert!(MetricsRegistry::flush_export_batch(8).await.is_err());
        assert!(MetricsRegistry::flush_export_batch(8).await.is_ok());
        let _ = MetricsRegistry::shutdown_flush(Duration::from_millis(200)).await;
        assert!(hits.load(Ordering::SeqCst) >= 1);
    }

    #[test]
    fn otlp_encode_uses_canonical_kind_enums_and_parent_graph() {
        let body = encode_otlp_traces_json(
            "markhand-test",
            &[
                ExportRecord {
                    name: "api.request".into(),
                    request_id: "550e8400-e29b-41d4-a716-446655440000".into(),
                    trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
                    span_id: "00f067aa0ba902b7".into(),
                    parent_span_id: None,
                    span_kind: otlp_span_kind::SERVER.to_string(),
                    outcome: "ok".into(),
                    start_time_unix_nano: 1,
                    end_time_unix_nano: 2,
                },
                ExportRecord {
                    name: "job.convert".into(),
                    request_id: "550e8400-e29b-41d4-a716-446655440000".into(),
                    trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
                    span_id: "00f067aa0ba902b8".into(),
                    parent_span_id: Some("00f067aa0ba902b7".into()),
                    span_kind: otlp_span_kind::INTERNAL.to_string(),
                    outcome: "ok".into(),
                    start_time_unix_nano: 2,
                    end_time_unix_nano: 3,
                },
                ExportRecord {
                    name: "retrieval.embedding".into(),
                    request_id: "550e8400-e29b-41d4-a716-446655440000".into(),
                    trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
                    span_id: "00f067aa0ba902b9".into(),
                    parent_span_id: Some("00f067aa0ba902b8".into()),
                    span_kind: otlp_span_kind::CLIENT.to_string(),
                    outcome: "ok".into(),
                    start_time_unix_nano: 3,
                    end_time_unix_nano: 4,
                },
            ],
        );
        let spans = &body["resourceSpans"][0]["scopeSpans"][0]["spans"];
        // Official protobuf: INTERNAL=1 SERVER=2 CLIENT=3 PRODUCER=4 CONSUMER=5
        assert_eq!(spans[0]["kind"], 2); // SERVER
        assert_eq!(spans[1]["kind"], 1); // INTERNAL
        assert_eq!(spans[2]["kind"], 3); // CLIENT
        assert!(spans[0].get("parentSpanId").is_none());
        assert_eq!(spans[1]["parentSpanId"], "00f067aa0ba902b7");
        assert_eq!(spans[2]["parentSpanId"], "00f067aa0ba902b8");
        assert_otlp_batch_parent_graph(&body).expect("parent graph");
        let ids: Vec<&str> = spans
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["spanId"].as_str().unwrap())
            .collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(
            ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
            3
        );
    }

    #[tokio::test]
    async fn real_span_lifecycle_exports_local_parents_no_artificial_nesting() {
        let _guard = test_lock();
        MetricsRegistry::reset_for_tests();
        MetricsRegistry::configure(&TelemetryConfig::disabled());
        let ctx = crate::telemetry::CorrelationContext::new("550e8400-e29b-41d4-a716-446655440000");
        let root_span = ctx.span_id.clone();
        crate::telemetry::scope(ctx, async {
            // Leaf emit does NOT rewrite current span — both leaves parent to root.
            emit_span(
                "leaf.a",
                "550e8400-e29b-41d4-a716-446655440000",
                "4bf92f3577b34da6a3ce929d0e0e4736",
                otlp_span_kind::INTERNAL,
                "ok",
                Duration::from_millis(1),
            );
            emit_span(
                "leaf.b",
                "550e8400-e29b-41d4-a716-446655440000",
                "4bf92f3577b34da6a3ce929d0e0e4736",
                otlp_span_kind::CLIENT,
                "ok",
                Duration::from_millis(1),
            );
            // Nested local work uses start_span so the parent is exported.
            let nested = start_span("retrieval.hybrid", otlp_span_kind::INTERNAL).unwrap();
            emit_span(
                "retrieval.fts",
                "550e8400-e29b-41d4-a716-446655440000",
                "4bf92f3577b34da6a3ce929d0e0e4736",
                otlp_span_kind::INTERNAL,
                "ok",
                Duration::from_millis(1),
            );
            nested.end("ok");
            complete_current_span(
                "api.request",
                otlp_span_kind::SERVER,
                "ok",
                Duration::from_millis(5),
            );
        })
        .await;
        let drained = MetricsRegistry::drain_export_for_tests(16);
        assert!(drained.len() >= 5);
        // Leaf emits parent to root (no artificial sequential nesting).
        assert_eq!(
            drained[0].parent_span_id.as_deref(),
            Some(root_span.as_str())
        );
        assert_eq!(
            drained[1].parent_span_id.as_deref(),
            Some(root_span.as_str())
        );
        assert_ne!(drained[0].span_id, drained[1].span_id);
        // Nested child parents to the start_span id (exported).
        let nested_id = drained
            .iter()
            .find(|r| r.name == "retrieval.hybrid")
            .map(|r| r.span_id.clone())
            .expect("nested parent exported");
        let fts = drained.iter().find(|r| r.name == "retrieval.fts").unwrap();
        assert_eq!(fts.parent_span_id.as_deref(), Some(nested_id.as_str()));
        let server = drained.iter().find(|r| r.name == "api.request").unwrap();
        assert_eq!(server.span_id, root_span);
        assert_eq!(server.span_kind, otlp_span_kind::SERVER);
        let body = encode_otlp_traces_json("markhand-test", &drained);
        assert_otlp_batch_parent_graph(&body).expect("decoded otlp parent graph");
    }

    #[test]
    fn one_exporter_dropped_counter_series_and_queue_gauges_zero_reset() {
        let _guard = test_lock();
        MetricsRegistry::reset_for_tests();
        for job_type in [
            "convert",
            "index",
            "delete",
            "reconcile",
            "embedding_batch",
            "lifecycle_refresh",
        ] {
            set_queue_depth(job_type, 7.0);
            set_queue_age_seconds(job_type, 11.0);
        }
        // Simulate observe_queue_metrics empty-queue reset.
        for job_type in [
            "convert",
            "index",
            "delete",
            "reconcile",
            "embedding_batch",
            "lifecycle_refresh",
        ] {
            set_queue_depth(job_type, 0.0);
            set_queue_age_seconds(job_type, 0.0);
        }
        // Queue floor is 16 (ExportQueue::new); fill then overflow once.
        MetricsRegistry::configure(&TelemetryConfig {
            export_queue_capacity: 16,
            ..TelemetryConfig::disabled()
        });
        for i in 0..16 {
            assert!(MetricsRegistry::enqueue_export(ExportRecord {
                name: "span".into(),
                request_id: format!("r{i}"),
                trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
                span_id: format!("{:016x}", i + 1),
                parent_span_id: None,
                span_kind: "INTERNAL".into(),
                outcome: "ok".into(),
                start_time_unix_nano: 1,
                end_time_unix_nano: 2,
            }));
        }
        assert!(!MetricsRegistry::enqueue_export(ExportRecord {
            name: "span".into(),
            request_id: "overflow".into(),
            trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
            span_id: "00f067aa0ba902b8".into(),
            parent_span_id: None,
            span_kind: "INTERNAL".into(),
            outcome: "ok".into(),
            start_time_unix_nano: 1,
            end_time_unix_nano: 2,
        }));
        let body = MetricsRegistry::render_prometheus();
        let dropped_lines: Vec<&str> = body
            .lines()
            .filter(|line| line.starts_with(METRIC_EXPORTER_DROPPED) && !line.starts_with("#"))
            .collect();
        assert_eq!(
            dropped_lines.len(),
            1,
            "expected exactly one dropped counter series, got {dropped_lines:?}"
        );
        for job_type in [
            "convert",
            "index",
            "delete",
            "reconcile",
            "embedding_batch",
            "lifecycle_refresh",
        ] {
            assert!(
                body.contains(&format!("job_type=\"{job_type}\""))
                    && body.contains("markhand_job_queue_depth"),
                "missing zero-reset gauge for {job_type}"
            );
        }
        assert!(body.contains("markhand_job_queue_depth{job_type=\"convert\"} 0"));
    }

    #[test]
    fn separate_embedding_fts_vector_retrieval_timers() {
        let _guard = test_lock();
        MetricsRegistry::reset_for_tests();
        record_retrieval_leg("embedding", "ok", Duration::from_millis(5));
        record_retrieval_leg("fts", "ok", Duration::from_millis(7));
        record_retrieval_leg("vector", "ok", Duration::from_millis(9));
        let body = MetricsRegistry::render_prometheus();
        assert!(body.contains("leg=\"embedding\""));
        assert!(body.contains("leg=\"fts\""));
        assert!(body.contains("leg=\"vector\""));
        assert!(body.contains(METRIC_RETRIEVAL_LEG));
    }

    #[tokio::test]
    async fn mock_collector_receives_spans_on_shutdown_flush_sigterm_path() {
        let _guard = test_lock();
        MetricsRegistry::reset_for_tests();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let captured = std::sync::Arc::new(Mutex::new(Vec::<u8>::new()));
        let captured_c = captured.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let mut buf = vec![0u8; 65536];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            captured_c.lock().unwrap().extend_from_slice(&buf[..n]);
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                .await;
        });
        MetricsRegistry::configure(&TelemetryConfig {
            exporter: crate::telemetry::OtelExporterKind::Otlp,
            otlp_endpoint: Some(format!("http://{addr}")),
            disable_network: false,
            export_queue_capacity: 32,
            sample_ratio_milli: 1000,
            ..TelemetryConfig::disabled()
        });
        SHUTDOWN.store(false, Ordering::SeqCst);
        assert!(MetricsRegistry::enqueue_export(ExportRecord {
            name: "worker.job".into(),
            request_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
            span_id: "00f067aa0ba902b7".into(),
            parent_span_id: Some("00f067aa0ba902b6".into()),
            span_kind: otlp_span_kind::INTERNAL.to_string(),
            outcome: "ok".into(),
            start_time_unix_nano: 10,
            end_time_unix_nano: 20,
        }));
        // SIGTERM / Ctrl-C path: drain then shutdown_flush.
        let flushed = MetricsRegistry::shutdown_flush(Duration::from_secs(1)).await;
        assert_eq!(flushed, 1);
        assert_eq!(MetricsRegistry::export_queue_len(), 0);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let captured_bytes = captured.lock().unwrap().clone();
        let raw = String::from_utf8_lossy(&captured_bytes);
        assert!(
            raw.contains("4bf92f3577b34da6a3ce929d0e0e4736"),
            "collector missing trace id: {raw}"
        );
        assert!(
            raw.contains("\"kind\":1") || raw.contains("\"kind\": 1"),
            "want INTERNAL=1 in payload: {raw}"
        );
        assert!(raw.contains("parentSpanId"));
    }

    #[tokio::test]
    async fn shutdown_flush_bounds_each_http_attempt_against_blackhole_collector() {
        let _guard = test_lock();
        MetricsRegistry::reset_for_tests();
        // Bind but never accept — connect hangs until client timeout.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Keep listener alive (don't accept) for the duration of the test.
        let _keep = listener;
        MetricsRegistry::configure(&TelemetryConfig {
            exporter: crate::telemetry::OtelExporterKind::Otlp,
            otlp_endpoint: Some(format!("http://{addr}")),
            disable_network: false,
            export_queue_capacity: 32,
            sample_ratio_milli: 1000,
            ..TelemetryConfig::disabled()
        });
        SHUTDOWN.store(false, Ordering::SeqCst);
        assert!(MetricsRegistry::enqueue_export(ExportRecord {
            name: "worker.convert".into(),
            request_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
            span_id: "00f067aa0ba902b7".into(),
            parent_span_id: None,
            span_kind: otlp_span_kind::CONSUMER.to_string(),
            outcome: "ok".into(),
            start_time_unix_nano: 10,
            end_time_unix_nano: 20,
        }));
        let started = std::time::Instant::now();
        let _ = MetricsRegistry::shutdown_flush(Duration::from_millis(400)).await;
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "blackhole collector must not stall past remaining deadline (elapsed {elapsed:?})"
        );
    }

    #[test]
    fn otlp_kind_enums_match_official_protobuf() {
        assert_eq!(otlp_span_kind::to_otlp_enum("INTERNAL"), 1);
        assert_eq!(otlp_span_kind::to_otlp_enum("SERVER"), 2);
        assert_eq!(otlp_span_kind::to_otlp_enum("CLIENT"), 3);
        assert_eq!(otlp_span_kind::to_otlp_enum("PRODUCER"), 4);
        assert_eq!(otlp_span_kind::to_otlp_enum("CONSUMER"), 5);
    }
}
