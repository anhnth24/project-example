//! Live PostgreSQL tests for P1B-I03 durable jobs, outbox, and event log.
//!
//! Skips cleanly when `MARKHAND_TEST_DATABASE_URL` is unset. These tests use the
//! non-superuser app role and production org-scoped transaction helper.

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::database::apply_migrations;
use fileconv_server::db::error::DbError;
use fileconv_server::db::models::{Job, JobStatus, JobType};
use fileconv_server::db::orgs;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::jobs::{
    self, CancelOutcome, CheckpointPayload, EnqueueJob, EventLogOutboxSink, EventPayload, JobError,
    JobPayload, OutboxSink, CURRENT_EVENT_PAYLOAD_VERSION, CURRENT_JOB_PAYLOAD_VERSION,
    MAX_LEASE_TOKEN_LEN, MAX_WORKER_ID_LEN,
};
use serde_json::json;
use tokio::sync::Barrier;
use tokio_postgres::{NoTls, Row};
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
        let db_name = format!("markhand_jobs_{}", Uuid::new_v4().simple());
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

async fn seed_org(pool: &Pool, org: Uuid, user: Uuid, slug: &str) -> OrgContext {
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
                Ok(())
            })
        }
    })
    .await
    .expect("seed org");
    context
}

fn enqueue_spec(key: &str) -> EnqueueJob {
    EnqueueJob::new(JobType::Convert, JobPayload::default(), key)
}

async fn enqueue_one(pool: &Pool, context: &OrgContext, key: &str) -> Job {
    jobs::enqueue(pool, context, enqueue_spec(key))
        .await
        .expect("enqueue")
        .job
}

async fn enqueue_with_attempts(
    pool: &Pool,
    context: &OrgContext,
    key: &str,
    max_attempts: u32,
) -> Job {
    let mut input = enqueue_spec(key);
    input.max_attempts = max_attempts;
    jobs::enqueue(pool, context, input)
        .await
        .expect("enqueue")
        .job
}

async fn get_job(pool: &Pool, context: &OrgContext, job_id: Uuid) -> Job {
    with_org_txn(pool, context, {
        let context = context.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_opt(
                        "SELECT id, org_id, job_type, status, payload_version, payload,
                                attempts, max_attempts, lease_owner, lease_expires_at,
                                heartbeat_at, checkpoint, idempotency_key, document_id,
                                version_id, available_at, started_at, finished_at,
                                last_error, created_at, updated_at
                         FROM jobs
                         WHERE org_id = $1 AND id = $2",
                        &[&context.org_id(), &job_id],
                    )
                    .await?
                    .ok_or(DbError::NotFound)?;
                map_job(&row)
            })
        }
    })
    .await
    .expect("get job")
}

fn map_job(row: &Row) -> Result<Job, DbError> {
    let job_type: String = row.get("job_type");
    let status: String = row.get("status");
    Ok(Job {
        id: row.get("id"),
        org_id: row.get("org_id"),
        job_type: JobType::parse(&job_type).map_err(DbError::Config)?,
        status: JobStatus::parse(&status).map_err(DbError::Config)?,
        payload_version: row.get("payload_version"),
        payload: row.get("payload"),
        attempts: row.get("attempts"),
        max_attempts: row.get("max_attempts"),
        lease_owner: row.get("lease_owner"),
        lease_expires_at: row.get("lease_expires_at"),
        heartbeat_at: row.get("heartbeat_at"),
        checkpoint: row.get("checkpoint"),
        idempotency_key: row.get("idempotency_key"),
        document_id: row.get("document_id"),
        version_id: row.get("version_id"),
        available_at: row.get("available_at"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
        last_error: row.get("last_error"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn lease_token(job: &Job) -> &str {
    job.lease_owner.as_deref().expect("claimed job has token")
}

async fn table_counts(pool: &Pool, context: &OrgContext) -> (i64, i64, i64) {
    with_org_txn(pool, context, {
        let context = context.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT
                            (SELECT count(*)::bigint FROM jobs WHERE org_id = $1),
                            (SELECT count(*)::bigint FROM outbox_events WHERE org_id = $1),
                            (SELECT count(*)::bigint FROM event_log WHERE org_id = $1)",
                        &[&context.org_id()],
                    )
                    .await?;
                Ok((row.get(0), row.get(1), row.get(2)))
            })
        }
    })
    .await
    .expect("table counts")
}

async fn force_expire(pool: &Pool, context: &OrgContext, job_id: Uuid) {
    with_org_txn(pool, context, {
        let context = context.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE jobs
                     SET lease_expires_at = clock_timestamp() - interval '1 second'
                     WHERE org_id = $1 AND id = $2",
                    &[&context.org_id(), &job_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("force expire");
}

async fn force_available(pool: &Pool, context: &OrgContext, job_id: Uuid) {
    with_org_txn(pool, context, {
        let context = context.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "UPDATE jobs
                     SET available_at = clock_timestamp() - interval '1 second'
                     WHERE org_id = $1 AND id = $2",
                    &[&context.org_id(), &job_id],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("force available");
}

async fn unpublished_outbox_count(pool: &Pool, context: &OrgContext) -> i64 {
    with_org_txn(pool, context, {
        let context = context.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint
                         FROM outbox_events
                         WHERE org_id = $1 AND published_at IS NULL",
                        &[&context.org_id()],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("unpublished count")
}

async fn event_count(pool: &Pool, context: &OrgContext, event_type: &str) -> i64 {
    with_org_txn(pool, context, {
        let context = context.clone();
        let event_type = event_type.to_string();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint
                         FROM event_log
                         WHERE org_id = $1 AND event_type = $2",
                        &[&context.org_id(), &event_type],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("event count")
}

struct FailingOnceSink {
    failed: AtomicBool,
    delegate: EventLogOutboxSink,
}

impl Default for FailingOnceSink {
    fn default() -> Self {
        Self {
            failed: AtomicBool::new(false),
            delegate: EventLogOutboxSink,
        }
    }
}

impl OutboxSink for FailingOnceSink {
    fn publish<'a>(
        &'a self,
        txn: &'a tokio_postgres::Transaction<'_>,
        ctx: &'a OrgContext,
        event: &'a fileconv_server::db::models::OutboxEvent,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<fileconv_server::db::models::EventLogEntry, JobError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            if !self.failed.swap(true, Ordering::SeqCst) {
                return Err(JobError::Database(DbError::Config(
                    "intentional sink failure".into(),
                )));
            }
            self.delegate.publish(txn, ctx, event).await
        })
    }
}

/// Always fails for one specific job's outbox event, delegates for the rest.
struct PoisonJobSink {
    poison_job_id: Uuid,
    delegate: EventLogOutboxSink,
}

impl PoisonJobSink {
    fn new(poison_job_id: Uuid) -> Self {
        Self {
            poison_job_id,
            delegate: EventLogOutboxSink,
        }
    }
}

impl OutboxSink for PoisonJobSink {
    fn publish<'a>(
        &'a self,
        txn: &'a tokio_postgres::Transaction<'_>,
        ctx: &'a OrgContext,
        event: &'a fileconv_server::db::models::OutboxEvent,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<fileconv_server::db::models::EventLogEntry, JobError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            if event.job_id == Some(self.poison_job_id) {
                return Err(JobError::Database(DbError::Config(
                    "intentional poison event".into(),
                )));
            }
            self.delegate.publish(txn, ctx, event).await
        })
    }
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn enqueue_and_outbox_are_atomic_and_rollback_together() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org(&pool, Uuid::new_v4(), Uuid::new_v4(), "jobs-atomic").await;

    let outcome = jobs::enqueue(&pool, &context, enqueue_spec("atomic-ok"))
        .await
        .expect("enqueue");
    assert!(outcome.created);
    assert_eq!(table_counts(&pool, &context).await, (1, 1, 0));

    let payload = JobPayload::default().to_json().expect("payload");
    let event_payload = EventPayload {
        job_id: Some(Uuid::new_v4()),
        document_id: None,
        version_id: None,
        outbox_event_id: None,
    }
    .to_json()
    .expect("event payload");
    let forced: Result<(), DbError> = with_org_txn(&pool, &context, {
        let context = context.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn.query_one("SELECT clock_timestamp()", &[]).await?;
                let now: chrono::DateTime<chrono::Utc> = row.get(0);
                let job_id = Uuid::new_v4();
                let outbox_key = format!("job.enqueued:{job_id}");
                let job_type = JobType::Convert.as_str();
                txn.execute(
                    "INSERT INTO jobs (
                        id, org_id, job_type, payload_version, payload, max_attempts,
                        idempotency_key, document_id, version_id, available_at
                     )
                     VALUES ($1, $2, $3, $4, $5, $6, $7, NULL, NULL, $8)",
                    &[
                        &job_id,
                        &context.org_id(),
                        &job_type,
                        &CURRENT_JOB_PAYLOAD_VERSION,
                        &payload,
                        &5_i32,
                        &"forced-rollback",
                        &now,
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO outbox_events (
                        org_id, event_type, payload_version, payload, idempotency_key, job_id
                     )
                     VALUES ($1, 'job.enqueued', $2, $3, $4, $5)",
                    &[
                        &context.org_id(),
                        &CURRENT_EVENT_PAYLOAD_VERSION,
                        &event_payload,
                        &outbox_key,
                        &job_id,
                    ],
                )
                .await?;
                Err(DbError::Config("forced rollback".into()))
            })
        }
    })
    .await;
    assert!(forced.is_err());
    assert_eq!(table_counts(&pool, &context).await, (1, 1, 0));

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn duplicate_enqueue_returns_existing_job_and_single_outbox() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org(&pool, Uuid::new_v4(), Uuid::new_v4(), "jobs-dup").await;

    let first = jobs::enqueue(&pool, &context, enqueue_spec("dup-key"))
        .await
        .expect("first enqueue");
    let second = jobs::enqueue(&pool, &context, enqueue_spec("dup-key"))
        .await
        .expect("duplicate enqueue");

    assert!(first.created);
    assert!(!second.created);
    assert_eq!(first.job.id, second.job.id);
    assert_eq!(table_counts(&pool, &context).await, (1, 1, 0));

    ephemeral.drop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn concurrent_duplicate_enqueue_same_key_creates_one_job_and_one_outbox() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org(&pool, Uuid::new_v4(), Uuid::new_v4(), "jobs-dup-concurrent").await;
    let pool = Arc::new(pool);
    let barrier = Arc::new(Barrier::new(16));
    let mut handles = Vec::new();
    for _ in 0..16 {
        let pool = Arc::clone(&pool);
        let context = context.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            jobs::enqueue(&pool, &context, enqueue_spec("dup-concurrent"))
                .await
                .expect("enqueue")
                .job
                .id
        }));
    }

    let mut ids = Vec::new();
    for handle in handles {
        ids.push(handle.await.expect("enqueue task"));
    }
    let unique: HashSet<_> = ids.into_iter().collect();
    assert_eq!(unique.len(), 1);
    assert_eq!(table_counts(&pool, &context).await, (1, 1, 0));

    if let Ok(pool) = Arc::try_unwrap(pool) {
        pool.close();
    }
    ephemeral.drop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn concurrent_claimers_do_not_double_claim_with_skip_locked() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org(&pool, Uuid::new_v4(), Uuid::new_v4(), "jobs-claim").await;
    for index in 0..20 {
        enqueue_one(&pool, &context, &format!("claim-{index}")).await;
    }

    let pool = Arc::new(pool);
    let barrier = Arc::new(Barrier::new(8));
    let mut handles = Vec::new();
    for worker in 0..8 {
        let pool = Arc::clone(&pool);
        let context = context.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            jobs::claim(
                &pool,
                &context,
                &format!("claimer-{worker}"),
                4,
                Duration::from_secs(60),
            )
            .await
            .expect("claim")
            .into_iter()
            .map(|job| job.id)
            .collect::<Vec<_>>()
        }));
    }

    let mut all_claimed = Vec::new();
    for handle in handles {
        all_claimed.extend(handle.await.expect("claimer task"));
    }
    let unique: HashSet<_> = all_claimed.iter().copied().collect();
    assert_eq!(all_claimed.len(), unique.len(), "double-claimed job id");
    assert_eq!(unique.len(), 20);

    if let Ok(pool) = Arc::try_unwrap(pool) {
        pool.close();
    }
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn reclaim_expired_leases_preserves_live_heartbeat_and_dead_letters_exhausted() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org(&pool, Uuid::new_v4(), Uuid::new_v4(), "jobs-reclaim").await;

    let reclaimable = enqueue_with_attempts(&pool, &context, "reclaimable", 3).await;
    let exhausted = enqueue_with_attempts(&pool, &context, "exhausted", 1).await;
    let live = enqueue_with_attempts(&pool, &context, "live", 3).await;
    let claimed = jobs::claim(&pool, &context, "worker", 3, Duration::from_secs(60))
        .await
        .expect("claim");
    assert_eq!(claimed.len(), 3);
    let live_claim = claimed
        .iter()
        .find(|claimed| claimed.id == live.id)
        .expect("live claim")
        .clone();
    force_expire(&pool, &context, reclaimable.id).await;
    force_expire(&pool, &context, exhausted.id).await;
    jobs::heartbeat(
        &pool,
        &context,
        live.id,
        lease_token(&live_claim),
        live_claim.attempts,
        Duration::from_secs(60),
    )
    .await
    .expect("heartbeat");

    let reclaimed = jobs::reclaim_expired(&pool, &context, 10, Duration::from_secs(5))
        .await
        .expect("reclaim");
    let reclaimed_ids: HashSet<_> = reclaimed.iter().map(|job| job.id).collect();
    assert!(reclaimed_ids.contains(&reclaimable.id));
    assert!(reclaimed_ids.contains(&exhausted.id));
    assert!(!reclaimed_ids.contains(&live.id));

    let reclaimable = get_job(&pool, &context, reclaimable.id).await;
    let exhausted = get_job(&pool, &context, exhausted.id).await;
    let live = get_job(&pool, &context, live.id).await;
    assert_eq!(reclaimable.status, JobStatus::Pending);
    assert_eq!(exhausted.status, JobStatus::DeadLetter);
    assert_eq!(live.status, JobStatus::Leased);

    ephemeral.drop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn heartbeat_vs_reclaim_race_has_one_winner_and_no_lost_update() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org(&pool, Uuid::new_v4(), Uuid::new_v4(), "jobs-heartbeat-race").await;
    let job = enqueue_with_attempts(&pool, &context, "heartbeat-race", 3).await;
    let claim = jobs::claim(&pool, &context, "race-worker", 1, Duration::from_secs(60))
        .await
        .expect("claim")
        .remove(0);
    force_expire(&pool, &context, job.id).await;

    let pool = Arc::new(pool);
    let barrier = Arc::new(Barrier::new(2));
    let heartbeat_pool = Arc::clone(&pool);
    let heartbeat_ctx = context.clone();
    let heartbeat_barrier = Arc::clone(&barrier);
    let token = lease_token(&claim).to_string();
    let attempts = claim.attempts;
    let heartbeat = tokio::spawn(async move {
        heartbeat_barrier.wait().await;
        jobs::heartbeat(
            &heartbeat_pool,
            &heartbeat_ctx,
            job.id,
            &token,
            attempts,
            Duration::from_secs(60),
        )
        .await
    });

    let reclaim_pool = Arc::clone(&pool);
    let reclaim_ctx = context.clone();
    let reclaim_barrier = Arc::clone(&barrier);
    let reclaim = tokio::spawn(async move {
        reclaim_barrier.wait().await;
        jobs::reclaim_expired(&reclaim_pool, &reclaim_ctx, 10, Duration::from_secs(1)).await
    });

    let heartbeat_result = heartbeat.await.expect("heartbeat task");
    let reclaimed = reclaim.await.expect("reclaim task").expect("reclaim");
    let stored = get_job(&pool, &context, job.id).await;
    match heartbeat_result {
        Ok(()) => {
            assert!(reclaimed.is_empty());
            assert_eq!(stored.status, JobStatus::Leased);
            assert_eq!(stored.lease_owner.as_deref(), Some(lease_token(&claim)));
        }
        Err(JobError::LeaseLost) => {
            assert_eq!(reclaimed.len(), 1);
            assert_eq!(stored.status, JobStatus::Pending);
            assert!(stored.lease_owner.is_none());
        }
        other => panic!("unexpected heartbeat result: {other:?}"),
    }

    if let Ok(pool) = Arc::try_unwrap(pool) {
        pool.close();
    }
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn stale_same_worker_reclaim_reclaim_token_cannot_mutate_new_lease() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org(&pool, Uuid::new_v4(), Uuid::new_v4(), "jobs-stale-token").await;
    let job = enqueue_with_attempts(&pool, &context, "stale-token", 3).await;
    let first = jobs::claim(&pool, &context, "same-worker", 1, Duration::from_secs(60))
        .await
        .expect("claim")
        .remove(0);
    let old_token = lease_token(&first).to_string();
    let old_attempts = first.attempts;
    force_expire(&pool, &context, job.id).await;
    jobs::reclaim_expired(&pool, &context, 10, Duration::from_secs(1))
        .await
        .expect("reclaim");
    force_available(&pool, &context, job.id).await;
    let second = jobs::claim(&pool, &context, "same-worker", 1, Duration::from_secs(60))
        .await
        .expect("reclaim claim")
        .remove(0);
    assert_eq!(second.id, job.id);
    assert_ne!(lease_token(&second), old_token);
    assert_ne!(second.attempts, old_attempts);

    assert!(matches!(
        jobs::heartbeat(
            &pool,
            &context,
            job.id,
            &old_token,
            old_attempts,
            Duration::from_secs(60),
        )
        .await,
        Err(JobError::LeaseLost)
    ));
    assert!(matches!(
        jobs::checkpoint(
            &pool,
            &context,
            job.id,
            &old_token,
            old_attempts,
            CheckpointPayload::default(),
        )
        .await,
        Err(JobError::LeaseLost)
    ));
    assert!(matches!(
        jobs::complete(&pool, &context, job.id, &old_token, old_attempts).await,
        Err(JobError::LeaseLost)
    ));
    assert!(matches!(
        jobs::fail(&pool, &context, job.id, &old_token, old_attempts, "stale").await,
        Err(JobError::LeaseLost)
    ));
    let stored = get_job(&pool, &context, job.id).await;
    assert_eq!(stored.status, JobStatus::Leased);
    assert_eq!(stored.lease_owner.as_deref(), Some(lease_token(&second)));

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn checkpoint_survives_kill_reclaim_and_resume_claim() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org(&pool, Uuid::new_v4(), Uuid::new_v4(), "jobs-checkpoint").await;
    let job = enqueue_one(&pool, &context, "checkpoint").await;
    let claimed = jobs::claim(&pool, &context, "worker-a", 1, Duration::from_secs(60))
        .await
        .expect("claim");
    let claim = claimed.first().expect("claimed job").clone();

    let checkpoint = CheckpointPayload {
        cursor_id: Some(Uuid::new_v4()),
        completed_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
        staged_object_keys: vec![],
        offset: Some(7),
    };
    let checkpoint_json = checkpoint.to_json().expect("checkpoint json");
    jobs::checkpoint(
        &pool,
        &context,
        job.id,
        lease_token(&claim),
        claim.attempts,
        checkpoint,
    )
    .await
    .expect("checkpoint");
    force_expire(&pool, &context, job.id).await;
    jobs::reclaim_expired(&pool, &context, 10, Duration::from_secs(1))
        .await
        .expect("reclaim");
    force_available(&pool, &context, job.id).await;
    let resumed = jobs::claim(&pool, &context, "worker-b", 1, Duration::from_secs(60))
        .await
        .expect("resume claim");

    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0].id, job.id);
    assert_eq!(resumed[0].checkpoint, Some(checkpoint_json));
    assert_eq!(resumed[0].attempts, 2);

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn max_length_worker_id_yields_usable_lease_tokens_for_all_worker_mutations() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org(
        &pool,
        Uuid::new_v4(),
        Uuid::new_v4(),
        "jobs-worker-boundary",
    )
    .await;
    let worker_id = "w".repeat(MAX_WORKER_ID_LEN);

    let heartbeat_job = enqueue_with_attempts(&pool, &context, "max-worker-heartbeat", 3).await;
    let heartbeat_claim = jobs::claim(&pool, &context, &worker_id, 1, Duration::from_secs(60))
        .await
        .expect("claim heartbeat")
        .remove(0);
    let heartbeat_token = lease_token(&heartbeat_claim);
    assert_eq!(heartbeat_token.len(), MAX_LEASE_TOKEN_LEN);
    jobs::heartbeat(
        &pool,
        &context,
        heartbeat_job.id,
        heartbeat_token,
        heartbeat_claim.attempts,
        Duration::from_secs(60),
    )
    .await
    .expect("heartbeat max token");

    let checkpoint_job = enqueue_with_attempts(&pool, &context, "max-worker-checkpoint", 3).await;
    let checkpoint_claim = jobs::claim(&pool, &context, &worker_id, 1, Duration::from_secs(60))
        .await
        .expect("claim checkpoint")
        .remove(0);
    let checkpoint = jobs::checkpoint(
        &pool,
        &context,
        checkpoint_job.id,
        lease_token(&checkpoint_claim),
        checkpoint_claim.attempts,
        CheckpointPayload {
            cursor_id: Some(Uuid::new_v4()),
            completed_ids: vec![Uuid::new_v4()],
            staged_object_keys: vec![],
            offset: Some(1),
        },
    )
    .await
    .expect("checkpoint max token");
    assert!(checkpoint.checkpoint.is_some());

    let complete_job = enqueue_with_attempts(&pool, &context, "max-worker-complete", 3).await;
    let complete_claim = jobs::claim(&pool, &context, &worker_id, 1, Duration::from_secs(60))
        .await
        .expect("claim complete")
        .remove(0);
    let completed = jobs::complete(
        &pool,
        &context,
        complete_job.id,
        lease_token(&complete_claim),
        complete_claim.attempts,
    )
    .await
    .expect("complete max token");
    assert_eq!(completed.status, JobStatus::Succeeded);

    let fail_job = enqueue_with_attempts(&pool, &context, "max-worker-fail", 3).await;
    let fail_claim = jobs::claim(&pool, &context, &worker_id, 1, Duration::from_secs(60))
        .await
        .expect("claim fail")
        .remove(0);
    let failed = jobs::fail(
        &pool,
        &context,
        fail_job.id,
        lease_token(&fail_claim),
        fail_claim.attempts,
        "transient",
    )
    .await
    .expect("fail max token");
    assert_eq!(failed.status, JobStatus::Pending);

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn over_limit_worker_id_is_rejected_before_claiming() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org(
        &pool,
        Uuid::new_v4(),
        Uuid::new_v4(),
        "jobs-worker-overlimit",
    )
    .await;
    let job = enqueue_one(&pool, &context, "worker-overlimit").await;
    let worker_id = "w".repeat(MAX_WORKER_ID_LEN + 1);

    assert!(matches!(
        jobs::claim(&pool, &context, &worker_id, 1, Duration::from_secs(60)).await,
        Err(JobError::InvalidLeaseOwner)
    ));
    let stored = get_job(&pool, &context, job.id).await;
    assert_eq!(stored.status, JobStatus::Pending);
    assert_eq!(stored.attempts, 0);
    assert!(stored.lease_owner.is_none());

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn non_owner_checkpoint_is_rejected_without_mutating_checkpoint() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org(
        &pool,
        Uuid::new_v4(),
        Uuid::new_v4(),
        "jobs-checkpoint-guard",
    )
    .await;
    let job = enqueue_one(&pool, &context, "checkpoint-guard").await;
    let claim = jobs::claim(&pool, &context, "owner", 1, Duration::from_secs(60))
        .await
        .expect("claim")
        .remove(0);
    assert!(matches!(
        jobs::checkpoint(
            &pool,
            &context,
            job.id,
            "not-the-token",
            claim.attempts,
            CheckpointPayload {
                cursor_id: Some(Uuid::new_v4()),
                completed_ids: vec![],
                staged_object_keys: vec![],
                offset: Some(1),
            },
        )
        .await,
        Err(JobError::LeaseLost)
    ));
    let stored = get_job(&pool, &context, job.id).await;
    assert_eq!(stored.checkpoint, None);

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn fail_retries_with_future_backoff_then_dead_letters_when_exhausted() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org(&pool, Uuid::new_v4(), Uuid::new_v4(), "jobs-retry").await;
    let job = enqueue_with_attempts(&pool, &context, "retry", 2).await;
    let claimed = jobs::claim(&pool, &context, "worker", 1, Duration::from_secs(60))
        .await
        .expect("claim");
    let first_claim = claimed.first().expect("first claim").clone();

    let retry = jobs::fail(
        &pool,
        &context,
        job.id,
        lease_token(&first_claim),
        first_claim.attempts,
        "transient",
    )
    .await
    .expect("fail retry");
    assert_eq!(retry.status, JobStatus::Pending);
    assert!(retry.available_at > retry.updated_at);
    assert_eq!(retry.last_error.as_deref(), Some("transient"));

    force_available(&pool, &context, job.id).await;
    let claimed = jobs::claim(&pool, &context, "worker", 1, Duration::from_secs(60))
        .await
        .expect("claim retry");
    let second_claim = claimed.first().expect("second claim").clone();
    let dead = jobs::fail(
        &pool,
        &context,
        job.id,
        lease_token(&second_claim),
        second_claim.attempts,
        "exhausted",
    )
    .await
    .expect("fail dead");
    assert_eq!(dead.status, JobStatus::DeadLetter);
    assert!(dead.finished_at.is_some());

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn cancel_is_idempotent_and_owner_guards_in_flight_mutations() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org(&pool, Uuid::new_v4(), Uuid::new_v4(), "jobs-cancel").await;

    let pending = enqueue_one(&pool, &context, "cancel-pending").await;
    assert!(matches!(
        jobs::cancel(&pool, &context, pending.id)
            .await
            .expect("cancel"),
        CancelOutcome::Cancelled(_)
    ));
    assert!(matches!(
        jobs::cancel(&pool, &context, pending.id)
            .await
            .expect("cancel again"),
        CancelOutcome::AlreadyCancelled(_)
    ));

    let leased = enqueue_one(&pool, &context, "cancel-leased").await;
    let leased_claimed = jobs::claim(&pool, &context, "owner", 1, Duration::from_secs(60))
        .await
        .expect("claim leased");
    let leased_claim = leased_claimed.first().expect("leased claim").clone();
    assert!(matches!(
        jobs::cancel(&pool, &context, leased.id)
            .await
            .expect("cancel leased"),
        CancelOutcome::Cancelled(_)
    ));
    assert!(matches!(
        jobs::heartbeat(
            &pool,
            &context,
            leased.id,
            lease_token(&leased_claim),
            leased_claim.attempts,
            Duration::from_secs(60),
        )
        .await,
        Err(JobError::LeaseLost)
    ));
    assert!(matches!(
        jobs::complete(
            &pool,
            &context,
            leased.id,
            lease_token(&leased_claim),
            leased_claim.attempts,
        )
        .await,
        Err(JobError::LeaseLost)
    ));
    assert!(matches!(
        jobs::fail(
            &pool,
            &context,
            leased.id,
            lease_token(&leased_claim),
            leased_claim.attempts,
            "cancelled worker tried to fail",
        )
        .await,
        Err(JobError::LeaseLost)
    ));
    assert_eq!(
        get_job(&pool, &context, leased.id).await.status,
        JobStatus::Cancelled
    );

    let guarded = enqueue_one(&pool, &context, "owner-guard").await;
    let guarded_claimed = jobs::claim(&pool, &context, "owner-a", 1, Duration::from_secs(60))
        .await
        .expect("claim guarded");
    let guarded_claim = guarded_claimed.first().expect("guarded claim").clone();
    assert!(matches!(
        jobs::complete(
            &pool,
            &context,
            guarded.id,
            "owner-b",
            guarded_claim.attempts
        )
        .await,
        Err(JobError::LeaseLost)
    ));
    assert!(matches!(
        jobs::fail(
            &pool,
            &context,
            guarded.id,
            "owner-b",
            guarded_claim.attempts,
            "nope"
        )
        .await,
        Err(JobError::LeaseLost)
    ));
    jobs::complete(
        &pool,
        &context,
        guarded.id,
        lease_token(&guarded_claim),
        guarded_claim.attempts,
    )
    .await
    .expect("complete owner");
    assert!(matches!(
        jobs::cancel(&pool, &context, guarded.id).await,
        Err(JobError::CannotCancelTerminal(JobStatus::Succeeded))
    ));

    ephemeral.drop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn concurrent_event_appends_are_per_org_gapless_and_unique() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org(&pool, Uuid::new_v4(), Uuid::new_v4(), "jobs-events").await;
    let pool = Arc::new(pool);
    let barrier = Arc::new(Barrier::new(32));
    let mut handles = Vec::new();
    for _ in 0..32 {
        let pool = Arc::clone(&pool);
        let context = context.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            jobs::append_event(
                &pool,
                &context,
                "test.concurrent",
                EventPayload {
                    job_id: None,
                    document_id: None,
                    version_id: None,
                    outbox_event_id: None,
                },
            )
            .await
            .expect("append event")
        }));
    }
    for handle in handles {
        handle.await.expect("event task");
    }

    let sequences = with_org_txn(&pool, &context, {
        let context = context.clone();
        move |txn| {
            Box::pin(async move {
                let rows = txn
                    .query(
                        "SELECT sequence_no
                         FROM event_log
                         WHERE org_id = $1
                         ORDER BY sequence_no",
                        &[&context.org_id()],
                    )
                    .await?;
                Ok(rows.into_iter().map(|row| row.get(0)).collect::<Vec<i64>>())
            })
        }
    })
    .await
    .expect("sequences");
    assert_eq!(sequences.len(), 32);
    assert_eq!(sequences, (1_i64..=32).collect::<Vec<_>>());

    if let Ok(pool) = Arc::try_unwrap(pool) {
        pool.close();
    }
    ephemeral.drop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn outbox_relay_is_concurrent_safe_and_replay_idempotent() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org(&pool, Uuid::new_v4(), Uuid::new_v4(), "jobs-outbox").await;
    for index in 0..20 {
        enqueue_one(&pool, &context, &format!("outbox-{index}")).await;
    }
    assert_eq!(unpublished_outbox_count(&pool, &context).await, 20);

    let pool = Arc::new(pool);
    let barrier = Arc::new(Barrier::new(4));
    let mut handles = Vec::new();
    for _ in 0..4 {
        let pool = Arc::clone(&pool);
        let context = context.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            jobs::relay_outbox(&pool, &context, 10)
                .await
                .expect("relay")
        }));
    }

    let mut published = Vec::new();
    for handle in handles {
        published.extend(handle.await.expect("relay task"));
    }
    let unique: HashSet<_> = published
        .iter()
        .map(|publication| publication.outbox.id)
        .collect();
    assert_eq!(published.len(), unique.len());
    assert_eq!(published.len(), 20);
    assert_eq!(unpublished_outbox_count(&pool, &context).await, 0);
    assert!(jobs::relay_outbox(&pool, &context, 10)
        .await
        .expect("replay")
        .is_empty());

    if let Ok(pool) = Arc::try_unwrap(pool) {
        pool.close();
    }
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn outbox_sink_failure_leaves_unpublished_and_retry_publishes_once() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org(&pool, Uuid::new_v4(), Uuid::new_v4(), "jobs-outbox-failure").await;
    enqueue_one(&pool, &context, "outbox-failure").await;
    assert_eq!(unpublished_outbox_count(&pool, &context).await, 1);

    let sink = Arc::new(FailingOnceSink::default());
    // A publish failure no longer aborts/errors the whole relay; the event is isolated
    // in its savepoint and left unpublished for the next pass.
    let first = jobs::relay_outbox_with_sink(&pool, &context, 10, &sink)
        .await
        .expect("relay tolerates a failing event");
    assert!(first.is_empty());
    assert_eq!(unpublished_outbox_count(&pool, &context).await, 1);
    assert_eq!(event_count(&pool, &context, "outbox.published").await, 0);

    let published = jobs::relay_outbox_with_sink(&pool, &context, 10, &sink)
        .await
        .expect("retry relay");
    assert_eq!(published.len(), 1);
    assert_eq!(unpublished_outbox_count(&pool, &context).await, 0);
    assert_eq!(event_count(&pool, &context, "outbox.published").await, 1);
    assert!(jobs::relay_outbox_with_sink(&pool, &context, 10, &sink)
        .await
        .expect("replay")
        .is_empty());
    assert_eq!(event_count(&pool, &context, "outbox.published").await, 1);

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn poison_outbox_event_does_not_block_healthy_events() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org(&pool, Uuid::new_v4(), Uuid::new_v4(), "jobs-outbox-poison").await;
    // Enqueue order becomes created_at order: healthy, poison (middle), healthy.
    enqueue_one(&pool, &context, "healthy-before").await;
    let poison = enqueue_one(&pool, &context, "poison").await;
    enqueue_one(&pool, &context, "healthy-after").await;
    assert_eq!(unpublished_outbox_count(&pool, &context).await, 3);

    let sink = Arc::new(PoisonJobSink::new(poison.id));
    let published = jobs::relay_outbox_with_sink(&pool, &context, 10, &sink)
        .await
        .expect("relay isolates the poison event");

    // The poison event in the middle must not block the healthy event after it.
    assert_eq!(published.len(), 2);
    assert!(published
        .iter()
        .all(|publication| publication.outbox.job_id != Some(poison.id)));
    assert_eq!(unpublished_outbox_count(&pool, &context).await, 1);
    assert_eq!(event_count(&pool, &context, "outbox.published").await, 2);

    ephemeral.drop().await;
}

#[tokio::test]
async fn older_payload_deserializes_and_payloads_reject_content_or_secrets() {
    let document_id = Uuid::new_v4();
    let decoded = jobs::decode_job_payload(1, json!({ "document_id": document_id }))
        .expect("decode older payload");
    assert_eq!(decoded.document_id, Some(document_id));
    assert_eq!(decoded.collection_id, None);

    assert!(matches!(
        jobs::decode_job_payload(2, json!({ "content": "raw markdown" })),
        Err(JobError::InvalidPayload(_))
    ));
    assert!(matches!(
        jobs::decode_job_payload(2, json!({ "secret": Uuid::new_v4() })),
        Err(JobError::InvalidPayload(_))
    ));
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn org_isolation_prevents_cross_org_claim_see_and_mutate() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let ctx_a = seed_org(&pool, Uuid::new_v4(), Uuid::new_v4(), "jobs-orga").await;
    let ctx_b = seed_org(&pool, Uuid::new_v4(), Uuid::new_v4(), "jobs-orgb").await;
    let job_a = enqueue_one(&pool, &ctx_a, "a").await;
    let job_b = enqueue_one(&pool, &ctx_b, "b").await;

    let claimed_b = jobs::claim(&pool, &ctx_b, "worker-b", 10, Duration::from_secs(60))
        .await
        .expect("claim b");
    assert_eq!(
        claimed_b.iter().map(|job| job.id).collect::<Vec<_>>(),
        vec![job_b.id]
    );
    let claim_b = claimed_b.first().expect("claim b").clone();
    assert!(matches!(
        jobs::complete(
            &pool,
            &ctx_b,
            job_a.id,
            lease_token(&claim_b),
            claim_b.attempts
        )
        .await,
        Err(JobError::LeaseLost)
    ));

    let claimed_a = jobs::claim(&pool, &ctx_a, "worker-a", 10, Duration::from_secs(60))
        .await
        .expect("claim a");
    assert_eq!(
        claimed_a.iter().map(|job| job.id).collect::<Vec<_>>(),
        vec![job_a.id]
    );
    let claim_a = claimed_a.first().expect("claim a").clone();
    assert!(matches!(
        jobs::heartbeat(
            &pool,
            &ctx_b,
            job_a.id,
            lease_token(&claim_a),
            claim_a.attempts,
            Duration::from_secs(60),
        )
        .await,
        Err(JobError::LeaseLost)
    ));
    jobs::complete(
        &pool,
        &ctx_a,
        job_a.id,
        lease_token(&claim_a),
        claim_a.attempts,
    )
    .await
    .expect("complete a");

    let (jobs_a, outbox_a, _) = table_counts(&pool, &ctx_a).await;
    let (jobs_b, outbox_b, _) = table_counts(&pool, &ctx_b).await;
    assert_eq!((jobs_a, jobs_b), (1, 1));
    assert!(outbox_a >= 2);
    assert_eq!(outbox_b, 1);

    ephemeral.drop().await;
}
