//! Integration tests for OrgContext, repositories, RLS, and document state machine.
//!
//! Skips cleanly when dual-role URLs are unset. Uses the non-superuser
//! `markhand_app` role so FORCE RLS genuinely applies.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{
    admin_database_url, app_database_url, boot_app_pool, boot_app_pool_with_max_size,
    DualRoleEphemeralDb,
};
use deadpool_postgres::Pool;
use fileconv_server::auth::context::{OrgContext, OrgContextError};
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::documents::{self, NewDocument};
use fileconv_server::db::error::DbError;
use fileconv_server::db::models::{CollectionVisibility, DocumentState};
use fileconv_server::db::orgs;
use fileconv_server::db::pool::{apply_org_context, with_org_txn};
use fileconv_server::services::document_state;
use uuid::Uuid;

async fn boot_pool() -> Option<(DualRoleEphemeralDb, Pool)> {
    let admin = admin_database_url()?;
    let app = app_database_url()?;
    Some(boot_app_pool(&admin, &app).await)
}

async fn boot_pool_sized(max_size: usize) -> Option<(DualRoleEphemeralDb, Pool)> {
    let admin = admin_database_url()?;
    let app = app_database_url()?;
    Some(boot_app_pool_with_max_size(&admin, &app, max_size).await)
}

fn make_ctx(org: Uuid, user: Uuid) -> OrgContext {
    OrgContext::try_new(org, user, ["doc.upload"], []).expect("valid OrgContext")
}

async fn seed_org(pool: &Pool, org: Uuid, user: Uuid, slug: &str) -> OrgContext {
    let context = make_ctx(org, user);
    let slug_owned = slug.to_string();
    with_org_txn(pool, &context, {
        let owned = context.clone();
        move |txn| {
            Box::pin(async move {
                orgs::ensure_exists(txn, &owned, &slug_owned, &slug_owned).await?;
                orgs::ensure_user(
                    txn,
                    &owned,
                    user,
                    &format!("{slug_owned}@example.com"),
                    &slug_owned,
                )
                .await?;
                orgs::ensure_membership(txn, &owned).await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed org");
    context
}

async fn seed_collection(pool: &Pool, context: &OrgContext, slug: &str) -> Uuid {
    let id = Uuid::new_v4();
    let slug = slug.to_string();
    with_org_txn(pool, context, {
        let owned = context.clone();
        move |txn| {
            Box::pin(async move {
                collections::insert(
                    txn,
                    &owned,
                    NewCollection {
                        id,
                        name: &slug,
                        slug: &slug,
                        description: None,
                        visibility: CollectionVisibility::Org,
                    },
                )
                .await?;
                Ok(id)
            })
        }
    })
    .await
    .expect("seed collection")
}

#[test]
fn org_context_fail_closed_on_empty_scope() {
    // Typed API: every public repository method requires `&OrgContext`.
    // Construction rejects nil org/user so no usable context exists without scope.
    assert_eq!(
        OrgContext::try_new(Uuid::nil(), Uuid::new_v4(), [] as [&str; 0], []),
        Err(OrgContextError::MissingOrgId)
    );
    assert_eq!(
        OrgContext::try_new(Uuid::new_v4(), Uuid::nil(), [] as [&str; 0], []),
        Err(OrgContextError::MissingUserId)
    );
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn cross_org_deny_via_predicate_and_rls() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };

    let org_a = Uuid::new_v4();
    let user_a = Uuid::new_v4();
    let org_b = Uuid::new_v4();
    let user_b = Uuid::new_v4();

    let ctx_a = seed_org(&pool, org_a, user_a, "orga").await;
    let ctx_b = seed_org(&pool, org_b, user_b, "orgb").await;
    let collection_a = seed_collection(&pool, &ctx_a, "lib-a").await;
    let collection_b = seed_collection(&pool, &ctx_b, "lib-b").await;

    let doc_a = Uuid::new_v4();
    let doc_b = Uuid::new_v4();
    with_org_txn(&pool, &ctx_a, {
        let owned = ctx_a.clone();
        move |txn| {
            Box::pin(async move {
                documents::insert(
                    txn,
                    &owned,
                    NewDocument {
                        id: doc_a,
                        collection_id: collection_a,
                        title: "Doc A",
                    },
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .unwrap();
    with_org_txn(&pool, &ctx_b, {
        let owned = ctx_b.clone();
        move |txn| {
            Box::pin(async move {
                documents::insert(
                    txn,
                    &owned,
                    NewDocument {
                        id: doc_b,
                        collection_id: collection_b,
                        title: "Doc B",
                    },
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .unwrap();

    // App predicate: A's repository methods never see B.
    with_org_txn(&pool, &ctx_a, {
        let owned = ctx_a.clone();
        move |txn| {
            Box::pin(async move {
                assert!(matches!(
                    documents::get_by_id(txn, &owned, doc_b).await,
                    Err(DbError::NotFound)
                ));
                assert_eq!(documents::count(txn, &owned).await?, 1);
                assert!(matches!(
                    collections::get_by_id(txn, &owned, collection_b).await,
                    Err(DbError::NotFound)
                ));
                Ok(())
            })
        }
    })
    .await
    .unwrap();

    // RLS defense-in-depth: raw read for B's id under A's GUC returns zero rows.
    with_org_txn(&pool, &ctx_a, move |txn| {
        Box::pin(async move {
            let leaked: i64 = txn
                .query_one(
                    "SELECT count(*)::bigint FROM documents WHERE id = $1",
                    &[&doc_b],
                )
                .await?
                .get(0);
            assert_eq!(leaked, 0, "RLS must hide org B rows under org A context");
            let foreign_org: i64 = txn
                .query_one(
                    "SELECT count(*)::bigint FROM documents WHERE org_id = $1",
                    &[&org_b],
                )
                .await?
                .get(0);
            assert_eq!(foreign_org, 0);
            Ok(())
        })
    })
    .await
    .unwrap();

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn pool_does_not_leak_tenant_gucs() {
    // Max size 1 forces the same physical backend connection to be reused.
    let Some((ephemeral, pool)) = boot_pool_sized(1).await else {
        return;
    };

    let org_a = Uuid::new_v4();
    let user_a = Uuid::new_v4();
    let org_b = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    let ctx_a = seed_org(&pool, org_a, user_a, "leak-a").await;
    let ctx_b = seed_org(&pool, org_b, user_b, "leak-b").await;
    let collection_a = seed_collection(&pool, &ctx_a, "leak-lib-a").await;

    // --- Commit path: capture backend pid while GUCs are set, then prove reuse + clear ---
    let commit_pid: i32 = with_org_txn(&pool, &ctx_a, {
        let owned = ctx_a.clone();
        move |txn| {
            Box::pin(async move {
                documents::insert(
                    txn,
                    &owned,
                    NewDocument {
                        id: Uuid::new_v4(),
                        collection_id: collection_a,
                        title: "A only",
                    },
                )
                .await?;
                let setting: String = txn
                    .query_one("SELECT current_setting('app.org_id', true)", &[])
                    .await?
                    .get(0);
                assert_eq!(setting, org_a.to_string());
                let pid: i32 = txn.query_one("SELECT pg_backend_pid()", &[]).await?.get(0);
                Ok(pid)
            })
        }
    })
    .await
    .unwrap();

    let client = pool.get().await.expect("checkout after commit");
    let reuse_pid: i32 = client
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        reuse_pid, commit_pid,
        "pool max_size=1 must reuse the same backend after commit"
    );
    let leaked: Option<String> = client
        .query_one(
            "SELECT NULLIF(current_setting('app.org_id', true), '')",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(
        leaked.is_none(),
        "app.org_id must be empty after committed txn, got {leaked:?}"
    );
    drop(client);

    // --- Rollback path: forced Err must also clear GUCs on the same backend ---
    let rollback_pid = std::sync::Arc::new(std::sync::Mutex::new(None::<i32>));
    let rollback_pid_slot = std::sync::Arc::clone(&rollback_pid);
    let rolled_back: Result<(), DbError> = with_org_txn(&pool, &ctx_a, move |txn| {
        Box::pin(async move {
            let setting: String = txn
                .query_one("SELECT current_setting('app.org_id', true)", &[])
                .await?
                .get(0);
            assert_eq!(setting, org_a.to_string());
            let pid: i32 = txn.query_one("SELECT pg_backend_pid()", &[]).await?.get(0);
            *rollback_pid_slot.lock().expect("pid lock") = Some(pid);
            Err(DbError::Config("forced rollback".into()))
        })
    })
    .await;
    assert!(
        matches!(rolled_back, Err(DbError::Config(_))),
        "closure Err must take the rollback path: {rolled_back:?}"
    );
    let rollback_pid = rollback_pid
        .lock()
        .expect("pid lock")
        .expect("pid captured before forced Err");
    assert_eq!(
        rollback_pid, commit_pid,
        "rollback must also run on the single pooled backend"
    );

    let client = pool.get().await.expect("checkout after rollback");
    let reuse_pid: i32 = client
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        reuse_pid, rollback_pid,
        "pool max_size=1 must reuse the same backend after rollback"
    );
    let leaked: Option<String> = client
        .query_one(
            "SELECT NULLIF(current_setting('app.org_id', true), '')",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(
        leaked.is_none(),
        "app.org_id must be empty after rolled-back txn, got {leaked:?}"
    );
    drop(client);

    // Query under B cannot see A's rows (no residual A context on reused backend).
    with_org_txn(&pool, &ctx_b, {
        let owned = ctx_b.clone();
        move |txn| {
            Box::pin(async move {
                assert_eq!(documents::count(txn, &owned).await?, 0);
                let raw: i64 = txn
                    .query_one(
                        "SELECT count(*)::bigint FROM documents WHERE org_id = $1",
                        &[&org_a],
                    )
                    .await?
                    .get(0);
                assert_eq!(raw, 0);
                Ok(())
            })
        }
    })
    .await
    .unwrap();

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn document_state_machine_legal_illegal_and_concurrent() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };
    let pool = Arc::new(pool);

    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let context = seed_org(&pool, org, user, "state-org").await;
    let collection = seed_collection(&pool, &context, "state-lib").await;
    let document_id = Uuid::new_v4();

    with_org_txn(&pool, &context, {
        let owned = context.clone();
        move |txn| {
            Box::pin(async move {
                documents::insert(
                    txn,
                    &owned,
                    NewDocument {
                        id: document_id,
                        collection_id: collection,
                        title: "State Machine Doc",
                    },
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .unwrap();

    // Legal transition persists.
    let converted = document_state::transition(
        &pool,
        &context,
        document_id,
        DocumentState::Uploaded,
        DocumentState::Converting,
    )
    .await
    .expect("uploaded→converting");
    assert_eq!(converted.state, DocumentState::Converting);

    let converted = document_state::transition(
        &pool,
        &context,
        document_id,
        DocumentState::Converting,
        DocumentState::Converted,
    )
    .await
    .expect("converting→converted");
    assert_eq!(converted.state, DocumentState::Converted);

    // Illegal transition rejected.
    let illegal = document_state::transition(
        &pool,
        &context,
        document_id,
        DocumentState::Converted,
        DocumentState::Indexed,
    )
    .await;
    assert!(
        matches!(illegal, Err(DbError::IllegalTransition { .. })),
        "{illegal:?}"
    );
    with_org_txn(&pool, &context, {
        let owned = context.clone();
        move |txn| {
            Box::pin(async move {
                let doc = documents::get_by_id(txn, &owned, document_id).await?;
                assert_eq!(doc.state, DocumentState::Converted);
                Ok(())
            })
        }
    })
    .await
    .unwrap();

    // failed→indexing is illegal (no failure-stage provenance).
    document_state::transition(
        &pool,
        &context,
        document_id,
        DocumentState::Converted,
        DocumentState::Failed,
    )
    .await
    .unwrap();
    let bad_retry = document_state::transition(
        &pool,
        &context,
        document_id,
        DocumentState::Failed,
        DocumentState::Indexing,
    )
    .await;
    assert!(matches!(bad_retry, Err(DbError::IllegalTransition { .. })));
    document_state::transition(
        &pool,
        &context,
        document_id,
        DocumentState::Failed,
        DocumentState::Converting,
    )
    .await
    .expect("failed→converting retry");
    document_state::transition(
        &pool,
        &context,
        document_id,
        DocumentState::Converting,
        DocumentState::Converted,
    )
    .await
    .unwrap();

    // Advance to indexing for concurrency test.
    document_state::transition(
        &pool,
        &context,
        document_id,
        DocumentState::Converted,
        DocumentState::Indexing,
    )
    .await
    .unwrap();

    // True lock contention: holder acquires FOR UPDATE and HOLDS it while the
    // challenger blocks inside its own transition. Without FOR UPDATE the
    // challenger would finish before release and both could succeed.
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();

    let pool_holder = Arc::clone(&pool);
    let ctx_holder = context.clone();
    let holder = tokio::spawn(async move {
        let mut client = pool_holder.get().await.expect("holder checkout");
        let txn = client.transaction().await.expect("holder txn");
        apply_org_context(&txn, &ctx_holder)
            .await
            .expect("holder GUC");
        documents::get_by_id_for_update(&txn, &ctx_holder, document_id)
            .await
            .expect("holder FOR UPDATE");
        let _ = locked_tx.send(());
        release_rx.await.expect("release signal");
        let doc = document_state::apply_transition(
            &txn,
            &ctx_holder,
            document_id,
            DocumentState::Indexing,
            DocumentState::Indexed,
        )
        .await
        .expect("holder transition");
        txn.commit().await.expect("holder commit");
        doc
    });

    locked_rx.await.expect("lock acquired");

    let pool_challenger = Arc::clone(&pool);
    let ctx_challenger = context.clone();
    let challenger = tokio::spawn(async move {
        document_state::transition(
            &pool_challenger,
            &ctx_challenger,
            document_id,
            DocumentState::Indexing,
            DocumentState::Indexed,
        )
        .await
    });

    // While the holder still holds FOR UPDATE, the challenger must be blocked.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !challenger.is_finished(),
        "challenger must block on FOR UPDATE while holder retains the row lock; \
         if this passes without FOR UPDATE the contention test is invalid"
    );

    release_tx.send(()).expect("release holder");
    let holder_doc = holder.await.expect("holder join");
    assert_eq!(holder_doc.state, DocumentState::Indexed);

    let challenger_res = challenger.await.expect("challenger join");
    assert!(
        matches!(challenger_res, Err(DbError::StaleState { .. })),
        "loser must observe updated state as stale: {challenger_res:?}"
    );

    with_org_txn(&pool, &context, {
        let owned = context.clone();
        move |txn| {
            Box::pin(async move {
                let doc = documents::get_by_id(txn, &owned, document_id).await?;
                assert_eq!(doc.state, DocumentState::Indexed);
                Ok(())
            })
        }
    })
    .await
    .unwrap();

    // Tombstone → purge path.
    document_state::transition(
        &pool,
        &context,
        document_id,
        DocumentState::Indexed,
        DocumentState::Tombstoned,
    )
    .await
    .unwrap();
    let purged = document_state::transition(
        &pool,
        &context,
        document_id,
        DocumentState::Tombstoned,
        DocumentState::Purged,
    )
    .await
    .unwrap();
    assert_eq!(purged.state, DocumentState::Purged);
    let terminal = document_state::transition(
        &pool,
        &context,
        document_id,
        DocumentState::Purged,
        DocumentState::Uploaded,
    )
    .await;
    assert!(matches!(terminal, Err(DbError::IllegalTransition { .. })));

    ephemeral.drop().await;
}
