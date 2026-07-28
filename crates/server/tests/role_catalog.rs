//! Live PostgreSQL tests for the global RBAC catalog (1C-03, migrations/0030):
//! canonical matrix values, immutability, and "a new org needs no manual
//! seed" — the guarantee `POST /orgs` (`tests/orgs.rs`) relies on.
//!
//! Skips cleanly when `MARKHAND_TEST_DATABASE_URL` is unset. Must run in the
//! `rust-integration` CI job — not run in this session (no Postgres available
//! here).

mod common;

use common::admin_database_url;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::auth::permissions::resolve_org_context_in_txn;
use fileconv_server::database::apply_migrations;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use std::collections::BTreeSet;
use tokio_postgres::NoTls;
use uuid::Uuid;

// Mirrors `crates/server/tests/schema_migrations.rs`'s `EphemeralDb` shape —
// a throwaway database per test, dropped at the end.
struct EphemeralDb {
    url: String,
    admin_url: String,
    name: String,
}

impl EphemeralDb {
    async fn create(base_url: &str) -> Self {
        let name = format!("markhand_test_rc_{}", Uuid::new_v4().simple());
        let (admin_client, connection) = tokio_postgres::connect(base_url, NoTls)
            .await
            .expect("connect admin");
        tokio::spawn(async move {
            let _ = connection.await;
        });
        admin_client
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .await
            .expect("create ephemeral db");
        let url = rewrite_database_url(base_url, &name);
        apply_migrations(&url).await.expect("apply migrations");
        Self {
            url,
            admin_url: base_url.to_string(),
            name,
        }
    }

    async fn drop(self) {
        let (admin_client, connection) = tokio_postgres::connect(&self.admin_url, NoTls)
            .await
            .expect("connect admin for drop");
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let _ = admin_client
            .batch_execute(&format!(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
                self.name
            ))
            .await;
        let _ = admin_client
            .batch_execute(&format!("DROP DATABASE IF EXISTS {}", self.name))
            .await;
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

fn test_database_url() -> Option<String> {
    admin_database_url()
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn canonical_matrix_matches_the_current_poc_effective_matrix() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    let pool = create_pool(&ephemeral.url).expect("create pool");
    let client = pool.get().await.expect("client");

    let rows = client
        .query(
            "SELECT rcp.role_code, p.code
             FROM role_catalog_permissions rcp
             JOIN permissions p ON p.id = rcp.permission_id
             ORDER BY rcp.role_code, p.code",
            &[],
        )
        .await
        .expect("query catalog matrix");
    let mut by_role: std::collections::BTreeMap<String, BTreeSet<String>> = Default::default();
    for row in &rows {
        let role: String = row.get(0);
        let permission: String = row.get(1);
        by_role.entry(role).or_default().insert(permission);
    }

    let owner_admin_expected: BTreeSet<String> = [
        "doc.upload",
        "doc.delete",
        "doc.publish",
        "qa.query",
        "member.manage",
        "audit.view",
        "qa.history",
        "jobs.system",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let editor_expected: BTreeSet<String> = ["doc.upload", "doc.publish", "qa.query"]
        .into_iter()
        .map(String::from)
        .collect();
    let viewer_expected: BTreeSet<String> = ["qa.query"].into_iter().map(String::from).collect();

    assert_eq!(by_role.get("owner"), Some(&owner_admin_expected));
    assert_eq!(by_role.get("admin"), Some(&owner_admin_expected));
    assert_eq!(by_role.get("editor"), Some(&editor_expected));
    assert_eq!(by_role.get("viewer"), Some(&viewer_expected));
    // Deliberately NOT granted to any role — see migrations/0030 comment
    // (matches the POC org's current, unchanged behavior).
    for permissions in by_role.values() {
        assert!(!permissions.contains("doc.quarantine.review"));
    }

    // Cross-check: the POC org's OWN `role_permissions` (migrations/0011/
    // 0017/0019) resolve to exactly the same set the catalog now describes —
    // globalizing the catalog changed no existing behavior.
    let poc_org: Uuid = "11111111-1111-1111-1111-111111111111".parse().unwrap();
    let poc_owner: Uuid = "22222222-2222-2222-2222-222222222201".parse().unwrap();
    let ctx = resolve_org_context_in_txn(&pool, poc_org, poc_owner)
        .await
        .expect("POC owner must still resolve");
    let poc_permissions: BTreeSet<String> = ctx.permissions().iter().cloned().collect();
    assert_eq!(poc_permissions, owner_admin_expected);

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn role_catalog_rows_cannot_be_updated_or_deleted() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    let pool = create_pool(&ephemeral.url).expect("create pool");
    let client = pool.get().await.expect("client");

    // `tokio_postgres::Error`'s top-level `Display` for `Kind::Db` is always
    // the fixed literal "db error" (see tokio-postgres src/error/mod.rs) — it
    // deliberately never inlines the server's message. The actual text
    // (`role_catalog is immutable...`, raised by
    // `role_catalog_enforce_immutability()`) lives on the nested
    // `DbError` reachable via `as_db_error()`; asserting against
    // `update_err.to_string()` directly would never contain "immutable"
    // regardless of whether the trigger fired correctly.
    let update_err = client
        .execute(
            "UPDATE role_catalog SET name = 'Hacked' WHERE code = 'owner'",
            &[],
        )
        .await
        .expect_err("UPDATE on a system role must be rejected");
    let update_db_err = update_err
        .as_db_error()
        .expect("must be a genuine DB-level error, not a connection/protocol failure");
    assert!(
        update_db_err.message().contains("immutable"),
        "{update_db_err}"
    );

    let delete_err = client
        .execute("DELETE FROM role_catalog WHERE code = 'viewer'", &[])
        .await
        .expect_err("DELETE on a system role must be rejected");
    let delete_db_err = delete_err
        .as_db_error()
        .expect("must be a genuine DB-level error, not a connection/protocol failure");
    assert!(
        delete_db_err.message().contains("immutable"),
        "{delete_db_err}"
    );

    let perm_row = client
        .query_one(
            "SELECT permission_id FROM role_catalog_permissions WHERE role_code = 'viewer' LIMIT 1",
            &[],
        )
        .await
        .expect("viewer must have at least one grant");
    let permission_id: Uuid = perm_row.get(0);
    let grant_delete_err = client
        .execute(
            "DELETE FROM role_catalog_permissions WHERE role_code = 'viewer' AND permission_id = $1",
            &[&permission_id],
        )
        .await
        .expect_err("DELETE on a system role grant must be rejected");
    let grant_delete_db_err = grant_delete_err
        .as_db_error()
        .expect("must be a genuine DB-level error, not a connection/protocol failure");
    assert!(
        grant_delete_db_err.message().contains("immutable"),
        "{grant_delete_db_err}"
    );

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn a_new_org_resolves_full_owner_permissions_with_no_manual_seed() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    let pool = create_pool(&ephemeral.url).expect("create pool");

    let org_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let ctx = OrgContext::try_new(org_id, user_id, [] as [&str; 0], []).unwrap();

    // Deliberately NOT calling any `roles`/`role_permissions` seed helper —
    // only `orgs`/`users`/`org_memberships` (the bare minimum any org create
    // path needs) plus the one `provision_org_role_catalog` call `POST
    // /orgs` (`services::orgs::create_org`) makes. This is the acceptance
    // contract: an org gets full, correct RBAC from the global catalog alone.
    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                fileconv_server::db::orgs::ensure_exists(txn, &ctx, "new-org-no-seed", "New Org")
                    .await?;
                fileconv_server::db::orgs::ensure_user(
                    txn,
                    &ctx,
                    user_id,
                    "owner@new-org-no-seed.test",
                    "New Owner",
                )
                .await?;
                txn.execute("SELECT provision_org_role_catalog($1)", &[&ctx.org_id()])
                    .await?;
                fileconv_server::db::orgs::ensure_membership(txn, &ctx).await?;
                Ok(())
            })
        }
    })
    .await
    .expect("bootstrap new org with zero manual role seeding");

    let resolved = resolve_org_context_in_txn(&pool, org_id, user_id)
        .await
        .expect("owner must resolve permissions with no manual role/role_permissions seed");
    assert!(resolved.has_permission("member.manage"));
    assert!(resolved.has_permission("doc.upload"));
    assert!(resolved.has_permission("audit.view"));
    assert!(resolved.has_permission("jobs.system"));

    ephemeral.drop().await;
}
