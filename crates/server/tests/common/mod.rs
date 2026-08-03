//! Shared helpers for DB-backed server integration tests.
//!
//! Dual-role layout:
//! - `MARKHAND_TEST_DATABASE_URL` — bootstrap role with `CREATEDB` (compose superuser)
//! - `MARKHAND_TEST_APP_DATABASE_URL` — non-superuser `markhand_app` for FORCE RLS
#![allow(dead_code)] // not every integration binary uses every helper

pub mod acl_fixture;
pub mod fixtures;
pub mod fts_visibility_diagnostic;
pub mod multi_org_denial;
pub mod multi_org_denial_world;
pub mod multi_org_fixture;
pub mod worker_pipeline;

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
use fileconv_server::services::download::CapabilityKeys;
use fileconv_server::state::RuntimeState;
use fileconv_server::storage::minio::{MinioClient, ObjectIdentityMeta};
use fileconv_server::storage::qdrant::{QdrantAdminApiKey, QdrantAdminClient, QdrantClient};
use std::path::PathBuf;
use tokio_postgres::NoTls;
use uuid::Uuid;

/// Serialize env-mutating tests within one integration-test binary so parallel
/// `#[test]` functions cannot interleave `set_var`/`remove_var` windows.
pub fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Restores prior process env values on drop; pair with [`test_env_lock`].
pub struct SavedEnvVars {
    vars: Vec<(String, Option<String>)>,
}

impl SavedEnvVars {
    pub fn save(names: &[&str]) -> Self {
        let vars = names
            .iter()
            .map(|name| ((*name).to_string(), std::env::var(name).ok()))
            .collect();
        Self { vars }
    }
}

impl Drop for SavedEnvVars {
    fn drop(&mut self) {
        for (name, value) in &self.vars {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

/// When `MARKHAND_E2E=1`, soft-skips are forbidden — missing live deps must panic.
pub fn markhand_e2e_required() -> bool {
    std::env::var("MARKHAND_E2E").ok().as_deref() == Some("1")
}

/// Whether integration prerequisites must be live (CI gate or explicit E2E opt-in).
pub fn markhand_test_required() -> bool {
    std::env::var("MARKHAND_TEST_REQUIRED").ok().as_deref() == Some("1") || markhand_e2e_required()
}

/// Pass through `Some`, panic in required mode when missing, else soft-skip with stderr.
pub fn take_live<T>(value: Option<T>, name: &str) -> Option<T> {
    match value {
        Some(value) => Some(value),
        None if std::env::var("MARKHAND_TEST_REQUIRED").ok().as_deref() == Some("1") => {
            panic!("MARKHAND_TEST_REQUIRED=1 requires {name}");
        }
        None if markhand_e2e_required() => panic!("MARKHAND_E2E=1 requires {name}"),
        None => {
            eprintln!("skipped: {name} unset — integration test requires live dependency");
            None
        }
    }
}

fn non_empty_env(var: &str) -> Option<String> {
    std::env::var(var)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn non_empty_env_first(vars: &[&str]) -> Option<String> {
    vars.iter().find_map(|var| non_empty_env(var))
}

/// MinIO connection fields read from `MARKHAND_TEST_MINIO_*` (with object-store aliases).
#[derive(Clone, Debug)]
pub struct MinioTestCredentials {
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
}

fn minio_test_credentials_raw() -> Option<MinioTestCredentials> {
    let endpoint = non_empty_env_first(&[
        "MARKHAND_TEST_MINIO_ENDPOINT",
        "MARKHAND_TEST_OBJECT_STORE_ENDPOINT",
    ])?;
    let access_key = non_empty_env_first(&[
        "MARKHAND_TEST_MINIO_ACCESS_KEY",
        "MARKHAND_TEST_OBJECT_STORE_ACCESS_KEY",
    ])?;
    let secret_key = non_empty_env_first(&[
        "MARKHAND_TEST_MINIO_SECRET_KEY",
        "MARKHAND_TEST_OBJECT_STORE_SECRET_KEY",
    ])?;
    if access_key.is_empty() || secret_key.is_empty() {
        return None;
    }
    let region = non_empty_env_first(&[
        "MARKHAND_TEST_MINIO_REGION",
        "MARKHAND_TEST_OBJECT_STORE_REGION",
    ])
    .unwrap_or_else(|| "us-east-1".into());
    Some(MinioTestCredentials {
        endpoint,
        access_key,
        secret_key,
        region,
    })
}

/// Strict MinIO credentials for integration tests (`MARKHAND_TEST_MINIO_*`).
pub fn minio_test_credentials() -> Option<MinioTestCredentials> {
    take_live(minio_test_credentials_raw(), "MARKHAND_TEST_MINIO_*")
}

fn build_minio_client(creds: MinioTestCredentials, bucket: String) -> MinioClient {
    std::env::set_var("RUST_S3_SKIP_LOCATION_CONSTRAINT", "true");
    let config = MinioConfig::new(
        creds.endpoint,
        SecretString::new(creds.access_key),
        SecretString::new(creds.secret_key),
        bucket,
        creds.region,
        true,
    )
    .expect("minio config");
    MinioClient::from_config(&config).expect("minio client")
}

/// Live MinIO client with an ephemeral bucket (`markhand-it-*` prefix).
pub fn test_minio_client() -> Option<MinioClient> {
    test_minio_client_with_bucket_prefix("markhand-it")
}

/// Live MinIO client with an ephemeral bucket using the given prefix.
pub fn test_minio_client_with_bucket_prefix(prefix: &str) -> Option<MinioClient> {
    let creds = minio_test_credentials()?;
    let bucket = format!("{prefix}-{}", Uuid::new_v4().simple());
    Some(build_minio_client(creds, bucket))
}

pub fn admin_database_url() -> Option<String> {
    take_live(
        non_empty_env("MARKHAND_TEST_DATABASE_URL"),
        "MARKHAND_TEST_DATABASE_URL",
    )
}

pub fn app_database_url() -> Option<String> {
    take_live(
        non_empty_env("MARKHAND_TEST_APP_DATABASE_URL"),
        "MARKHAND_TEST_APP_DATABASE_URL",
    )
}

pub fn test_qdrant_url() -> Option<String> {
    take_live(
        non_empty_env("MARKHAND_TEST_QDRANT_URL"),
        "MARKHAND_TEST_QDRANT_URL",
    )
}

/// Live Qdrant client when `MARKHAND_TEST_QDRANT_URL` is configured.
pub fn test_qdrant_client() -> Option<QdrantClient> {
    let url = test_qdrant_url()?;
    let api_key = non_empty_env("MARKHAND_TEST_QDRANT_API_KEY").map(SecretString::new);
    Some(QdrantClient::with_api_key(url, api_key).expect("qdrant client"))
}

/// Qdrant admin client for collection cleanup in integration tests.
pub fn test_qdrant_admin_client() -> Option<QdrantAdminClient> {
    let url = test_qdrant_url()?;
    let key = non_empty_env("MARKHAND_TEST_QDRANT_ADMIN_API_KEY")
        .unwrap_or_else(|| "test-operator-admin-key".into());
    Some(
        QdrantAdminClient::new(
            url,
            QdrantAdminApiKey::new(SecretString::new(key)).expect("admin key"),
        )
        .expect("qdrant admin client"),
    )
}

/// `fileconv` binary used by ConvertWorker sandboxes.
pub fn fileconv_binary() -> Option<PathBuf> {
    let path = if let Ok(path) = std::env::var("MARKHAND_TEST_FILECONV_BIN") {
        PathBuf::from(path)
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/fileconv")
    };
    take_live(path.exists().then_some(path), "target/debug/fileconv")
}

/// `/usr/bin/python3` for sandbox integration tests.
pub fn python3_binary() -> Option<PathBuf> {
    let path = PathBuf::from("/usr/bin/python3");
    take_live(path.exists().then_some(path), "/usr/bin/python3")
}

/// Sandbox isolation available on the host (bubblewrap/firejail).
pub fn sandbox_isolation_available() -> Option<()> {
    take_live(
        match fileconv_server::workers::sandbox::preflight() {
            Ok(()) => Some(()),
            Err(_) => None,
        },
        "sandbox isolation (bubblewrap/firejail)",
    )
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

/// Bounded soak dimensions for [`minio_cleanup_soak_lane`].
pub const MINIO_CLEANUP_GUARD_SOAK_ROUNDS: usize = 3;
pub const MINIO_CLEANUP_GUARD_SOAK_CONCURRENCY: usize = 4;
pub const MINIO_CLEANUP_GUARD_SOAK_OBJECTS_PER_BUCKET: usize = 3;

/// Hermetic guardrail: soak must stay bounded and stress multi-object buckets.
pub fn assert_minio_cleanup_soak_params(rounds: usize, concurrency: usize, objects: usize) {
    assert!(
        (1..=8).contains(&rounds),
        "soak rounds must stay bounded, got {rounds}"
    );
    assert!(
        (2..=16).contains(&concurrency),
        "soak concurrency must expose cleanup races, got {concurrency}"
    );
    assert!(
        objects >= 2,
        "each bucket must hold multiple objects, got {objects}"
    );
}

/// Lane failure with round/slot/bucket identity for residual-bucket diagnosis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinioCleanupSoakLaneFailure {
    pub round: usize,
    pub slot: usize,
    pub bucket_name: String,
    pub error: fileconv_server::storage::StorageError,
}

impl std::fmt::Display for MinioCleanupSoakLaneFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "round {} slot {} bucket {} cleanup failed: {:?}",
            self.round, self.slot, self.bucket_name, self.error
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinioCleanupSoakLaneSuccess {
    pub round: usize,
    pub slot: usize,
    pub bucket_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MinioCleanupSoakLaneOutcome {
    Success(MinioCleanupSoakLaneSuccess),
    LaneFailed(MinioCleanupSoakLaneFailure),
    JoinFailed {
        round: usize,
        slot: usize,
        error: String,
    },
}

/// Await every spawned lane in a soak round before returning outcomes.
pub async fn collect_minio_cleanup_soak_round(
    round: usize,
    handles: Vec<(
        usize,
        tokio::task::JoinHandle<Result<String, MinioCleanupSoakLaneFailure>>,
    )>,
) -> Vec<MinioCleanupSoakLaneOutcome> {
    let mut outcomes = Vec::with_capacity(handles.len());
    for (slot, handle) in handles {
        outcomes.push(match handle.await {
            Ok(Ok(bucket_name)) => {
                MinioCleanupSoakLaneOutcome::Success(MinioCleanupSoakLaneSuccess {
                    round,
                    slot,
                    bucket_name,
                })
            }
            Ok(Err(failure)) => MinioCleanupSoakLaneOutcome::LaneFailed(failure),
            Err(join_error) => MinioCleanupSoakLaneOutcome::JoinFailed {
                round,
                slot,
                error: join_error.to_string(),
            },
        });
    }
    outcomes
}

/// Assert a fully drained soak round; reports every failing lane together.
pub fn assert_minio_cleanup_soak_round_succeeded(outcomes: &[MinioCleanupSoakLaneOutcome]) {
    let failures: Vec<String> = outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            MinioCleanupSoakLaneOutcome::Success(success) => {
                if success.bucket_name.starts_with("markhand-it-") {
                    None
                } else {
                    Some(format!(
                        "round {} slot {} unexpected bucket name: {}",
                        success.round, success.slot, success.bucket_name
                    ))
                }
            }
            MinioCleanupSoakLaneOutcome::LaneFailed(failure) => Some(failure.to_string()),
            MinioCleanupSoakLaneOutcome::JoinFailed { round, slot, error } => {
                Some(format!("round {round} slot {slot} join failed: {error}"))
            }
        })
        .collect();
    if !failures.is_empty() {
        panic!(
            "MinIO cleanup soak round had {} failing lane(s):\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}

fn minio_cleanup_soak_lane_failure(
    round: usize,
    slot: usize,
    bucket_name: &str,
    error: fileconv_server::storage::StorageError,
) -> MinioCleanupSoakLaneFailure {
    MinioCleanupSoakLaneFailure {
        round,
        slot,
        bucket_name: bucket_name.to_string(),
        error,
    }
}

/// One soak lane: unique bucket, multiple objects, explicit guard cleanup.
pub async fn minio_cleanup_soak_lane(
    objects_per_bucket: usize,
    round: usize,
    slot: usize,
) -> Result<String, MinioCleanupSoakLaneFailure> {
    assert_minio_cleanup_soak_params(1, 2, objects_per_bucket);
    let store = test_minio_client().expect("live soak lane requires MinIO env");
    let bucket_name = store.bucket_name().to_string();
    let guard = MinioCleanupGuard::new(store.clone());
    let org = Uuid::new_v4();
    store
        .ensure_bucket()
        .await
        .map_err(|error| minio_cleanup_soak_lane_failure(round, slot, &bucket_name, error))?;
    for object_index in 0..objects_per_bucket {
        let version_id = Uuid::new_v4();
        let key = trusted_key(org, version_id, Uuid::new_v4(), None).expect("trusted key");
        let payload = format!("soak-r{round}-s{slot}-o{object_index}");
        put_bytes(
            &store,
            org,
            &key,
            payload.as_bytes(),
            "text/plain",
            ObjectIdentityMeta {
                org_id: org,
                collection_id: None,
                document_id: None,
                version_id: Some(version_id),
                original_filename: Some(format!("soak-{object_index}.txt")),
                canonical_format: Some("txt".into()),
                content_sha256: Some(sha256_hex(payload.as_bytes())),
                content_length: Some(payload.len() as u64),
                disposition: Some("trusted".into()),
            },
        )
        .await;
    }
    guard
        .cleanup()
        .await
        .map_err(|error| minio_cleanup_soak_lane_failure(round, slot, &bucket_name, error))?;
    Ok(bucket_name)
}

/// Seed an org user with the given permission codes (owner role) + password.
///
/// Callers that need non-empty `allowed_collection_ids` under the 1C
/// `(qa.query, read)` projection must include `qa.query` in `permissions`.
/// Omit it only for fixtures that intentionally prove missing-query denial or
/// that never assert collection scope (member/org/auth-only tests).
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
    let capability_keys =
        CapabilityKeys::from_auth_signing_key(test_auth_config().signing_key.as_ref().unwrap())
            .expect("test capability keys");
    AppState::from_parts_with_store(runtime, pool, Some(auth), store)
        .expect("app state")
        .with_capability_keys(capability_keys)
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
