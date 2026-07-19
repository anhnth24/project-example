use std::sync::Arc;
use std::time::Duration;

use deadpool_postgres::Pool;
use tokio::time::timeout;

use crate::database;
use crate::http::{AppState, DEPENDENCY_TIMEOUT};
use crate::services::index_signature::validate_signature_digest;

const READINESS_CACHE_TTL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub(crate) struct CachedReadiness {
    pub(crate) checked_at: tokio::time::Instant,
    pub(crate) result: Result<(), HealthErrorKind>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum HealthErrorKind {
    DependencyUnavailable,
    ConfigurationInvalid,
}

pub(crate) async fn check_dependencies(state: Arc<AppState>) -> Result<(), HealthErrorKind> {
    let mut cached = state.readiness_cache().lock().await;
    if let Some(previous) = cached.as_ref() {
        if previous.checked_at.elapsed() < READINESS_CACHE_TTL {
            return previous.result;
        }
    }

    let result = check_dependencies_uncached(&state).await;
    *cached = Some(CachedReadiness {
        checked_at: tokio::time::Instant::now(),
        result,
    });
    result
}

async fn check_dependencies_uncached(state: &AppState) -> Result<(), HealthErrorKind> {
    validate_index_signature_config(state)?;
    // TODO(O03): reconciliation-backlog readiness.
    let database = timeout(
        DEPENDENCY_TIMEOUT,
        database::check_connection(state.runtime().endpoints().database_url.expose()),
    );
    let migrations = timeout(DEPENDENCY_TIMEOUT, check_migrations_applied(state.pool()));
    let qdrant = state
        .http_client()
        .get(format!(
            "{}/healthz",
            state.runtime().endpoints().qdrant_url
        ))
        .send();
    let minio = state
        .http_client()
        .get(format!(
            "{}/minio/health/live",
            state.runtime().endpoints().minio_url
        ))
        .send();

    let (database, migrations, qdrant, minio) = tokio::join!(database, migrations, qdrant, minio);
    database
        .map_err(|_| HealthErrorKind::DependencyUnavailable)?
        .map_err(|_| HealthErrorKind::DependencyUnavailable)?;
    migrations
        .map_err(|_| HealthErrorKind::DependencyUnavailable)?
        .map_err(|_| HealthErrorKind::DependencyUnavailable)?;
    ensure_success(qdrant).await?;
    ensure_success(minio).await
}

fn validate_index_signature_config(state: &AppState) -> Result<(), HealthErrorKind> {
    let Some(signature) = state.runtime().config().index_signature() else {
        return Ok(());
    };
    let normalized = signature.trim().to_ascii_lowercase();
    validate_signature_digest(&normalized).map_err(|_| HealthErrorKind::ConfigurationInvalid)
}

async fn check_migrations_applied(pool: &Pool) -> Result<(), String> {
    let (expected_name, expected_checksum) = database::latest_migration();
    let client = pool
        .get()
        .await
        .map_err(|error| format!("PostgreSQL pool checkout failed: {error}"))?;
    let row = client
        .query_opt(
            "SELECT checksum FROM markhand_schema_migrations WHERE name = $1",
            &[&expected_name],
        )
        .await
        .map_err(|error| format!("cannot inspect migration history: {error}"))?;
    match row {
        Some(row) if row.get::<_, String>(0) == expected_checksum => Ok(()),
        Some(_) => Err(format!("migration checksum mismatch for {expected_name}")),
        None => Err(format!("migration {expected_name} has not been applied")),
    }
}

async fn ensure_success(
    response: Result<reqwest::Response, reqwest::Error>,
) -> Result<(), HealthErrorKind> {
    let response = response.map_err(|_| HealthErrorKind::DependencyUnavailable)?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(HealthErrorKind::DependencyUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tokio_postgres::NoTls;
    use tower::ServiceExt;
    use uuid::Uuid;

    use crate::api::ApiError;
    use crate::config::{RuntimeEndpoints, SecretString, ServerConfig};
    use crate::db::pool::create_pool;
    use crate::http::{router, AppState};
    use crate::state::RuntimeState;

    fn app_with_unreachable_dependencies() -> axum::Router {
        let runtime =
            RuntimeState::from_config(ServerConfig::test_with_endpoints(RuntimeEndpoints {
                database_url: SecretString::new("postgres://unused"),
                qdrant_url: "http://127.0.0.1:1".into(),
                minio_url: "http://127.0.0.1:1".into(),
            }))
            .unwrap();
        let pool = create_pool("postgres://markhand_app:markhand_app@127.0.0.1:5432/markhand_test")
            .expect("pool");
        router(AppState::from_parts(runtime, pool, None).unwrap())
    }

    fn test_database_url() -> Option<String> {
        match std::env::var("MARKHAND_TEST_DATABASE_URL") {
            Ok(url) if !url.trim().is_empty() => Some(url),
            _ => {
                eprintln!("skipped: MARKHAND_TEST_DATABASE_URL unset");
                None
            }
        }
    }

    fn rewrite_database_url(base_url: &str, database_name: &str) -> String {
        let (without_query, query) = match base_url.split_once('?') {
            Some((head, tail)) => (head, Some(tail)),
            None => (base_url, None),
        };
        let prefix = without_query
            .rsplit_once('/')
            .map(|(head, _)| head)
            .expect("database URL must include a path");
        match query {
            Some(tail) => format!("{prefix}/{database_name}?{tail}"),
            None => format!("{prefix}/{database_name}"),
        }
    }

    async fn connect_raw(database_url: &str) -> tokio_postgres::Client {
        let (client, connection) = tokio_postgres::connect(database_url, NoTls)
            .await
            .unwrap_or_else(|error| panic!("database connection failed: {error}"));
        tokio::spawn(async move {
            let _ = connection.await;
        });
        client
    }

    struct EphemeralDb {
        admin_url: String,
        db_name: String,
        url: String,
    }

    impl EphemeralDb {
        async fn create(base_url: &str) -> Self {
            let db_name = format!("markhand_health_{}", Uuid::new_v4().simple());
            let admin_url = rewrite_database_url(base_url, "postgres");
            let admin = connect_raw(&admin_url).await;
            admin
                .batch_execute(&format!("CREATE DATABASE \"{db_name}\""))
                .await
                .expect("CREATE DATABASE");
            Self {
                admin_url,
                db_name: db_name.clone(),
                url: rewrite_database_url(base_url, &db_name),
            }
        }

        async fn drop(self) {
            let admin = connect_raw(&self.admin_url).await;
            let _ = admin
                .batch_execute(&format!(
                    "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
                     WHERE datname = '{}' AND pid <> pg_backend_pid(); \
                     DROP DATABASE IF EXISTS \"{}\"",
                    self.db_name, self.db_name
                ))
                .await;
        }
    }

    #[tokio::test]
    async fn readiness_fails_closed_when_dependencies_are_unreachable() {
        let response = app_with_unreachable_dependencies()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/health/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let error: ApiError = serde_json::from_slice(&body).unwrap();
        assert_eq!(error.code, "dependency_unavailable");
    }

    #[tokio::test]
    async fn startup_probe_reflects_initialization_flag() {
        let runtime =
            RuntimeState::from_config(ServerConfig::test_with_endpoints(RuntimeEndpoints {
                database_url: SecretString::new("postgres://unused"),
                qdrant_url: "http://127.0.0.1:1".into(),
                minio_url: "http://127.0.0.1:1".into(),
            }))
            .unwrap();
        let pool = create_pool("postgres://markhand_app:markhand_app@127.0.0.1:5432/markhand_test")
            .expect("pool");
        let state = AppState::from_parts(runtime, pool, None).unwrap();
        state.set_startup_complete_for_test(false);
        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/health/start")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn migration_history_missing_fails_closed_when_database_is_available() {
        let Some(base_url) = test_database_url() else {
            return;
        };
        let db = EphemeralDb::create(&base_url).await;
        let pool = create_pool(&db.url).expect("pool");
        let result = super::check_migrations_applied(&pool).await;
        db.drop().await;
        assert!(result
            .unwrap_err()
            .contains("cannot inspect migration history"));
    }
}
