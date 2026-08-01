//! DB-gated resolver tests for Phase 1C `(qa.query, read)` collection projection.
//!
//! Skips cleanly when `MARKHAND_TEST_DATABASE_URL` / `MARKHAND_TEST_APP_DATABASE_URL`
//! are unset; runs in GitHub `rust-integration` with live PostgreSQL.

mod common;

use common::acl_fixture::{
    attempt_grant_group_access, attempt_grant_role_access, boot_acl_pool, group_grant_count,
    resolver_allowed_collection_ids, role_grant_count, seed_acl_collection_matrix, seed_acl_org,
    user_grant_count, PERMISSION_QA_QUERY,
};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::db::models::AccessLevel;
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::services::acl_mutate::revoke_collection_access_for_principal;

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

/// Migration 0036 rejects dormant group/role grants on `private` collections.
/// Integrated fail-closed: grant attempts roll back independently, counts stay
/// zero, and the member cannot resolve either private collection.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL and MARKHAND_TEST_APP_DATABASE_URL"]
async fn private_visibility_ignores_group_and_role_grants() {
    let Some((ephemeral, pool)) = boot_acl_pool().await else {
        return;
    };

    let fixture = seed_acl_org(&pool).await;
    let matrix = seed_acl_collection_matrix(&pool, &fixture).await;

    let group_err = attempt_grant_group_access(
        &pool,
        &fixture,
        matrix.private_group_leak,
        AccessLevel::Write,
    )
    .await
    .expect_err("group grant on private collection must be rejected");
    assert!(
        group_err.contains("visibility groups"),
        "unexpected group-grant rejection: {group_err}"
    );

    let role_err = attempt_grant_role_access(
        &pool,
        &fixture,
        matrix.private_role_leak,
        AccessLevel::Write,
    )
    .await
    .expect_err("role grant on private collection must be rejected");
    assert!(
        role_err.contains("visibility groups"),
        "unexpected role-grant rejection: {role_err}"
    );

    assert_eq!(
        group_grant_count(&pool, fixture.org, matrix.private_group_leak).await,
        0,
        "rejected group grant must not persist"
    );
    assert_eq!(
        role_grant_count(&pool, fixture.org, matrix.private_role_leak).await,
        0,
        "rejected role grant must not persist"
    );

    let allowed = resolver_allowed_collection_ids(&pool, fixture.org, fixture.member).await;
    assert!(
        !allowed.contains(&matrix.private_group_leak),
        "member must not resolve private collection after rejected group grant: {allowed:?}"
    );
    assert!(
        !allowed.contains(&matrix.private_role_leak),
        "member must not resolve private collection after rejected role grant: {allowed:?}"
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

    assert_eq!(
        group_grant_count(&pool, fixture.org, collection).await,
        1,
        "containment fixture must seed one group grant before revoke"
    );
    assert_eq!(
        role_grant_count(&pool, fixture.org, collection).await,
        1,
        "containment fixture must seed one role grant before revoke"
    );
    assert_eq!(
        user_grant_count(&pool, fixture.org, collection, fixture.member).await,
        1,
        "containment fixture must seed target member direct grant before revoke"
    );
    assert_eq!(
        user_grant_count(&pool, fixture.org, collection, fixture.other_user).await,
        1,
        "containment fixture must seed unrelated other-user direct grant before revoke"
    );

    let member_allowed_before =
        resolver_allowed_collection_ids(&pool, fixture.org, fixture.member).await;
    assert!(
        member_allowed_before.contains(&collection),
        "member must resolve containment before revoke; got {member_allowed_before:?}"
    );

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
