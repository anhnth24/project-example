#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        println!("fileconv-server\n\nStarts the Phase 1B POC health API and applies migrations.");
        return;
    }
    match fileconv_server::config::ServerConfig::from_env() {
        Ok(config) if args.iter().any(|argument| argument == "--check-config") => {
            fileconv_server::telemetry::logging::init_tracing(&config);
            match config.runtime_endpoints() {
                Ok(_) => println!(
                    "configuration valid: profile={:?}, bind={}",
                    config.profile(),
                    config.bind_addr()
                ),
                Err(error) => exit_with_error(format!("invalid server configuration: {error}")),
            }
        }
        Ok(config) => {
            fileconv_server::telemetry::logging::init_tracing(&config);
            let state = match fileconv_server::state::RuntimeState::from_config(config) {
                Ok(state) => state,
                Err(error) => exit_with_error(error.to_string()),
            };
            if let Err(error) =
                fileconv_server::database::apply_migrations(state.endpoints().database_url.expose())
                    .await
            {
                exit_with_error(error);
            }
            let app = match fileconv_server::http::AppState::new(state.clone()) {
                Ok(state) => fileconv_server::http::router(state),
                Err(error) => exit_with_error(error),
            };
            let listener = match tokio::net::TcpListener::bind(state.config().bind_addr()).await {
                Ok(listener) => listener,
                Err(error) => exit_with_error(format!("cannot bind server: {error}")),
            };
            tracing::info!(
                bind_addr = %state.config().bind_addr(),
                "fileconv-server listening"
            );
            if let Err(error) = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown_signal())
            .await
            {
                exit_with_error(format!("server failed: {error}"));
            }
        }
        Err(error) => {
            exit_with_error(format!("invalid server configuration: {error}"));
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
                    tracing::warn!(error = %error, "cannot register SIGTERM handler");
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
