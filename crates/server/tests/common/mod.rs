//! Shared helpers for DB-backed server integration tests.
//!
//! Dual-role layout:
//! - `MARKHAND_TEST_DATABASE_URL` — bootstrap role with `CREATEDB` (compose superuser)
//! - `MARKHAND_TEST_APP_DATABASE_URL` — non-superuser `markhand_app` for FORCE RLS
#![allow(dead_code)] // not every integration binary uses every helper

use deadpool_postgres::Pool;
use fileconv_server::database::apply_migrations;
use fileconv_server::db::pool::{create_pool, create_pool_with_max_size};
use tokio_postgres::NoTls;
use uuid::Uuid;

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
                 GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO markhand_app;
                 GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO markhand_app;
                 GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO markhand_app;",
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
