//! DB-gated resolver tests for Phase 1C `(qa.query, read)` collection projection.
//!
//! Skips cleanly when `MARKHAND_TEST_DATABASE_URL` / `MARKHAND_TEST_APP_DATABASE_URL`
//! are unset; runs in GitHub `rust-integration` with live PostgreSQL.

mod common;

use std::collections::BTreeSet;

use common::acl_fixture::{
    boot_acl_pool, expected_member_read_projection, group_grant_count, poc_resolver_projection,
    resolver_allowed_collection_ids, role_grant_count, seed_acl_collection_matrix, seed_acl_org,
    user_grant_count, PERMISSION_QA_QUERY,
};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::services::acl_mutate::revoke_collection_access_for_principal;
use uuid::Uuid;

/// Group grant on a `groups` collection must surface via the resolver even when
/// the member has no direct `collection_user_access` row.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_APP_DATABASE_URL"]
async fn groups_visibility_group_grant_allows_member_without_user_grant() {
    let Some((ephemeral, pool)) = boot_acl_pool().await else {
        return;
    };

    let fixture = seed_acl_org(&pool).await;
    let matrix = seed_acl_collection_matrix(&pool, &fixture).await;

    let allowed = resolver_allowed_collection_ids(&pool, fixture.org, fixture.member).await;
    assert!(
        allowed.contains(&matrix.groups_via_group),
        "member must see groups collection via group grant; got {allowed:?}"
    );

    ephemeral.drop().await;
}

/// Dormant group/role grants on `private` collections must not leak into the
/// resolver projection (private arm ignores non-user grants).
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_APP_DATABASE_URL"]
async fn private_visibility_ignores_group_and_role_grants() {
    let Some((ephemeral, pool)) = boot_acl_pool().await else {
        return;
    };

    let fixture = seed_acl_org(&pool).await;
    let matrix = seed_acl_collection_matrix(&pool, &fixture).await;

    let allowed = resolver_allowed_collection_ids(&pool, fixture.org, fixture.member).await;
    assert!(
        !allowed.contains(&matrix.private_group_leak),
        "private collection must not admit group grant: {allowed:?}"
    );
    assert!(
        !allowed.contains(&matrix.private_role_leak),
        "private collection must not admit role grant: {allowed:?}"
    );

    ephemeral.drop().await;
}

/// `revoke_collection_access_for_principal` must delete group/role grants,
/// revoke only the target user's direct grant, and leave other users' grants.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_APP_DATABASE_URL"]
async fn containment_removes_group_role_grants_but_preserves_other_user_grants() {
    let Some((ephemeral, pool)) = boot_acl_pool().await else {
        return;
    };

    let fixture = seed_acl_org(&pool).await;
    let matrix = seed_acl_collection_matrix(&pool, &fixture).await;
    let collection = matrix.containment;

    let owner_ctx =
        OrgContext::try_new(fixture.org, fixture.owner, [PERMISSION_QA_QUERY], []).unwrap();
    with_org_txn(&pool, &owner_ctx, {
        let fixture = fixture.clone();
        move |txn| {
            Box::pin(async move {
                revoke_collection_access_for_principal(
                    txn,
                    fixture.org,
                    fixture.member,
                    collection,
                    fixture.owner,
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("revoke collection access");

    assert_eq!(
        group_grant_count(&pool, fixture.org, collection).await,
        0,
        "group grants must be removed during containment revoke"
    );
    assert_eq!(
        role_grant_count(&pool, fixture.org, collection).await,
        0,
        "role grants must be removed during containment revoke"
    );
    assert_eq!(
        user_grant_count(&pool, fixture.org, collection, fixture.member).await,
        0,
        "target user's direct grant must be removed"
    );
    assert_eq!(
        user_grant_count(&pool, fixture.org, collection, fixture.other_user).await,
        1,
        "other user's direct grant must survive containment revoke"
    );

    let member_allowed = resolver_allowed_collection_ids(&pool, fixture.org, fixture.member).await;
    assert!(
        !member_allowed.contains(&collection),
        "revoked member must not retain collection access"
    );

    let other_allowed =
        resolver_allowed_collection_ids(&pool, fixture.org, fixture.other_user).await;
    assert!(
        other_allowed.contains(&collection),
        "other user must retain access via preserved direct grant; got {other_allowed:?}"
    );

    ephemeral.drop().await;
}

/// Hermetic contract pin: documents the RED gap between the POC resolver and
/// the `(qa.query, read)` projection SQL already ships. CI integration runs
/// prove the live resolver still matches the narrower POC set until Task 6 GREEN.
#[test]
fn poc_resolver_projection_is_narrower_than_qa_query_read_matrix() {
    let matrix = common::acl_fixture::AclCollectionMatrix {
        org_visible: Uuid::from_u128(1),
        private_owned: Uuid::from_u128(2),
        private_foreign: Uuid::from_u128(3),
        private_user_grant: Uuid::from_u128(4),
        private_group_leak: Uuid::from_u128(5),
        private_role_leak: Uuid::from_u128(6),
        groups_via_group: Uuid::from_u128(7),
        groups_via_role: Uuid::from_u128(8),
        groups_denied: Uuid::from_u128(9),
        groups_read_grant: Uuid::from_u128(10),
        containment: Uuid::from_u128(11),
    };
    let expected = expected_member_read_projection(&matrix);
    let poc = poc_resolver_projection(&matrix);
    let missing_from_poc: BTreeSet<_> = expected.difference(&poc).copied().collect();
    assert!(
        !missing_from_poc.is_empty(),
        "Task 6 RED: POC resolver must omit group/role-backed collections until wired; \
         missing from POC projection: {missing_from_poc:?}"
    );
    assert!(
        missing_from_poc.contains(&matrix.groups_via_group),
        "groups_via_group is the canonical RED case"
    );
}
