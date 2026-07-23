#[tokio::main]
async fn main() {
    fileconv_server::init_tracing();
    let args: Vec<String> = std::env::args().collect();
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        println!(
            "fileconv-server\n\nStarts the Phase 1B POC health API.\n\nOptions:\n  --check-config   Validate config and exit\n  --migrate-only   Apply migrations via MARKHAND_MIGRATOR_DATABASE_URL and exit"
        );
        return;
    }
    let migrate_only = args.iter().any(|argument| argument == "--migrate-only");
    match fileconv_server::config::ServerConfig::from_env() {
        Ok(config) if args.iter().any(|argument| argument == "--check-config") => {
            match config.runtime_endpoints() {
                Ok(_) => println!(
                    "configuration valid: profile={:?}, bind={}",
                    config.profile(),
                    config.bind_addr()
                ),
                Err(error) => exit_with_error(format!("invalid server configuration: {error}")),
            }
        }
        Ok(_config) if migrate_only => {
            let migrator_url =
                migrator_database_url().unwrap_or_else(|error| exit_with_error(error));
            refuse_app_role_migration(&migrator_url);
            if let Err(error) = fileconv_server::database::apply_migrations(&migrator_url).await {
                exit_with_error(error);
            }
            println!("migrations applied via migrator credentials");
        }
        Ok(config) => {
            // API never migrates as the app role. One-shot migrator must run first
            // (compose `migrate` service / `deploy/scripts/migrate.sh`).
            if std::env::var_os("MARKHAND_ALLOW_INLINE_MIGRATE").is_some() {
                // Escape hatch for hermetic unit bootstraps only — still refuses app role.
                if let Ok(url) = migrator_database_url() {
                    refuse_app_role_migration(&url);
                    if let Err(error) = fileconv_server::database::apply_migrations(&url).await {
                        exit_with_error(error);
                    }
                }
            }
            let state = match fileconv_server::state::RuntimeState::from_config(config) {
                Ok(state) => state,
                Err(error) => exit_with_error(error.to_string()),
            };
            let app = match fileconv_server::http::AppState::new(state.clone()) {
                Ok(state) => fileconv_server::http::router(state),
                Err(error) => exit_with_error(error),
            };
            let listener = match tokio::net::TcpListener::bind(state.config().bind_addr()).await {
                Ok(listener) => listener,
                Err(error) => exit_with_error(format!("cannot bind server: {error}")),
            };
            println!(
                "fileconv-server listening on http://{}",
                state.config().bind_addr()
            );
            let serve_result = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown_signal())
            .await;
            // Flush AFTER serve returns (SIGTERM/Ctrl-C graceful path).
            let flushed = fileconv_server::telemetry::MetricsRegistry::shutdown_flush(
                std::time::Duration::from_secs(2),
            )
            .await;
            if flushed > 0 {
                tracing::info!(target: "telemetry", flushed, "api exporter shutdown flush complete");
            }
            if let Err(error) = serve_result {
                exit_with_error(format!("server failed: {error}"));
            }
        }
        Err(error) => {
            exit_with_error(format!("invalid server configuration: {error}"));
        }
    }
}

fn migrator_database_url() -> Result<String, String> {
    std::env::var("MARKHAND_MIGRATOR_DATABASE_URL")
        .map_err(|_| {
            "MARKHAND_MIGRATOR_DATABASE_URL is required for --migrate-only (dedicated migrator role)"
                .into()
        })
        .and_then(|value| {
            let trimmed = value.trim().to_string();
            if trimmed.is_empty() {
                return Err("MARKHAND_MIGRATOR_DATABASE_URL is empty".into());
            }
            Ok(trimmed)
        })
}

fn refuse_app_role_migration(database_url: &str) {
    let lower = database_url.to_ascii_lowercase();
    // Crude userinfo check — never migrate as markhand_app.
    if let Some(rest) = lower.split("://").nth(1) {
        let user = rest.split('@').next().unwrap_or_default();
        let user = user.split(':').next().unwrap_or_default();
        if user == "markhand_app" {
            exit_with_error(
                "refusing to migrate as markhand_app — use MARKHAND_MIGRATOR_DATABASE_URL / markhand_migrator"
                    .into(),
            );
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    {
        let terminate = async {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut signal) => {
                    signal.recv().await;
                }
                Err(error) => {
                    eprintln!("fileconv-server: cannot register SIGTERM handler: {error}");
                }
            }
        };
        tokio::select! {
            _ = ctrl_c => {}
            _ = terminate => {}
        }
    }
    #[cfg(not(unix))]
    ctrl_c.await;
}

fn exit_with_error(error: String) -> ! {
    eprintln!("fileconv-server: {error}");
    std::process::exit(1);
}
