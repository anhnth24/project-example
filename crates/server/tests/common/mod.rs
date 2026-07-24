//! Shared helpers for DB-backed server integration tests.
//!
//! Dual-role layout:
//! - `MARKHAND_TEST_DATABASE_URL` — bootstrap role with `CREATEDB` (compose superuser)
//! - `MARKHAND_TEST_APP_DATABASE_URL` — non-superuser `markhand_app` for FORCE RLS
#![allow(dead_code)] // not every integration binary uses every helper

pub mod fixtures;

use bytes::Bytes;
use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::auth::jwt::JwtKeys;
use fileconv_server::auth::provider::{AuthProvider, AuthRequestMeta, PasswordAuthProvider};
use fileconv_server::auth::session;
use fileconv_server::config::{
    Argon2Config, AuthConfig, JwtAlgorithm, MinioConfig, RuntimeEndpoints, SecretString,
    ServerConfig,
};
use fileconv_server::database::apply_migrations;
use fileconv_server::db::orgs;
use fileconv_server::db::pool::{create_pool, create_pool_with_max_size, with_org_txn};
use fileconv_server::http::{router, AppState};
use fileconv_server::state::RuntimeState;
use fileconv_server::storage::minio::{MinioClient, ObjectIdentityMeta};
use tokio_postgres::NoTls;
use uuid::Uuid;

/// When `MARKHAND_E2E=1`, soft-skips are forbidden — missing live deps must panic.
pub fn markhand_e2e_required() -> bool {
    std::env::var("MARKHAND_E2E").ok().as_deref() == Some("1")
}

/// Pass through `Some`, panic under `MARKHAND_E2E=1` when missing, else `None` (soft-skip).
pub fn take_live<T>(value: Option<T>, name: &str) -> Option<T> {
    match value {
        Some(value) => Some(value),
        None if markhand_e2e_required() => panic!("MARKHAND_E2E=1 requires {name}"),
        None => None,
    }
}

pub fn admin_database_url() -> Option<String> {
    match std::env::var("MARKHAND_TEST_DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => {
            eprintln!(
                "skipped: MARKHAND_TEST_DATABASE_URL unset — integration tests require PostgreSQL"
            );
            None
        }
    }
}

pub fn app_database_url() -> Option<String> {
    match std::env::var("MARKHAND_TEST_APP_DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => {
            eprintln!(
                "skipped: MARKHAND_TEST_APP_DATABASE_URL unset — FORCE RLS assertions require markhand_app"
            );
            None
        }
    }
}

pub fn rewrite_database_url(base_url: &str, database_name: &str) -> String {
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

pub async fn connect_raw(database_url: &str) -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .unwrap_or_else(|error| panic!("connect failed for {database_url}: {error}"));
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

/// Drop an ephemeral database with an independent `WITH (FORCE)` statement.
///
/// Failures propagate so suites cannot silently leave prefix databases behind.
pub async fn drop_database_force(admin_maintenance_url: &str, db_name: &str) -> Result<(), String> {
    let admin = connect_raw(admin_maintenance_url).await;
    admin
        .batch_execute(&format!(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
             WHERE datname = '{db_name}' AND pid <> pg_backend_pid()"
        ))
        .await
        .map_err(|error| format!("terminate backends for {db_name}: {error}"))?;
    admin
        .batch_execute(&format!(
            "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
        ))
        .await
        .map_err(|error| format!("DROP DATABASE {db_name} WITH (FORCE): {error}"))?;
    Ok(())
}

/// Ephemeral database created by the admin role, with the app role granted and
/// used for the application pool so FORCE RLS is actually enforced.
pub struct DualRoleEphemeralDb {
    admin_maintenance_url: String,
    db_name: String,
    pub admin_db_url: String,
    pub app_url: String,
}

impl DualRoleEphemeralDb {
    pub fn db_name(&self) -> &str {
        &self.db_name
    }

    pub async fn create(admin_base_url: &str, app_base_url: &str) -> Self {
        let db_name = format!("markhand_it_{}", Uuid::new_v4().simple());
        let admin_maintenance_url = rewrite_database_url(admin_base_url, "postgres");
        let admin = connect_raw(&admin_maintenance_url).await;
        admin
            .batch_execute(&format!("CREATE DATABASE \"{db_name}\""))
            .await
            .expect("CREATE DATABASE");
        admin
            .batch_execute(&format!(
                "GRANT CONNECT ON DATABASE \"{db_name}\" TO markhand_app"
            ))
            .await
            .expect("GRANT CONNECT to markhand_app");

        let admin_db_url = rewrite_database_url(admin_base_url, &db_name);
        let app_url = rewrite_database_url(app_base_url, &db_name);

        // Migrate as the bootstrap role (CREATE EXTENSION / ownership), then
        // grant the non-superuser app role DML + EXECUTE so FORCE RLS applies.
        apply_migrations(&admin_db_url)
            .await
            .expect("apply migrations");
        let admin_on_db = connect_raw(&admin_db_url).await;
        admin_on_db
            .batch_execute(
                "GRANT USAGE ON SCHEMA public TO markhand_app;
                 REVOKE CREATE ON SCHEMA public FROM markhand_app;
                 GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO markhand_app;
                 GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO markhand_app;
                 GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO markhand_app;
                 REVOKE UPDATE, DELETE, TRUNCATE ON TABLE audit_log FROM markhand_app;
                 GRANT SELECT, INSERT ON TABLE audit_log TO markhand_app;",
            )
            .await
            .expect("grant app role privileges on ephemeral database");

        Self {
            admin_maintenance_url,
            db_name,
            admin_db_url,
            app_url,
        }
    }

    pub async fn drop(self) {
        drop_database_force(&self.admin_maintenance_url, &self.db_name)
            .await
            .unwrap_or_else(|error| panic!("ephemeral database cleanup failed: {error}"));
    }
}

pub async fn boot_app_pool(
    admin_base_url: &str,
    app_base_url: &str,
) -> (DualRoleEphemeralDb, Pool) {
    let ephemeral = DualRoleEphemeralDb::create(admin_base_url, app_base_url).await;
    let pool = create_pool(&ephemeral.app_url).expect("create app-role pool");
    (ephemeral, pool)
}

pub async fn boot_app_pool_with_max_size(
    admin_base_url: &str,
    app_base_url: &str,
    max_size: usize,
) -> (DualRoleEphemeralDb, Pool) {
    let ephemeral = DualRoleEphemeralDb::create(admin_base_url, app_base_url).await;
    let pool = create_pool_with_max_size(&ephemeral.app_url, max_size)
        .expect("create sized app-role pool");
    (ephemeral, pool)
}

/// Assert the pool connection is `markhand_app` without superuser/bypassrls.
pub async fn assert_markhand_app_role(pool: &Pool) {
    let client = pool.get().await.expect("app pool client");
    let row = client
        .query_one(
            "SELECT current_user::text AS current_user,
                    rolsuper,
                    rolbypassrls
             FROM pg_roles
             WHERE rolname = current_user",
            &[],
        )
        .await
        .expect("role probe");
    let current_user: String = row.get("current_user");
    let rolsuper: bool = row.get("rolsuper");
    let rolbypassrls: bool = row.get("rolbypassrls");
    assert_eq!(current_user, "markhand_app");
    assert!(!rolsuper, "markhand_app must not be superuser");
    assert!(!rolbypassrls, "markhand_app must not bypass RLS");
}
pub fn test_auth_config() -> AuthConfig {
    AuthConfig {
        issuer: Some("https://issuer.markhand.test".into()),
        audience: Some("markhand-api".into()),
        signing_key: Some(SecretString::new("integration-test-signing-key-32b!")),
        alg: JwtAlgorithm::Hs256,
        kid: Some("test-kid-1".into()),
        access_token_ttl_secs: 900,
        refresh_token_ttl_secs: 3_600,
        argon2: Argon2Config {
            memory_kib: 8_192,
            time_cost: 1,
            parallelism: 1,
        },
    }
}

pub fn test_minio_client() -> Option<MinioClient> {
    let endpoint = match std::env::var("MARKHAND_TEST_MINIO_ENDPOINT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("MARKHAND_TEST_OBJECT_STORE_ENDPOINT")
                .ok()
                .filter(|value| !value.trim().is_empty())
        }) {
        Some(url) => url,
        None => {
            eprintln!("skipped: MARKHAND_TEST_MINIO_ENDPOINT unset");
            return None;
        }
    };
    let access_key = std::env::var("MARKHAND_TEST_MINIO_ACCESS_KEY")
        .ok()
        .or_else(|| std::env::var("MARKHAND_TEST_OBJECT_STORE_ACCESS_KEY").ok())?;
    let secret_key = std::env::var("MARKHAND_TEST_MINIO_SECRET_KEY")
        .ok()
        .or_else(|| std::env::var("MARKHAND_TEST_OBJECT_STORE_SECRET_KEY").ok())?;
    if access_key.is_empty() || secret_key.is_empty() {
        eprintln!("skipped: MinIO test credentials empty");
        return None;
    }
    let region = std::env::var("MARKHAND_TEST_MINIO_REGION")
        .or_else(|_| std::env::var("MARKHAND_TEST_OBJECT_STORE_REGION"))
        .unwrap_or_else(|_| "us-east-1".into());
    let bucket = format!("markhand-it-{}", Uuid::new_v4().simple());
    std::env::set_var("RUST_S3_SKIP_LOCATION_CONSTRAINT", "true");
    let config = MinioConfig::new(
        endpoint,
        SecretString::new(access_key),
        SecretString::new(secret_key),
        bucket,
        region,
        true,
    )
    .expect("minio config");
    Some(MinioClient::from_config(&config).expect("minio client"))
}

/// Deletes objects/bucket even if the owning test panics.
///
/// Prefer [`MinioCleanupGuard::cleanup`].await in the success path so errors
/// propagate and the bucket-gone assertion runs; Drop remains a last-resort.
pub struct MinioCleanupGuard {
    client: MinioClient,
    cleaned: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl MinioCleanupGuard {
    pub fn new(client: MinioClient) -> Self {
        Self {
            client,
            cleaned: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Explicit async cleanup that propagates errors and asserts the bucket is gone.
    pub async fn cleanup(&self) -> Result<(), fileconv_server::storage::StorageError> {
        if self.cleaned.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        self.client.cleanup_bucket_and_assert_gone().await
    }
}

impl Drop for MinioCleanupGuard {
    fn drop(&mut self) {
        if self.cleaned.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let client = self.client.clone();
        let cleaned = self.cleaned.clone();
        let _ = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            if let Ok(runtime) = runtime {
                if runtime
                    .block_on(client.cleanup_bucket_and_assert_gone())
                    .is_ok()
                {
                    cleaned.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
        })
        .join();
    }
}

/// Seed an org user with the given permission codes (owner role) + password.
pub async fn seed_user_with_permissions(
    pool: &Pool,
    org: Uuid,
    user: Uuid,
    email: &str,
    password: &str,
    permissions: &[&str],
) {
    let ctx = OrgContext::try_new(org, user, permissions.iter().copied(), []).unwrap();
    let email = email.to_string();
    let permission_codes: Vec<String> = permissions.iter().map(|p| (*p).to_string()).collect();
    with_org_txn(pool, &ctx, {
        let owned = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let slug = format!("it-org-{}", org.simple());
                orgs::ensure_exists(txn, &owned, &slug, "Integration Org").await?;
                orgs::ensure_user(txn, &owned, user, &email, "Integration User").await?;
                orgs::ensure_membership(txn, &owned).await?;
                txn.execute(
                    "INSERT INTO org_quotas (
                        org_id, max_storage_bytes, max_documents,
                        max_concurrent_jobs, max_monthly_tokens
                     )
                     VALUES ($1, 1073741824, 1000, 100, 1000000)
                     ON CONFLICT (org_id) DO NOTHING",
                    &[&org],
                )
                .await?;
                for code in &permission_codes {
                    txn.execute(
                        "INSERT INTO permissions (id, code, description)
                         VALUES ($1, $2, $2)
                         ON CONFLICT (code) DO NOTHING",
                        &[&Uuid::new_v4(), code],
                    )
                    .await?;
                }
                let role_id = Uuid::new_v4();
                txn.execute(
                    "INSERT INTO roles (id, org_id, code, name, is_system)
                     VALUES ($1, $2, 'owner', 'Owner', true)
                     ON CONFLICT (org_id, code) DO NOTHING",
                    &[&role_id, &org],
                )
                .await?;
                let role_id: Uuid = txn
                    .query_one(
                        "SELECT id FROM roles WHERE org_id = $1 AND code = 'owner'",
                        &[&org],
                    )
                    .await?
                    .get(0);
                for code in &permission_codes {
                    let perm_id: Uuid = txn
                        .query_one("SELECT id FROM permissions WHERE code = $1", &[code])
                        .await?
                        .get(0);
                    txn.execute(
                        "INSERT INTO role_permissions (org_id, role_id, permission_id)
                         VALUES ($1, $2, $3)
                         ON CONFLICT DO NOTHING",
                        &[&org, &role_id, &perm_id],
                    )
                    .await?;
                }
                Ok(())
            })
        }
    })
    .await
    .expect("seed user permissions");
    session::set_password_hash(pool, user, password, &test_auth_config().argon2)
        .await
        .expect("set password");
}

pub async fn login_access_token(pool: &Pool, email: &str, password: &str) -> String {
    login_tokens(pool, email, password).await.0
}

/// Returns `(access_token, refresh_token)` for production-router logout barriers.
pub async fn login_tokens(pool: &Pool, email: &str, password: &str) -> (String, String) {
    let auth = PasswordAuthProvider::new(
        pool.clone(),
        test_auth_config(),
        JwtKeys::from_auth(&test_auth_config()).unwrap(),
    );
    let login = auth
        .login_password(
            email,
            password,
            &AuthRequestMeta {
                // audit_log_validate_insert requires a UUID request_id.
                request_id: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("login");
    (
        login.tokens.access_token.expose().to_string(),
        login.tokens.refresh_token.expose().to_string(),
    )
}

pub fn build_app_state(pool: Pool, app_database_url: &str, store: Option<MinioClient>) -> AppState {
    // Pool is injected explicitly; database_url is only for RuntimeState wiring.
    let runtime = RuntimeState::from_config(ServerConfig::test_with_endpoints(RuntimeEndpoints {
        database_url: SecretString::new(app_database_url),
        qdrant_url: "http://127.0.0.1:6333".into(),
        minio_url: "http://127.0.0.1:9000".into(),
    }))
    .expect("runtime");
    let auth = PasswordAuthProvider::new(
        pool.clone(),
        test_auth_config(),
        JwtKeys::from_auth(&test_auth_config()).unwrap(),
    );
    AppState::from_parts_with_store(runtime, pool, Some(auth), store).expect("app state")
}

pub fn build_router(
    pool: Pool,
    app_database_url: &str,
    store: Option<MinioClient>,
) -> axum::Router {
    router(build_app_state(pool, app_database_url, store))
}

pub async fn put_bytes(
    store: &MinioClient,
    org: Uuid,
    key: &fileconv_server::storage::ObjectKey,
    bytes: &[u8],
    content_type: &str,
    meta: ObjectIdentityMeta,
) {
    store.ensure_bucket().await.expect("ensure bucket");
    store
        .put_object(org, key, Bytes::copy_from_slice(bytes), &meta, content_type)
        .await
        .expect("put object");
}

#[allow(unused_imports)]
pub use fileconv_server::storage::keys::{quarantine_key, trusted_key};
#[allow(unused_imports)]
pub use fixtures::{
    convert_to_markdown, sha256_hex, tiny_docx_bytes, tiny_pdf_bytes, tiny_png_ocr_bytes,
    tiny_pptx_bytes, tiny_xlsx_bytes,
};
