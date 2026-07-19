//! Live PostgreSQL tests for P1B-I02 atomic quota admission.
//!
//! Skips cleanly when `MARKHAND_TEST_DATABASE_URL` is unset. These tests use the
//! non-superuser app role and the same tenant transaction helper as production.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::database::apply_migrations;
use fileconv_server::db::models::{ReservationStatus, ResourceKind};
use fileconv_server::db::orgs;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::services::quota::{
    self, apply_quota_headers, QuotaDenial, QuotaError, QuotaSettlement, QuotaSnapshot,
};
use tokio_postgres::NoTls;
use uuid::Uuid;

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
        .unwrap_or_else(|error| panic!("connect failed for {database_url}: {error}"));
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
        let db_name = format!("markhand_quota_{}", Uuid::new_v4().simple());
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

async fn boot_pool(base_url: &str) -> (EphemeralDb, Pool) {
    let ephemeral = EphemeralDb::create(base_url).await;
    apply_migrations(&ephemeral.url)
        .await
        .expect("apply migrations");
    let pool = create_pool(&ephemeral.url).expect("pool");
    (ephemeral, pool)
}

fn ctx(org: Uuid, user: Uuid) -> OrgContext {
    OrgContext::try_new(org, user, ["doc.upload"], []).expect("ctx")
}

#[derive(Debug, Clone, Copy)]
struct QuotaFixture {
    storage: i64,
    documents: i32,
    concurrent: i32,
    tokens: i64,
}

async fn seed_org_with_quota(
    pool: &Pool,
    org: Uuid,
    user: Uuid,
    slug: &str,
    quota: QuotaFixture,
) -> OrgContext {
    let context = ctx(org, user);
    let slug = slug.to_string();
    with_org_txn(pool, &context, {
        let context = context.clone();
        move |txn| {
            Box::pin(async move {
                orgs::ensure_exists(txn, &context, &slug, &slug).await?;
                orgs::ensure_user(txn, &context, user, &format!("{slug}@example.test"), &slug)
                    .await?;
                orgs::ensure_membership(txn, &context).await?;
                txn.execute(
                    "INSERT INTO org_quotas (
                        org_id, max_storage_bytes, max_documents,
                        max_concurrent_jobs, max_monthly_tokens
                     )
                     VALUES ($1, $2, $3, $4, $5)",
                    &[
                        &org,
                        &quota.storage,
                        &quota.documents,
                        &quota.concurrent,
                        &quota.tokens,
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed org quota");
    context
}

async fn active_reserved(pool: &Pool, context: &OrgContext, kind: ResourceKind) -> i64 {
    with_org_txn(pool, context, {
        let context = context.clone();
        move |txn| {
            Box::pin(async move {
                let kind = kind.as_str();
                let row = txn
                    .query_one(
                        "SELECT COALESCE(SUM(amount), 0)::bigint
                         FROM quota_reservations
                         WHERE org_id = $1 AND resource_kind = $2
                           AND status = 'reserved' AND expires_at > now()",
                        &[&context.org_id(), &kind],
                    )
                    .await?;
                Ok(row.get::<_, i64>(0))
            })
        }
    })
    .await
    .expect("active reserved")
}

async fn counter_value(pool: &Pool, context: &OrgContext, key: &str) -> i64 {
    with_org_txn(pool, context, {
        let context = context.clone();
        let key = key.to_string();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT COALESCE(SUM(value), 0)::bigint
                         FROM usage_counters
                         WHERE org_id = $1 AND counter_key = $2",
                        &[&context.org_id(), &key],
                    )
                    .await?;
                Ok(row.get::<_, i64>(0))
            })
        }
    })
    .await
    .expect("counter value")
}

async fn reservation_count(pool: &Pool, context: &OrgContext, key: &str) -> i64 {
    with_org_txn(pool, context, {
        let context = context.clone();
        let key = key.to_string();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint
                         FROM quota_reservations
                         WHERE org_id = $1 AND reservation_key = $2",
                        &[&context.org_id(), &key],
                    )
                    .await?;
                Ok(row.get::<_, i64>(0))
            })
        }
    })
    .await
    .expect("reservation count")
}

async fn force_expire(pool: &Pool, context: &OrgContext, key: &str) {
    with_org_txn(pool, context, {
        let context = context.clone();
        let key = key.to_string();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE quota_reservations
                     SET expires_at = now() - interval '1 second'
                     WHERE org_id = $1 AND reservation_key = $2",
                    &[&context.org_id(), &key],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("force expire");
}

#[tokio::test]
async fn concurrent_reserve_does_not_over_reserve() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let pool = Arc::new(pool);
    let context = seed_org_with_quota(
        &pool,
        Uuid::new_v4(),
        Uuid::new_v4(),
        "quota-concurrent",
        QuotaFixture {
            storage: 5,
            documents: 100,
            concurrent: 10,
            tokens: 100,
        },
    )
    .await;

    let attempts = 16;
    let mut tasks = Vec::new();
    for idx in 0..attempts {
        let pool = Arc::clone(&pool);
        let context = context.clone();
        tasks.push(tokio::spawn(async move {
            quota::reserve(
                &pool,
                &context,
                &format!("concurrent-{idx}"),
                ResourceKind::StorageBytes,
                1,
                Duration::from_secs(60),
                None,
            )
            .await
        }));
    }

    let mut succeeded = 0;
    let mut denied = 0;
    for task in tasks {
        match task.await.expect("join") {
            Ok(_) => succeeded += 1,
            Err(QuotaError::QuotaExceeded(_)) => denied += 1,
            Err(error) => panic!("unexpected quota error: {error:?}"),
        }
    }
    assert_eq!(succeeded, 5);
    assert_eq!(denied, attempts - 5);
    assert_eq!(
        active_reserved(&pool, &context, ResourceKind::StorageBytes).await,
        5
    );

    ephemeral.drop().await;
}

#[tokio::test]
async fn terminal_settlement_is_idempotent_and_typed() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org_with_quota(
        &pool,
        Uuid::new_v4(),
        Uuid::new_v4(),
        "quota-settle",
        QuotaFixture {
            storage: 100,
            documents: 10,
            concurrent: 1,
            tokens: 100,
        },
    )
    .await;

    quota::reserve(
        &pool,
        &context,
        "finalize-once",
        ResourceKind::StorageBytes,
        7,
        Duration::from_secs(60),
        None,
    )
    .await
    .unwrap();
    assert!(matches!(
        quota::finalize(&pool, &context, "finalize-once")
            .await
            .unwrap(),
        QuotaSettlement::Finalized(_)
    ));
    assert!(matches!(
        quota::finalize(&pool, &context, "finalize-once")
            .await
            .unwrap(),
        QuotaSettlement::AlreadyFinalized(_)
    ));
    assert_eq!(counter_value(&pool, &context, "storage_bytes").await, 7);
    assert!(matches!(
        quota::refund(&pool, &context, "finalize-once")
            .await
            .unwrap(),
        QuotaSettlement::FinalizedCannotRefund(_)
    ));

    quota::reserve(
        &pool,
        &context,
        "refund-once",
        ResourceKind::StorageBytes,
        5,
        Duration::from_secs(60),
        None,
    )
    .await
    .unwrap();
    assert!(matches!(
        quota::refund(&pool, &context, "refund-once").await.unwrap(),
        QuotaSettlement::Refunded(_)
    ));
    assert!(matches!(
        quota::refund(&pool, &context, "refund-once").await.unwrap(),
        QuotaSettlement::AlreadyRefunded(_)
    ));
    assert_eq!(
        active_reserved(&pool, &context, ResourceKind::StorageBytes).await,
        0
    );
    assert!(matches!(
        quota::finalize(&pool, &context, "refund-once")
            .await
            .unwrap(),
        QuotaSettlement::RefundedCannotFinalize(_)
    ));

    quota::reserve(
        &pool,
        &context,
        "expire-once",
        ResourceKind::StorageBytes,
        5,
        Duration::from_secs(60),
        None,
    )
    .await
    .unwrap();
    force_expire(&pool, &context, "expire-once").await;
    assert_eq!(
        quota::expire_reserved(&pool, &context, 10).await.unwrap(),
        1
    );
    assert!(matches!(
        quota::finalize(&pool, &context, "expire-once")
            .await
            .unwrap(),
        QuotaSettlement::Expired(_)
    ));

    ephemeral.drop().await;
}

#[tokio::test]
async fn idempotency_key_retries_create_one_reservation() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let pool = Arc::new(pool);
    let context = seed_org_with_quota(
        &pool,
        Uuid::new_v4(),
        Uuid::new_v4(),
        "quota-idem",
        QuotaFixture {
            storage: 10,
            documents: 10,
            concurrent: 10,
            tokens: 10,
        },
    )
    .await;

    let first = quota::reserve(
        &pool,
        &context,
        "same-key",
        ResourceKind::StorageBytes,
        1,
        Duration::from_secs(60),
        None,
    )
    .await
    .unwrap();
    let second = quota::reserve(
        &pool,
        &context,
        "same-key",
        ResourceKind::StorageBytes,
        1,
        Duration::from_secs(60),
        None,
    )
    .await
    .unwrap();
    assert!(first.created);
    assert!(!second.created);
    assert_eq!(first.reservation.id, second.reservation.id);

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let pool = Arc::clone(&pool);
        let context = context.clone();
        tasks.push(tokio::spawn(async move {
            quota::reserve(
                &pool,
                &context,
                "same-key-concurrent",
                ResourceKind::StorageBytes,
                1,
                Duration::from_secs(60),
                None,
            )
            .await
            .expect("same-key reserve")
            .reservation
            .id
        }));
    }
    let mut ids = Vec::new();
    for task in tasks {
        ids.push(task.await.expect("join"));
    }
    assert!(ids.iter().all(|id| *id == ids[0]));
    assert_eq!(
        reservation_count(&pool, &context, "same-key-concurrent").await,
        1
    );

    ephemeral.drop().await;
}

#[tokio::test]
async fn mismatched_and_terminal_key_reuse_is_rejected() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org_with_quota(
        &pool,
        Uuid::new_v4(),
        Uuid::new_v4(),
        "quota-conflict",
        QuotaFixture {
            storage: 10,
            documents: 10,
            concurrent: 10,
            tokens: 10,
        },
    )
    .await;

    quota::reserve(
        &pool,
        &context,
        "reuse-conflict",
        ResourceKind::StorageBytes,
        2,
        Duration::from_secs(60),
        None,
    )
    .await
    .unwrap();
    assert!(matches!(
        quota::reserve(
            &pool,
            &context,
            "reuse-conflict",
            ResourceKind::StorageBytes,
            3,
            Duration::from_secs(60),
            None,
        )
        .await,
        Err(QuotaError::ReservationConflict)
    ));
    quota::refund(&pool, &context, "reuse-conflict")
        .await
        .unwrap();
    assert!(matches!(
        quota::reserve(
            &pool,
            &context,
            "reuse-conflict",
            ResourceKind::StorageBytes,
            2,
            Duration::from_secs(60),
            None,
        )
        .await,
        Err(QuotaError::ReservationConflict)
    ));

    ephemeral.drop().await;
}

#[tokio::test]
async fn concurrent_finalize_and_refund_do_not_double_apply() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let pool = Arc::new(pool);
    let context = seed_org_with_quota(
        &pool,
        Uuid::new_v4(),
        Uuid::new_v4(),
        "quota-race",
        QuotaFixture {
            storage: 10,
            documents: 10,
            concurrent: 10,
            tokens: 10,
        },
    )
    .await;
    quota::reserve(
        &pool,
        &context,
        "settle-race",
        ResourceKind::StorageBytes,
        4,
        Duration::from_secs(60),
        None,
    )
    .await
    .unwrap();

    let fin_pool = Arc::clone(&pool);
    let fin_ctx = context.clone();
    let finalize =
        tokio::spawn(async move { quota::finalize(&fin_pool, &fin_ctx, "settle-race").await });
    let refund_pool = Arc::clone(&pool);
    let refund_ctx = context.clone();
    let refund =
        tokio::spawn(async move { quota::refund(&refund_pool, &refund_ctx, "settle-race").await });

    let finalize = finalize.await.unwrap();
    let refund = refund.await.unwrap();
    let finalized = matches!(
        &finalize,
        Ok(QuotaSettlement::Finalized(_) | QuotaSettlement::AlreadyFinalized(_))
    ) || matches!(&refund, Ok(QuotaSettlement::FinalizedCannotRefund(_)));
    let refunded = matches!(
        &refund,
        Ok(QuotaSettlement::Refunded(_) | QuotaSettlement::AlreadyRefunded(_))
    ) || matches!(&finalize, Ok(QuotaSettlement::RefundedCannotFinalize(_)));
    assert_ne!(finalized, refunded, "exactly one terminal direction wins");
    assert!(
        counter_value(&pool, &context, "storage_bytes").await == 0
            || counter_value(&pool, &context, "storage_bytes").await == 4
    );

    ephemeral.drop().await;
}

#[tokio::test]
async fn upload_two_resource_settlement_is_atomic() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org_with_quota(
        &pool,
        Uuid::new_v4(),
        Uuid::new_v4(),
        "quota-upload-atomic",
        QuotaFixture {
            storage: 100,
            documents: 10,
            concurrent: 10,
            tokens: 10,
        },
    )
    .await;

    quota::reserve_upload(&pool, &context, "upload-atomic", 9, Duration::from_secs(60))
        .await
        .unwrap();
    // Force one member of the two-resource operation to be non-finalizable. The
    // finalize transaction must not partially increment the other counter.
    quota::refund(&pool, &context, "upload.documents.upload-atomic")
        .await
        .unwrap();
    assert!(matches!(
        quota::finalize_upload(&pool, &context, "upload-atomic").await,
        Err(QuotaError::RefundedCannotFinalize)
    ));
    assert_eq!(counter_value(&pool, &context, "storage_bytes").await, 0);
    assert_eq!(counter_value(&pool, &context, "documents").await, 0);
    assert_eq!(
        active_reserved(&pool, &context, ResourceKind::StorageBytes).await,
        9
    );

    quota::refund_upload(&pool, &context, "upload-atomic")
        .await
        .unwrap();
    assert_eq!(
        active_reserved(&pool, &context, ResourceKind::StorageBytes).await,
        0
    );

    ephemeral.drop().await;
}

#[tokio::test]
async fn concurrent_jobs_admission_respects_limit() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let pool = Arc::new(pool);
    let context = seed_org_with_quota(
        &pool,
        Uuid::new_v4(),
        Uuid::new_v4(),
        "quota-jobs",
        QuotaFixture {
            storage: 100,
            documents: 10,
            concurrent: 2,
            tokens: 10,
        },
    )
    .await;
    let mut tasks = Vec::new();
    for idx in 0..6 {
        let pool = Arc::clone(&pool);
        let context = context.clone();
        tasks.push(tokio::spawn(async move {
            quota::reserve(
                &pool,
                &context,
                &format!("job-{idx}"),
                ResourceKind::ConcurrentJobs,
                1,
                Duration::from_secs(60),
                None,
            )
            .await
        }));
    }
    let mut ok = 0;
    let mut denied = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(_) => ok += 1,
            Err(QuotaError::QuotaExceeded(_)) => denied += 1,
            Err(error) => panic!("unexpected error: {error:?}"),
        }
    }
    assert_eq!(ok, 2);
    assert_eq!(denied, 4);
    assert_eq!(
        active_reserved(&pool, &context, ResourceKind::ConcurrentJobs).await,
        2
    );

    ephemeral.drop().await;
}

#[tokio::test]
async fn expired_crash_reservation_does_not_block_and_sweeps() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org_with_quota(
        &pool,
        Uuid::new_v4(),
        Uuid::new_v4(),
        "quota-crash",
        QuotaFixture {
            storage: 1,
            documents: 10,
            concurrent: 10,
            tokens: 10,
        },
    )
    .await;

    quota::reserve(
        &pool,
        &context,
        "crashed",
        ResourceKind::StorageBytes,
        1,
        Duration::from_secs(60),
        None,
    )
    .await
    .unwrap();
    force_expire(&pool, &context, "crashed").await;
    quota::reserve(
        &pool,
        &context,
        "replacement",
        ResourceKind::StorageBytes,
        1,
        Duration::from_secs(60),
        None,
    )
    .await
    .expect("expired reservation must not block");
    assert_eq!(
        quota::expire_reserved(&pool, &context, 10).await.unwrap(),
        1
    );

    with_org_txn(&pool, &context, {
        let context = context.clone();
        move |txn| {
            Box::pin(async move {
                let status: String = txn
                    .query_one(
                        "SELECT status FROM quota_reservations
                         WHERE org_id = $1 AND reservation_key = 'crashed'",
                        &[&context.org_id()],
                    )
                    .await?
                    .get(0);
                assert_eq!(status, ReservationStatus::Expired.as_str());
                Ok(())
            })
        }
    })
    .await
    .unwrap();

    ephemeral.drop().await;
}

#[tokio::test]
async fn overflow_paths_reject_without_wrap_or_panic() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org_with_quota(
        &pool,
        Uuid::new_v4(),
        Uuid::new_v4(),
        "quota-overflow",
        QuotaFixture {
            storage: i64::MAX,
            documents: 10,
            concurrent: 10,
            tokens: 10,
        },
    )
    .await;

    assert!(matches!(
        quota::reserve(
            &pool,
            &context,
            "too-large",
            ResourceKind::StorageBytes,
            i64::MAX as u64 + 1,
            Duration::from_secs(60),
            None,
        )
        .await,
        Err(QuotaError::InvalidAmount)
    ));

    with_org_txn(&pool, &context, {
        let context = context.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO usage_counters (
                        org_id, counter_key, period_start, period_end, value
                     )
                     VALUES (
                        $1, 'storage_bytes',
                        timestamptz '1970-01-01 00:00:00+00',
                        timestamptz '9999-12-31 00:00:00+00',
                        $2
                     )",
                    &[&context.org_id(), &i64::MAX],
                )
                .await?;
                txn.execute(
                    "INSERT INTO quota_reservations (
                        org_id, reservation_key, resource_kind, amount, expires_at
                     )
                     VALUES ($1, 'overflow-finalize', 'storage_bytes', 1, now() + interval '1 hour')",
                    &[&context.org_id()],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        quota::finalize(&pool, &context, "overflow-finalize").await,
        Err(QuotaError::ArithmeticOverflow)
    ));

    ephemeral.drop().await;
}

#[tokio::test]
async fn quota_rows_are_org_scoped_by_predicate_and_rls() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let org_a = Uuid::new_v4();
    let org_b = Uuid::new_v4();
    let ctx_a = seed_org_with_quota(
        &pool,
        org_a,
        Uuid::new_v4(),
        "quota-iso-a",
        QuotaFixture {
            storage: 10,
            documents: 10,
            concurrent: 10,
            tokens: 10,
        },
    )
    .await;
    let ctx_b = seed_org_with_quota(
        &pool,
        org_b,
        Uuid::new_v4(),
        "quota-iso-b",
        QuotaFixture {
            storage: 10,
            documents: 10,
            concurrent: 10,
            tokens: 10,
        },
    )
    .await;

    quota::reserve(
        &pool,
        &ctx_a,
        "shared-name",
        ResourceKind::StorageBytes,
        3,
        Duration::from_secs(60),
        None,
    )
    .await
    .unwrap();
    quota::reserve(
        &pool,
        &ctx_b,
        "shared-name",
        ResourceKind::StorageBytes,
        4,
        Duration::from_secs(60),
        None,
    )
    .await
    .unwrap();
    quota::finalize(&pool, &ctx_b, "shared-name").await.unwrap();

    assert_eq!(
        active_reserved(&pool, &ctx_a, ResourceKind::StorageBytes).await,
        3
    );
    assert_eq!(counter_value(&pool, &ctx_a, "storage_bytes").await, 0);
    assert_eq!(counter_value(&pool, &ctx_b, "storage_bytes").await, 4);

    with_org_txn(&pool, &ctx_a, move |txn| {
        Box::pin(async move {
            let leaked: i64 = txn
                .query_one(
                    "SELECT count(*)::bigint FROM quota_reservations WHERE org_id = $1",
                    &[&org_b],
                )
                .await?
                .get(0);
            assert_eq!(leaked, 0, "RLS must hide org B quota rows from org A");
            Ok(())
        })
    })
    .await
    .unwrap();

    ephemeral.drop().await;
}

#[test]
fn quota_error_and_success_headers_are_resource_scoped() {
    let denial = QuotaError::QuotaExceeded(QuotaDenial {
        resource_kind: ResourceKind::ConcurrentJobs,
        limit: 2,
        committed: 0,
        active_reserved: 2,
        requested: 1,
        remaining: 0,
        retry_after_secs: Some(30),
    });
    let response = denial.into_response();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response.headers().get("x-quota-resource").unwrap(),
        "concurrent_jobs"
    );
    assert_eq!(response.headers().get("x-quota-limit").unwrap(), "2");
    assert_eq!(response.headers().get("x-quota-remaining").unwrap(), "0");
    assert_eq!(response.headers().get("retry-after").unwrap(), "30");

    let mut headers = axum::http::HeaderMap::new();
    apply_quota_headers(
        &mut headers,
        &QuotaSnapshot {
            resource_kind: ResourceKind::StorageBytes,
            limit: 10,
            committed: 3,
            active_reserved: 2,
            remaining: 5,
        },
    );
    assert_eq!(headers.get("x-quota-resource").unwrap(), "storage_bytes");
    assert_eq!(headers.get("x-quota-used").unwrap(), "3");
    assert_eq!(headers.get("x-quota-reserved").unwrap(), "2");
}
