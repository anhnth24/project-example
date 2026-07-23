#![allow(clippy::await_holding_lock)]
//! P1B-O01 deploy-role exact tests: fresh migrator ownership + legacy app-owned upgrade.
//!
//! Requires `MARKHAND_TEST_DATABASE_URL` (superuser/bootstrap) and
//! `MARKHAND_TEST_APP_DATABASE_URL` (markhand_app).

mod common;

use common::{
    admin_database_url, app_database_url, connect_raw, drop_database_force, rewrite_database_url,
};
use fileconv_server::database::apply_migrations;
use std::sync::Mutex;
use uuid::Uuid;

fn role_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn rewrite_user(base_url: &str, user: &str, password: &str) -> Option<String> {
    let (scheme, rest) = base_url.split_once("://")?;
    let (_old_userinfo, hostpart) = rest.split_once('@')?;
    let (without_query, query) = match hostpart.split_once('?') {
        Some((head, tail)) => (head, Some(tail)),
        None => (hostpart, None),
    };
    let url = format!("{scheme}://{user}:{password}@{without_query}");
    Some(match query {
        Some(q) => format!("{url}?{q}"),
        None => url,
    })
}

async fn ensure_roles(admin_db_url: &str, migrator_password: &str, app_password: &str) {
    let _guard = role_lock();
    let client = connect_raw(admin_db_url).await;
    let mig_pass = migrator_password.replace('\'', "''");
    let app_pass = app_password.replace('\'', "''");
    client
        .batch_execute(&format!(
            "CREATE EXTENSION IF NOT EXISTS pgcrypto;
             DO $$ BEGIN
               IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'markhand_migrator') THEN
                 CREATE ROLE markhand_migrator LOGIN PASSWORD '{mig_pass}'
                   NOSUPERUSER NOCREATEDB NOCREATEROLE INHERIT;
               ELSE
                 ALTER ROLE markhand_migrator WITH INHERIT;
               END IF;
               IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'markhand_app') THEN
                 CREATE ROLE markhand_app LOGIN PASSWORD '{app_pass}'
                   NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
               END IF;
             END $$;
             GRANT USAGE, CREATE ON SCHEMA public TO markhand_migrator;
             GRANT USAGE ON SCHEMA public TO markhand_app;
             REVOKE CREATE ON SCHEMA public FROM markhand_app;
             ALTER DEFAULT PRIVILEGES FOR ROLE markhand_migrator IN SCHEMA public
               GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO markhand_app;
             ALTER DEFAULT PRIVILEGES FOR ROLE markhand_migrator IN SCHEMA public
               GRANT USAGE, SELECT ON SEQUENCES TO markhand_app;
             ALTER DEFAULT PRIVILEGES FOR ROLE markhand_migrator IN SCHEMA public
               GRANT EXECUTE ON FUNCTIONS TO markhand_app;"
        ))
        .await
        .expect("ensure migrator/app roles");
}

fn app_password_from_url(app_url: &str) -> String {
    app_url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('@').next())
        .and_then(|userinfo| userinfo.split_once(':').map(|(_, p)| p.to_string()))
        .unwrap_or_else(|| "markhand_app".into())
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn fresh_volume_migrator_owns_audit_app_cannot_mutate_schema() {
    let Some(admin) = admin_database_url() else {
        return;
    };
    let Some(app_base) = app_database_url() else {
        return;
    };
    let db_name = format!("markhand_deploy_fresh_{}", Uuid::new_v4().simple());
    let admin_maintenance = rewrite_database_url(&admin, "postgres");
    let admin_client = connect_raw(&admin_maintenance).await;
    admin_client
        .batch_execute(&format!("CREATE DATABASE \"{db_name}\""))
        .await
        .expect("create fresh db");

    let admin_db = rewrite_database_url(&admin, &db_name);
    let mig_password = std::env::var("MARKHAND_MIGRATOR_DB_PASSWORD")
        .unwrap_or_else(|_| "markhand_migrator_test".into());
    let app_password = app_password_from_url(&app_base);
    ensure_roles(&admin_db, &mig_password, &app_password).await;
    admin_client
        .batch_execute(&format!(
            "GRANT CONNECT ON DATABASE \"{db_name}\" TO markhand_migrator;
             GRANT CONNECT ON DATABASE \"{db_name}\" TO markhand_app;"
        ))
        .await
        .expect("grant connect");

    let migrator_url =
        rewrite_user(&admin_db, "markhand_migrator", &mig_password).expect("migrator url");
    assert!(
        !migrator_url
            .to_ascii_lowercase()
            .contains("://markhand_app"),
        "migrator URL must not use markhand_app"
    );
    // Set a known password so the rewritten URL authenticates.
    {
        let _guard = role_lock();
        let admin_on_db = connect_raw(&admin_db).await;
        let pass = mig_password.replace('\'', "''");
        admin_on_db
            .batch_execute(&format!("ALTER ROLE markhand_migrator PASSWORD '{pass}'"))
            .await
            .expect("set migrator password");
    }
    apply_migrations(&migrator_url)
        .await
        .expect("fresh migrate must use dedicated migrator credentials");

    let admin_on_db = connect_raw(&admin_db).await;
    admin_on_db
        .batch_execute(
            "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO markhand_app;
             GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO markhand_app;
             GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO markhand_app;
             REVOKE UPDATE, DELETE, TRUNCATE ON TABLE audit_log FROM markhand_app;
             GRANT SELECT, INSERT ON TABLE audit_log TO markhand_app;
             REVOKE CREATE ON SCHEMA public FROM markhand_app;",
        )
        .await
        .expect("grant app DML after migrator schema");

    let owner: String = admin_on_db
        .query_one(
            "SELECT pg_get_userbyid(c.relowner)
             FROM pg_class c
             JOIN pg_namespace n ON n.oid = c.relnamespace
             WHERE n.nspname = 'public' AND c.relname = 'audit_log'",
            &[],
        )
        .await
        .expect("owner query")
        .get(0);
    assert_eq!(
        owner, "markhand_migrator",
        "fresh volume audit_log must be owned by markhand_migrator, got {owner}"
    );

    let app_url = rewrite_database_url(&app_base, &db_name);
    let app = connect_raw(&app_url).await;
    assert!(
        app.batch_execute("CREATE TABLE o01_app_must_not_create (id int)")
            .await
            .is_err(),
        "app must not CREATE TABLE"
    );
    assert!(
        app.batch_execute("ALTER TABLE audit_log ADD COLUMN o01_leak text")
            .await
            .is_err(),
        "app must not ALTER audit_log"
    );
    assert!(
        app.batch_execute("UPDATE audit_log SET outcome = 'deny' WHERE false")
            .await
            .is_err(),
        "app must not UPDATE audit_log"
    );
    assert!(
        app.batch_execute("DELETE FROM audit_log WHERE false")
            .await
            .is_err(),
        "app must not DELETE audit_log"
    );

    drop(app);
    drop(admin_on_db);
    drop_database_force(&admin_maintenance, &db_name)
        .await
        .expect("cleanup fresh db");
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn legacy_app_owned_audit_transfers_via_migrator_upgrade() {
    let Some(admin) = admin_database_url() else {
        return;
    };
    let Some(app_base) = app_database_url() else {
        return;
    };
    let db_name = format!("markhand_deploy_legacy_{}", Uuid::new_v4().simple());
    let admin_maintenance = rewrite_database_url(&admin, "postgres");
    let admin_client = connect_raw(&admin_maintenance).await;
    admin_client
        .batch_execute(&format!("CREATE DATABASE \"{db_name}\""))
        .await
        .expect("create legacy db");
    let admin_db = rewrite_database_url(&admin, &db_name);
    let mig_password = std::env::var("MARKHAND_MIGRATOR_DB_PASSWORD")
        .unwrap_or_else(|_| "markhand_migrator_test".into());
    let app_password = app_password_from_url(&app_base);
    ensure_roles(&admin_db, &mig_password, &app_password).await;
    admin_client
        .batch_execute(&format!(
            "GRANT CONNECT ON DATABASE \"{db_name}\" TO markhand_migrator;
             GRANT CONNECT ON DATABASE \"{db_name}\" TO markhand_app;"
        ))
        .await
        .expect("grant connect");

    // Legacy path: schema applied under bootstrap, then objects reassigned to app.
    apply_migrations(&admin_db)
        .await
        .expect("bootstrap migrate");
    let admin_on_db = connect_raw(&admin_db).await;
    admin_on_db
        .batch_execute(
            "ALTER TABLE audit_log OWNER TO markhand_app;
             DO $$ BEGIN
               BEGIN
                 ALTER FUNCTION audit_log_enforce_immutability() OWNER TO markhand_app;
               EXCEPTION WHEN undefined_function THEN NULL;
               END;
               BEGIN
                 ALTER FUNCTION audit_log_validate_insert() OWNER TO markhand_app;
               EXCEPTION WHEN undefined_function THEN NULL;
               END;
             END $$;",
        )
        .await
        .expect("simulate legacy app ownership");

    let owner_before: String = admin_on_db
        .query_one(
            "SELECT pg_get_userbyid(c.relowner)
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
             WHERE n.nspname = 'public' AND c.relname = 'audit_log'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(owner_before, "markhand_app");

    {
        let _guard = role_lock();
        let pass = mig_password.replace('\'', "''");
        admin_on_db
            .batch_execute(&format!(
                "ALTER ROLE markhand_migrator WITH INHERIT PASSWORD '{pass}';
                 GRANT markhand_app TO markhand_migrator WITH INHERIT TRUE;
                 GRANT USAGE, CREATE ON SCHEMA public TO markhand_migrator;
                 GRANT ALL ON ALL TABLES IN SCHEMA public TO markhand_migrator;
                 GRANT ALL ON ALL FUNCTIONS IN SCHEMA public TO markhand_migrator;
                 DELETE FROM markhand_schema_migrations
                 WHERE name = '0028_expand_audit_ownership_migrator.sql';"
            ))
            .await
            .expect("grant migrator reassignment rights");
    }

    let migrator_url =
        rewrite_user(&admin_db, "markhand_migrator", &mig_password).expect("migrator url");
    apply_migrations(&migrator_url)
        .await
        .unwrap_or_else(|error| {
            panic!("legacy upgrade via migrator must transfer ownership: {error}")
        });

    let owner_after: String = admin_on_db
        .query_one(
            "SELECT pg_get_userbyid(c.relowner)
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
             WHERE n.nspname = 'public' AND c.relname = 'audit_log'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_ne!(
        owner_after, "markhand_app",
        "0028 must transfer audit_log ownership away from markhand_app (got {owner_after})"
    );
    assert_eq!(owner_after, "markhand_migrator");

    for fn_name in [
        "audit_log_enforce_immutability",
        "audit_log_validate_insert",
    ] {
        let fn_owner: String = admin_on_db
            .query_one(
                "SELECT pg_get_userbyid(p.proowner)
                 FROM pg_proc p
                 JOIN pg_namespace n ON n.oid = p.pronamespace
                 WHERE n.nspname = 'public' AND p.proname = $1
                 LIMIT 1",
                &[&fn_name],
            )
            .await
            .unwrap_or_else(|error| panic!("{fn_name} owner query: {error}"))
            .get(0);
        assert_ne!(
            fn_owner, "markhand_app",
            "{fn_name} must not remain owned by markhand_app (got {fn_owner})"
        );
    }

    let grants: i64 = admin_on_db
        .query_one(
            "SELECT COUNT(*)::bigint FROM information_schema.role_table_grants
             WHERE grantee = 'markhand_app' AND table_name = 'audit_log'
               AND privilege_type IN ('UPDATE', 'DELETE', 'TRUNCATE', 'TRIGGER', 'REFERENCES')",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        grants, 0,
        "app must have SELECT+INSERT only on audit_log after 0028"
    );

    let app_url = rewrite_database_url(&app_base, &db_name);
    let app = connect_raw(&app_url).await;
    assert!(
        app.batch_execute("ALTER TABLE audit_log DISABLE TRIGGER ALL")
            .await
            .is_err(),
        "app must not alter audit triggers after upgrade"
    );

    drop(app);
    drop(admin_on_db);
    drop_database_force(&admin_maintenance, &db_name)
        .await
        .expect("cleanup legacy db");
}
