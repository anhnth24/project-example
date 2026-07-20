use tracing_subscriber::EnvFilter;

use crate::config::{LogFormat, ServerConfig};

const DEFAULT_DEPENDENCY_LEVEL: &str = "warn";
const APPLICATION_TARGETS: &[&str] = &[
    "fileconv_server",
    "fileconv_worker",
    "fileconv_core",
    "fileconv_knowledge",
];
const SENSITIVE_DEPENDENCY_TARGETS: &[&str] = &[
    "tokio_postgres",
    "postgres",
    "postgres_protocol",
    "postgres_types",
    "deadpool",
    "deadpool_postgres",
    "hyper",
    "hyper_util",
    "h2",
    "reqwest",
    "rustls",
    "rust_s3",
    "s3",
    "tower",
    "tower_http",
];

pub fn init_tracing(config: &ServerConfig) {
    let rust_log = std::env::var("RUST_LOG").ok();
    let filter = env_filter(config.log_level(), rust_log.as_deref());

    let result = match config.log_format() {
        LogFormat::Json => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(true)
            .json()
            .try_init(),
        LogFormat::Pretty => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(true)
            .pretty()
            .try_init(),
    };
    let _ = result;
}

fn env_filter(configured: &str, rust_log: Option<&str>) -> EnvFilter {
    let directives = filter_directives(configured, rust_log);
    EnvFilter::try_new(&directives).unwrap_or_else(|_| EnvFilter::new(DEFAULT_DEPENDENCY_LEVEL))
}

fn filter_directives(configured: &str, rust_log: Option<&str>) -> String {
    let configured_level = level_directive(configured).unwrap_or("info");
    let source = rust_log
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(configured);
    let mut app_level = level_directive(source).unwrap_or(configured_level);
    let mut directives = vec![DEFAULT_DEPENDENCY_LEVEL.to_string()];
    let mut app_specific = Vec::new();

    for directive in source
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Some(level) = level_directive(directive) {
            app_level = level;
            continue;
        }
        let Some((target, level)) = directive.rsplit_once('=') else {
            continue;
        };
        let target = target.trim();
        let Some(level) = normalize_level(level.trim()) else {
            continue;
        };
        if APPLICATION_TARGETS
            .iter()
            .any(|app| target == *app || target.starts_with(&format!("{app}::")))
        {
            app_specific.push(format!("{target}={level}"));
        }
    }

    directives.extend(
        APPLICATION_TARGETS
            .iter()
            .map(|target| format!("{target}={app_level}")),
    );
    directives.extend(app_specific);
    // Keep these caps last: EnvFilter gives equal-specificity directives last-wins
    // precedence, and earlier env/config dependency directives are ignored anyway.
    directives.extend(
        SENSITIVE_DEPENDENCY_TARGETS
            .iter()
            .map(|target| format!("{target}=warn")),
    );
    directives.join(",")
}

fn level_directive(value: &str) -> Option<&'static str> {
    let trimmed = value.trim();
    if trimmed.contains('=') || trimmed.contains('[') {
        return None;
    }
    normalize_level(trimmed)
}

fn normalize_level(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "trace" => Some("trace"),
        "debug" => Some("debug"),
        "info" => Some("info"),
        "warn" => Some("warn"),
        "error" => Some("error"),
        "off" => Some("off"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use tracing::subscriber::with_default;
    use tracing_subscriber::fmt::MakeWriter;

    use super::{env_filter, filter_directives};

    #[derive(Clone, Default)]
    struct Capture {
        bytes: Arc<Mutex<Vec<u8>>>,
    }

    struct CaptureWriter {
        bytes: Arc<Mutex<Vec<u8>>>,
    }

    impl<'a> MakeWriter<'a> for Capture {
        type Writer = CaptureWriter;

        fn make_writer(&'a self) -> Self::Writer {
            CaptureWriter {
                bytes: self.bytes.clone(),
            }
        }
    }

    impl Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.bytes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn sensitive_dependencies_are_capped_while_app_targets_follow_filter() {
        let capture = Capture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(env_filter(
                "info",
                Some("tokio_postgres=debug,reqwest=trace,fileconv_server=debug"),
            ))
            .with_writer(capture.clone())
            .with_target(true)
            .with_ansi(false)
            .without_time()
            .finish();

        with_default(subscriber, || {
            tracing::debug!(target: "tokio_postgres", "postgres debug canary");
            tracing::trace!(target: "reqwest", "reqwest trace canary");
            tracing::debug!(target: "fileconv_server", "app debug canary");
            tracing::info!(target: "fileconv_worker", "worker startup canary");
            tracing::warn!(target: "tokio_postgres", "postgres warn canary");
        });

        let output = String::from_utf8(
            capture
                .bytes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        )
        .expect("utf8 logs");
        assert!(output.contains("app debug canary"));
        assert!(output.contains("worker startup canary"));
        assert!(output.contains("fileconv_worker"));
        assert!(output.contains("postgres warn canary"));
        assert!(!output.contains("postgres debug canary"));
        assert!(!output.contains("reqwest trace canary"));

        let directives = filter_directives(
            "info",
            Some("tokio_postgres=debug,reqwest=trace,fileconv_server=debug"),
        );
        assert!(directives.ends_with("tower_http=warn"));
        assert!(directives.contains("fileconv_server=debug"));
        assert!(directives.contains("fileconv_worker=info"));
        assert!(directives.contains("tokio_postgres=warn"));
        assert!(directives.contains("reqwest=warn"));
    }
}
