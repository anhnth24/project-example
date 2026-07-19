//! Live PostgreSQL tests for P1B-I03 durable jobs, outbox, and event log.
//!
//! Skips cleanly when `MARKHAND_TEST_DATABASE_URL` is unset. These tests use the
//! non-superuser app role and production org-scoped transaction helper.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::database::apply_migrations;
use fileconv_server::db::error::DbError;
use fileconv_server::db::jobs as db_jobs;
use fileconv_server::db::models::{Job, JobStatus, JobType};
use fileconv_server::db::orgs;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::jobs::{
    self, CancelOutcome, CheckpointPayload, EnqueueJob, EventPayload, JobError, JobPayload,
    CURRENT_EVENT_PAYLOAD_VERSION, CURRENT_JOB_PAYLOAD_VERSION,
};
use serde_json::json;
use tokio::sync::Barrier;
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
                db_jobs::get_by_id(txn, &context, job_id)
                    .await?
                    .ok_or(DbError::NotFound)
            })
        }
    })
    .await
    .expect("get job")
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

#[tokio::test]
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
                let now = db_jobs::fresh_clock_timestamp(txn).await?;
                let job_id = Uuid::new_v4();
                let outbox_key = format!("job.enqueued:{job_id}");
                db_jobs::insert_job_with_outbox(
                    txn,
                    &context,
                    db_jobs::NewJob {
                        id: job_id,
                        job_type: JobType::Convert,
                        payload_version: CURRENT_JOB_PAYLOAD_VERSION,
                        payload: &payload,
                        max_attempts: 5,
                        idempotency_key: "forced-rollback",
                        document_id: None,
                        version_id: None,
                        available_at: now,
                        outbox_event_type: "job.enqueued",
                        outbox_payload_version: CURRENT_EVENT_PAYLOAD_VERSION,
                        outbox_payload: &event_payload,
                        outbox_idempotency_key: &outbox_key,
                    },
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
    force_expire(&pool, &context, reclaimable.id).await;
    force_expire(&pool, &context, exhausted.id).await;
    assert!(
        jobs::heartbeat(&pool, &context, live.id, "worker", Duration::from_secs(60))
            .await
            .expect("heartbeat")
    );

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

#[tokio::test]
async fn checkpoint_survives_kill_reclaim_and_resume_claim() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org(&pool, Uuid::new_v4(), Uuid::new_v4(), "jobs-checkpoint").await;
    let job = enqueue_one(&pool, &context, "checkpoint").await;
    jobs::claim(&pool, &context, "worker-a", 1, Duration::from_secs(60))
        .await
        .expect("claim");

    let checkpoint = CheckpointPayload {
        cursor_id: Some(Uuid::new_v4()),
        completed_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
        offset: Some(7),
    };
    let checkpoint_json = checkpoint.to_json().expect("checkpoint json");
    jobs::checkpoint(&pool, &context, job.id, "worker-a", checkpoint)
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
async fn fail_retries_with_future_backoff_then_dead_letters_when_exhausted() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_pool(&base_url).await;
    let context = seed_org(&pool, Uuid::new_v4(), Uuid::new_v4(), "jobs-retry").await;
    let job = enqueue_with_attempts(&pool, &context, "retry", 2).await;
    jobs::claim(&pool, &context, "worker", 1, Duration::from_secs(60))
        .await
        .expect("claim");

    let retry = jobs::fail(&pool, &context, job.id, "worker", "transient")
        .await
        .expect("fail retry");
    assert_eq!(retry.status, JobStatus::Pending);
    assert!(retry.available_at > retry.updated_at);
    assert_eq!(retry.last_error.as_deref(), Some("transient"));

    force_available(&pool, &context, job.id).await;
    jobs::claim(&pool, &context, "worker", 1, Duration::from_secs(60))
        .await
        .expect("claim retry");
    let dead = jobs::fail(&pool, &context, job.id, "worker", "exhausted")
        .await
        .expect("fail dead");
    assert_eq!(dead.status, JobStatus::DeadLetter);
    assert!(dead.finished_at.is_some());

    ephemeral.drop().await;
}

#[tokio::test]
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
    jobs::claim(&pool, &context, "owner", 1, Duration::from_secs(60))
        .await
        .expect("claim leased");
    assert!(matches!(
        jobs::cancel(&pool, &context, leased.id)
            .await
            .expect("cancel leased"),
        CancelOutcome::Cancelled(_)
    ));
    assert!(
        !jobs::heartbeat(&pool, &context, leased.id, "owner", Duration::from_secs(60))
            .await
            .expect("heartbeat lost")
    );

    let guarded = enqueue_one(&pool, &context, "owner-guard").await;
    jobs::claim(&pool, &context, "owner-a", 1, Duration::from_secs(60))
        .await
        .expect("claim guarded");
    assert!(matches!(
        jobs::complete(&pool, &context, guarded.id, "owner-b").await,
        Err(JobError::LeaseLost)
    ));
    assert!(matches!(
        jobs::fail(&pool, &context, guarded.id, "owner-b", "nope").await,
        Err(JobError::LeaseLost)
    ));
    jobs::complete(&pool, &context, guarded.id, "owner-a")
        .await
        .expect("complete owner");
    assert!(matches!(
        jobs::cancel(&pool, &context, guarded.id).await,
        Err(JobError::CannotCancelTerminal(JobStatus::Succeeded))
    ));

    ephemeral.drop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
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
        move |txn| Box::pin(async move { db_jobs::event_log_sequences(txn, &context).await })
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
    assert!(matches!(
        jobs::complete(&pool, &ctx_b, job_a.id, "worker-b").await,
        Err(JobError::LeaseLost)
    ));

    let claimed_a = jobs::claim(&pool, &ctx_a, "worker-a", 10, Duration::from_secs(60))
        .await
        .expect("claim a");
    assert_eq!(
        claimed_a.iter().map(|job| job.id).collect::<Vec<_>>(),
        vec![job_a.id]
    );
    assert!(
        !jobs::heartbeat(&pool, &ctx_b, job_a.id, "worker-a", Duration::from_secs(60))
            .await
            .expect("cross heartbeat")
    );
    jobs::complete(&pool, &ctx_a, job_a.id, "worker-a")
        .await
        .expect("complete a");

    let (jobs_a, outbox_a, _) = table_counts(&pool, &ctx_a).await;
    let (jobs_b, outbox_b, _) = table_counts(&pool, &ctx_b).await;
    assert_eq!((jobs_a, jobs_b), (1, 1));
    assert!(outbox_a >= 2);
    assert_eq!(outbox_b, 1);

    ephemeral.drop().await;
}
