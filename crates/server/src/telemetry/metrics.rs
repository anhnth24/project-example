use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::telemetry::validate_metric;

const LABEL_SET_CAP: usize = 128;
const HTTP_DURATION_BUCKETS: &[f64] = &[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0];
const JOB_DURATION_BUCKETS: &[f64] = &[0.1, 0.5, 1.0, 2.5, 5.0, 15.0, 30.0, 60.0, 300.0];
const LATENCY_BUCKETS: &[f64] = &[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0];

#[derive(Debug)]
pub struct MetricsRegistry {
    http_requests: CounterFamily,
    http_request_duration: HistogramFamily,
    jobs_processed: CounterFamily,
    jobs_duration: HistogramFamily,
    jobs_in_flight: AtomicI64,
    jobs_queue_depth: AtomicI64,
    retrieval_latency: HistogramFamily,
    embedding_latency: HistogramFamily,
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self::with_label_set_cap(LABEL_SET_CAP)
    }

    fn with_label_set_cap(label_set_cap: usize) -> Self {
        Self {
            http_requests: CounterFamily::new(
                "markhand_http_requests_total",
                "HTTP requests served by the Markhand API.",
                &["route", "method", "status"],
                label_set_cap,
            ),
            http_request_duration: HistogramFamily::new(
                "markhand_http_request_duration_seconds",
                "HTTP request duration in seconds.",
                &["route", "method", "status"],
                HTTP_DURATION_BUCKETS,
                label_set_cap,
            ),
            jobs_processed: CounterFamily::new(
                "markhand_jobs_processed_total",
                "Background jobs processed by workers.",
                &["job_type", "outcome"],
                label_set_cap,
            ),
            jobs_duration: HistogramFamily::new(
                "markhand_job_duration_seconds",
                "Background job processing duration in seconds.",
                &["job_type", "outcome"],
                JOB_DURATION_BUCKETS,
                label_set_cap,
            ),
            jobs_in_flight: AtomicI64::new(0),
            jobs_queue_depth: AtomicI64::new(0),
            retrieval_latency: HistogramFamily::new(
                "markhand_retrieval_latency_seconds",
                "Retrieval service latency in seconds.",
                &["stage", "outcome"],
                LATENCY_BUCKETS,
                label_set_cap,
            ),
            embedding_latency: HistogramFamily::new(
                "markhand_embedding_latency_seconds",
                "Embedding computation latency in seconds.",
                &["stage", "outcome"],
                LATENCY_BUCKETS,
                label_set_cap,
            ),
        }
    }

    pub fn record_http_request(
        &self,
        route: &str,
        method: &str,
        status: u16,
        duration_seconds: f64,
    ) {
        let status = status.to_string();
        let labels = [
            ("route", route),
            ("method", method),
            ("status", status.as_str()),
        ];
        let _ = self.http_requests.increment(&labels);
        let _ = self
            .http_request_duration
            .observe(&labels, duration_seconds);
    }

    pub fn record_job_processed(&self, job_type: &str, outcome: &str, duration_seconds: f64) {
        let labels = [("job_type", job_type), ("outcome", outcome)];
        let _ = self.jobs_processed.increment(&labels);
        let _ = self.jobs_duration.observe(&labels, duration_seconds);
    }

    pub fn increment_jobs_in_flight(&self) {
        self.jobs_in_flight.fetch_add(1, Ordering::Relaxed);
    }

    pub fn decrement_jobs_in_flight(&self) {
        self.jobs_in_flight.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn set_jobs_queue_depth(&self, value: i64) {
        self.jobs_queue_depth.store(value.max(0), Ordering::Relaxed);
    }

    pub fn observe_retrieval_latency(&self, stage: &str, outcome: &str, duration_seconds: f64) {
        let labels = [("stage", stage), ("outcome", outcome)];
        let _ = self.retrieval_latency.observe(&labels, duration_seconds);
    }

    pub fn observe_embedding_latency(&self, stage: &str, outcome: &str, duration_seconds: f64) {
        let labels = [("stage", stage), ("outcome", outcome)];
        let _ = self.embedding_latency.observe(&labels, duration_seconds);
    }

    pub fn render_prometheus(&self) -> String {
        let mut output = String::new();
        self.http_requests.render(&mut output);
        self.http_request_duration.render(&mut output);
        self.jobs_processed.render(&mut output);
        self.jobs_duration.render(&mut output);
        render_gauge(
            &mut output,
            "markhand_jobs_in_flight",
            "Background jobs currently being processed.",
            self.jobs_in_flight.load(Ordering::Relaxed),
        );
        render_gauge(
            &mut output,
            "markhand_jobs_queue_depth",
            "Background jobs waiting to be processed.",
            self.jobs_queue_depth.load(Ordering::Relaxed),
        );
        self.retrieval_latency.render(&mut output);
        self.embedding_latency.render(&mut output);
        output
    }
}

#[derive(Debug)]
struct CounterFamily {
    name: &'static str,
    help: &'static str,
    label_names: &'static [&'static str],
    label_set_cap: usize,
    entries: Mutex<BTreeMap<LabelSet, Arc<CounterEntry>>>,
}

impl CounterFamily {
    fn new(
        name: &'static str,
        help: &'static str,
        label_names: &'static [&'static str],
        label_set_cap: usize,
    ) -> Self {
        validate_metric(name, label_names).expect("static metric definition is valid");
        Self {
            name,
            help,
            label_names,
            label_set_cap,
            entries: Mutex::new(BTreeMap::new()),
        }
    }

    fn increment(&self, labels: &[(&str, &str)]) -> Result<(), String> {
        let entry = self.entry(labels)?;
        entry.value.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn entry(&self, labels: &[(&str, &str)]) -> Result<Arc<CounterEntry>, String> {
        let labels = self.label_set(labels)?;
        let mut entries = self.lock_entries();
        let key = bounded_key(&mut entries, labels, self.label_set_cap);
        Ok(entries
            .entry(key)
            .or_insert_with(|| Arc::new(CounterEntry::default()))
            .clone())
    }

    fn label_set(&self, labels: &[(&str, &str)]) -> Result<LabelSet, String> {
        label_set(self.name, self.label_names, labels)
    }

    fn lock_entries(&self) -> std::sync::MutexGuard<'_, BTreeMap<LabelSet, Arc<CounterEntry>>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn render(&self, output: &mut String) {
        render_header(output, self.name, self.help, "counter");
        for (labels, entry) in self.lock_entries().iter() {
            let _ = writeln!(
                output,
                "{}{} {}",
                self.name,
                render_labels(labels),
                entry.value.load(Ordering::Relaxed)
            );
        }
    }
}

#[derive(Debug)]
struct HistogramFamily {
    name: &'static str,
    help: &'static str,
    label_names: &'static [&'static str],
    buckets: &'static [f64],
    label_set_cap: usize,
    entries: Mutex<BTreeMap<LabelSet, Arc<HistogramEntry>>>,
}

impl HistogramFamily {
    fn new(
        name: &'static str,
        help: &'static str,
        label_names: &'static [&'static str],
        buckets: &'static [f64],
        label_set_cap: usize,
    ) -> Self {
        validate_metric(name, label_names).expect("static metric definition is valid");
        Self {
            name,
            help,
            label_names,
            buckets,
            label_set_cap,
            entries: Mutex::new(BTreeMap::new()),
        }
    }

    fn observe(&self, labels: &[(&str, &str)], value: f64) -> Result<(), String> {
        if !value.is_finite() || value < 0.0 {
            return Ok(());
        }
        let entry = self.entry(labels)?;
        if let Some(index) = self.buckets.iter().position(|bucket| value <= *bucket) {
            entry.buckets[index].fetch_add(1, Ordering::Relaxed);
        }
        entry.count.fetch_add(1, Ordering::Relaxed);
        entry
            .sum_micros
            .fetch_add((value * 1_000_000.0).round() as u64, Ordering::Relaxed);
        Ok(())
    }

    fn entry(&self, labels: &[(&str, &str)]) -> Result<Arc<HistogramEntry>, String> {
        let labels = label_set(self.name, self.label_names, labels)?;
        let mut entries = self.lock_entries();
        let key = bounded_key(&mut entries, labels, self.label_set_cap);
        Ok(entries
            .entry(key)
            .or_insert_with(|| Arc::new(HistogramEntry::new(self.buckets.len())))
            .clone())
    }

    fn lock_entries(&self) -> std::sync::MutexGuard<'_, BTreeMap<LabelSet, Arc<HistogramEntry>>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn render(&self, output: &mut String) {
        render_header(output, self.name, self.help, "histogram");
        for (labels, entry) in self.lock_entries().iter() {
            let mut cumulative = 0;
            for (index, bucket) in self.buckets.iter().enumerate() {
                cumulative += entry.buckets[index].load(Ordering::Relaxed);
                let mut bucket_labels = labels.clone();
                bucket_labels.push(("le".into(), format_float(*bucket)));
                let _ = writeln!(
                    output,
                    "{}_bucket{} {}",
                    self.name,
                    render_labels(&bucket_labels),
                    cumulative
                );
            }
            let count = entry.count.load(Ordering::Relaxed);
            let mut infinity_labels = labels.clone();
            infinity_labels.push(("le".into(), "+Inf".into()));
            let _ = writeln!(
                output,
                "{}_bucket{} {}",
                self.name,
                render_labels(&infinity_labels),
                count
            );
            let _ = writeln!(
                output,
                "{}_sum{} {:.6}",
                self.name,
                render_labels(labels),
                entry.sum_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0
            );
            let _ = writeln!(
                output,
                "{}_count{} {}",
                self.name,
                render_labels(labels),
                count
            );
        }
    }
}

type LabelSet = Vec<(String, String)>;

#[derive(Debug, Default)]
struct CounterEntry {
    value: AtomicU64,
}

#[derive(Debug)]
struct HistogramEntry {
    buckets: Vec<AtomicU64>,
    count: AtomicU64,
    sum_micros: AtomicU64,
}

impl HistogramEntry {
    fn new(bucket_count: usize) -> Self {
        Self {
            buckets: (0..bucket_count).map(|_| AtomicU64::new(0)).collect(),
            count: AtomicU64::new(0),
            sum_micros: AtomicU64::new(0),
        }
    }
}

fn label_set(
    metric_name: &str,
    expected_names: &[&str],
    labels: &[(&str, &str)],
) -> Result<LabelSet, String> {
    let names = labels.iter().map(|(name, _)| *name).collect::<Vec<_>>();
    validate_metric(metric_name, &names)?;
    if names != expected_names {
        return Err(format!(
            "metric labels for {metric_name} do not match allowlist"
        ));
    }
    Ok(labels
        .iter()
        .map(|(name, value)| ((*name).to_string(), bounded_label_value(value)))
        .collect())
}

fn bounded_label_value(value: &str) -> String {
    if value.is_empty()
        || value.len() > 128
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b'"' || byte == b'\\')
    {
        "other".into()
    } else {
        value.into()
    }
}

fn bounded_key<T>(
    entries: &mut BTreeMap<LabelSet, Arc<T>>,
    labels: LabelSet,
    label_set_cap: usize,
) -> LabelSet {
    if entries.contains_key(&labels) || entries.len() < label_set_cap.saturating_sub(1) {
        return labels;
    }
    labels
        .into_iter()
        .map(|(name, _)| (name, "other".into()))
        .collect()
}

fn render_header(output: &mut String, name: &str, help: &str, metric_type: &str) {
    let _ = writeln!(output, "# HELP {name} {help}");
    let _ = writeln!(output, "# TYPE {name} {metric_type}");
}

fn render_gauge(output: &mut String, name: &str, help: &str, value: i64) {
    validate_metric(name, &[]).expect("static metric definition is valid");
    render_header(output, name, help, "gauge");
    let _ = writeln!(output, "{name} {value}");
}

fn render_labels(labels: &LabelSet) -> String {
    if labels.is_empty() {
        return String::new();
    }
    let pairs = labels
        .iter()
        .map(|(name, value)| format!(r#"{name}="{}""#, escape_label_value(value)))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{pairs}}}")
}

fn escape_label_value(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| match character {
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '"' => "\\\"".chars().collect(),
            '\n' => "\\n".chars().collect(),
            other => vec![other],
        })
        .collect()
}

fn format_float(value: f64) -> String {
    let rendered = format!("{value:.3}");
    rendered.trim_end_matches('0').trim_end_matches('.').into()
}

#[cfg(test)]
mod tests {
    use super::MetricsRegistry;

    #[test]
    fn prometheus_output_uses_expected_text_format() {
        let registry = MetricsRegistry::new();
        registry.record_http_request("/api/v1/health/live", "GET", 200, 0.012);
        let output = registry.render_prometheus();
        assert!(output.contains("# HELP markhand_http_requests_total"));
        assert!(output.contains(
            r#"markhand_http_requests_total{route="/api/v1/health/live",method="GET",status="200"} 1"#
        ));
        assert!(output.contains("markhand_http_request_duration_seconds_bucket"));
        assert!(output.contains("markhand_jobs_in_flight 0"));
    }

    #[test]
    fn label_cardinality_is_capped_with_other_bucket() {
        let registry = MetricsRegistry::with_label_set_cap(3);
        for index in 0..10 {
            registry.record_http_request(&format!("/route/{index}"), "GET", 200, 0.001);
        }
        let output = registry.render_prometheus();
        assert!(output.contains(r#"route="other",method="other",status="other""#));
        assert!(output.matches("markhand_http_requests_total{").count() <= 3);
    }

    #[test]
    fn forbidden_metric_labels_are_rejected_and_not_rendered() {
        let registry = MetricsRegistry::new();
        assert!(registry
            .http_requests
            .increment(&[("route", "/safe"), ("method", "GET"), ("org_id", "tenant")])
            .is_err());
        let output = registry.render_prometheus();
        assert!(!output.contains("tenant"));
        assert!(!output.contains("org_id"));
    }

    #[test]
    fn metric_names_keep_markhand_prefix_contract() {
        assert!(crate::telemetry::validate_metric(
            "markhand_http_requests_total",
            &["route", "method", "status"]
        )
        .is_ok());
        assert!(crate::telemetry::validate_metric("http_requests_total", &[]).is_err());
    }
}
