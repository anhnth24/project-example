//! DB-gated equivalence tests: resolver projection vs `allowed_collections_sql`.
//!
//! Skips cleanly when `MARKHAND_TEST_DATABASE_URL` / `MARKHAND_TEST_APP_DATABASE_URL`
//! are unset; runs in GitHub `rust-integration` with live PostgreSQL.

mod common;

use common::acl_fixture::{
    boot_acl_pool, expected_member_read_projection, resolver_allowed_collection_ids,
    seed_acl_collection_matrix, seed_acl_org, sql_allowed_collection_ids,
    sql_collection_access_exists, PERMISSION_QA_QUERY,
};
use fileconv_server::db::models::AccessLevel;

/// For every fixture state in the shared matrix, resolver `allowed_collection_ids`
/// must equal the `allowed_collections_sql(qa.query, read)` set.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_APP_DATABASE_URL"]
async fn resolver_matches_sql_predicate_for_acl_fixture_matrix() {
    let Some((ephemeral, pool)) = boot_acl_pool().await else {
        return;
    };

    let fixture = seed_acl_org(&pool).await;
    let matrix = seed_acl_collection_matrix(&pool, &fixture).await;

    let resolver_ids = resolver_allowed_collection_ids(&pool, fixture.org, fixture.member).await;
    let expected = expected_member_read_projection(&matrix);
    let sql_ids = sql_allowed_collection_ids(
        &pool,
        fixture.org,
        fixture.member,
        PERMISSION_QA_QUERY,
        AccessLevel::Read,
    )
    .await;

    assert_eq!(
        resolver_ids, expected,
        "resolver drift: member `(qa.query, read)` projection must match canonical fixture \
         matrix (org_visible, private_owned, private_user_grant, groups_via_group, \
         groups_via_role, groups_read_grant, containment); resolver={resolver_ids:?} \
         expected={expected:?}"
    );

    assert_eq!(
        resolver_ids, sql_ids,
        "resolver and SQL must agree on the full ACL fixture matrix"
    );

    assert_eq!(
        sql_ids, expected,
        "SQL projection must match canonical matrix expectations"
    );

    ephemeral.drop().await;
}

/// A `read` direct grant must satisfy the read projection but not write/admin guards.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_APP_DATABASE_URL"]
async fn read_grant_does_not_satisfy_write_or_admin() {
    let Some((ephemeral, pool)) = boot_acl_pool().await else {
        return;
    };

    let fixture = seed_acl_org(&pool).await;
    let matrix = seed_acl_collection_matrix(&pool, &fixture).await;
    let collection = matrix.groups_read_grant;

    let read_sql = sql_allowed_collection_ids(
        &pool,
        fixture.org,
        fixture.member,
        PERMISSION_QA_QUERY,
        AccessLevel::Read,
    )
    .await;
    assert!(
        read_sql.contains(&collection),
        "read grant must satisfy read projection"
    );

    let write_allowed = sql_collection_access_exists(
        &pool,
        fixture.org,
        fixture.member,
        collection,
        PERMISSION_QA_QUERY,
        AccessLevel::Write,
    )
    .await;
    assert!(
        !write_allowed,
        "read grant must not satisfy write required_access"
    );

    let admin_allowed = sql_collection_access_exists(
        &pool,
        fixture.org,
        fixture.member,
        collection,
        PERMISSION_QA_QUERY,
        AccessLevel::Admin,
    )
    .await;
    assert!(
        !admin_allowed,
        "read grant must not satisfy admin required_access"
    );

    ephemeral.drop().await;
}
