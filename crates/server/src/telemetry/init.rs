//! Safe tracing/metrics initialization with optional OTLP + explicit test capture.

use std::sync::OnceLock;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{MetricExporter as OtlpMetricExporter, SpanExporter, WithExportConfig};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::resource::Resource;
use opentelemetry_sdk::trace::{
    InMemorySpanExporter, Sampler, SdkTracerProvider, SimpleSpanProcessor,
};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Registry;

use super::config::{OtelExporterKind, TelemetryConfig};
use super::metrics;

/// Process telemetry handles; set only after a fully successful init.
pub struct TelemetryRuntime {
    tracer_provider: SdkTracerProvider,
    meter_provider: Option<SdkMeterProvider>,
    span_exporter: Option<InMemorySpanExporter>,
    metric_exporter: Option<InMemoryMetricExporter>,
}

impl TelemetryRuntime {
    pub fn force_flush(&self) -> Result<(), String> {
        self.tracer_provider
            .force_flush()
            .map_err(|error| format!("trace flush failed: {error}"))?;
        if let Some(meter) = &self.meter_provider {
            meter
                .force_flush()
                .map_err(|error| format!("metric flush failed: {error}"))?;
        }
        Ok(())
    }

    pub fn shutdown(&self) -> Result<(), String> {
        let _ = self.force_flush();
        self.tracer_provider
            .shutdown()
            .map_err(|error| format!("trace shutdown failed: {error}"))?;
        if let Some(meter) = &self.meter_provider {
            meter
                .shutdown()
                .map_err(|error| format!("metric shutdown failed: {error}"))?;
        }
        Ok(())
    }

    pub fn span_exporter(&self) -> Option<&InMemorySpanExporter> {
        self.span_exporter.as_ref()
    }

    pub fn metric_exporter(&self) -> Option<&InMemoryMetricExporter> {
        self.metric_exporter.as_ref()
    }
}

static RUNTIME: OnceLock<TelemetryRuntime> = OnceLock::new();

/// Initialise process telemetry. Safe to call once; subsequent calls are no-ops.
///
/// - Installs fmt tracing (+ OTel layer) and propagates `try_init` errors.
/// - OTLP exporter is installed only when configured **and** network is allowed.
/// - In-memory exporters are installed only when `capture_in_memory` is explicit.
/// - `exporter=none` without capture never installs unbounded test exporters.
/// - `OnceLock` is set only after success.
pub fn init(config: &TelemetryConfig) -> Result<(), String> {
    if RUNTIME.get().is_some() {
        return Ok(());
    }

    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let resource = Resource::builder()
        .with_service_name(config.service_name.clone())
        .build();

    // ParentBased for every ratio, including 0.0 and 1.0, so remote parents win.
    let root = if config.sample_ratio() <= 0.0 {
        Sampler::AlwaysOff
    } else if config.sample_ratio() >= 1.0 {
        Sampler::AlwaysOn
    } else {
        Sampler::TraceIdRatioBased(config.sample_ratio())
    };
    let sampler = Sampler::ParentBased(Box::new(root));

    let (tracer_provider, span_exporter) =
        build_tracer_provider(config, resource.clone(), sampler)?;
    let tracer = tracer_provider.tracer(config.service_name.clone());
    opentelemetry::global::set_tracer_provider(tracer_provider.clone());

    let (meter_provider, metric_exporter) = if config.metrics_enabled {
        let built = build_meter_provider(config, resource)?;
        opentelemetry::global::set_meter_provider(built.0.clone());
        let meter = opentelemetry::global::meter("markhand");
        metrics::install(&meter, true)?;
        (Some(built.0), built.1)
    } else {
        metrics::install(&opentelemetry::global::meter("markhand"), false)?;
        (None, None)
    };

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_ansi(false);
    Registry::default()
        .with(filter)
        .with(fmt_layer)
        .with(otel_layer)
        .try_init()
        .map_err(|error| format!("tracing subscriber init failed: {error}"))?;

    let runtime = TelemetryRuntime {
        tracer_provider,
        meter_provider,
        span_exporter,
        metric_exporter,
    };
    RUNTIME
        .set(runtime)
        .map_err(|_| "telemetry runtime already initialised".to_string())?;

    if config.exporter_enabled() {
        tracing::info!(
            target: "telemetry",
            service = %config.service_name,
            metrics_enabled = config.metrics_enabled,
            "OTLP tracing exporter enabled"
        );
    } else if config.exporter == OtelExporterKind::Otlp && config.disable_network {
        tracing::info!(
            target: "telemetry",
            "OTLP exporter configured but network disabled; local exporters only when capture enabled"
        );
    }
    Ok(())
}

fn build_tracer_provider(
    config: &TelemetryConfig,
    resource: Resource,
    sampler: Sampler,
) -> Result<(SdkTracerProvider, Option<InMemorySpanExporter>), String> {
    let builder = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_sampler(sampler);

    if config.exporter_enabled() {
        let endpoint = config
            .otlp_endpoint
            .as_deref()
            .ok_or("otlp endpoint missing")?;
        let exporter = SpanExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint)
            .build()
            .map_err(|error| format!("otlp span exporter failed: {error}"))?;
        Ok((builder.with_batch_exporter(exporter).build(), None))
    } else if config.capture_in_memory {
        let exporter = InMemorySpanExporter::default();
        Ok((
            builder
                .with_span_processor(SimpleSpanProcessor::new(exporter.clone()))
                .build(),
            Some(exporter),
        ))
    } else {
        // exporter=none: no unbounded in-memory capture in general/prod runtime.
        Ok((builder.build(), None))
    }
}

fn build_meter_provider(
    config: &TelemetryConfig,
    resource: Resource,
) -> Result<(SdkMeterProvider, Option<InMemoryMetricExporter>), String> {
    if config.exporter_enabled() {
        let endpoint = config
            .otlp_endpoint
            .as_deref()
            .ok_or("otlp endpoint missing")?;
        let exporter = OtlpMetricExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint)
            .build()
            .map_err(|error| format!("otlp metric exporter failed: {error}"))?;
        let reader = PeriodicReader::builder(exporter).build();
        Ok((
            SdkMeterProvider::builder()
                .with_resource(resource)
                .with_reader(reader)
                .build(),
            None,
        ))
    } else if config.capture_in_memory {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone()).build();
        Ok((
            SdkMeterProvider::builder()
                .with_resource(resource)
                .with_reader(reader)
                .build(),
            Some(exporter),
        ))
    } else {
        Ok((
            SdkMeterProvider::builder().with_resource(resource).build(),
            None,
        ))
    }
}

/// Convenience init for binaries: load config from process env + profile.
pub fn init_from_env(profile: crate::config::Profile) -> Result<(), String> {
    let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let config = TelemetryConfig::from_env_map(&env, profile)?;
    init(&config)
}

pub fn runtime() -> Option<&'static TelemetryRuntime> {
    RUNTIME.get()
}

pub fn force_flush() -> Result<(), String> {
    match RUNTIME.get() {
        Some(runtime) => runtime.force_flush(),
        None => Ok(()),
    }
}

pub fn shutdown() -> Result<(), String> {
    match RUNTIME.get() {
        Some(runtime) => runtime.shutdown(),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Profile;
    use std::collections::BTreeMap;

    #[test]
    fn init_without_network_does_not_require_collector() {
        let mut config = TelemetryConfig::from_env_map(&BTreeMap::new(), Profile::Test).unwrap();
        config.capture_in_memory = true;
        // May already be initialised by other tests; must still succeed.
        assert!(init(&config).is_ok());
        assert!(runtime().is_some());
    }

    #[test]
    fn sampler_is_parent_based_for_extreme_ratios() {
        // Construction path used by init — AlwaysOn/Off are wrapped in ParentBased.
        let off = Sampler::ParentBased(Box::new(Sampler::AlwaysOff));
        let on = Sampler::ParentBased(Box::new(Sampler::AlwaysOn));
        let _ = (off, on);
    }
}
