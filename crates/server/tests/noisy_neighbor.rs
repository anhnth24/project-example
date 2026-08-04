//! Live PostgreSQL noisy-neighbor tests for 1C-10 per-org worker fairness.
//!
//! Deterministic by construction: fairness is asserted by COUNTING claim
//! order over the round-robin rotation (`workers::fairness::OrgRotation`) and
//! the 1C-09 `concurrent_jobs` admission — never by wall-clock SLO (that is
//! 1C-13's load gate). Skips cleanly when the dual-role URLs are unset; runs
//! against non-superuser `markhand_app` so RLS claims behave as in prod.

mod common;

use std::time::{Duration, Instant};

use common::phase1c_probe::{elapsed_ms, emit_probe_result};
use common::{admin_database_url, app_database_url, boot_app_pool};
use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::db::models::JobType;
use fileconv_server::db::orgs;
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::jobs::{self, EnqueueJob, JobPayload};
use fileconv_server::workers::fairness::OrgRotation;
use serde_json::json;
use uuid::Uuid;

const LEASE_TTL: Duration = Duration::from_secs(60);

/// Worker-posture context: empty permission set, like `bin/worker.rs`.
fn worker_ctx(org: Uuid, user: Uuid) -> OrgContext {
    OrgContext::try_new(org, user, [] as [&str; 0], []).expect("ctx")
}

async fn seed_org(pool: &Pool, slug: &str, max_concurrent_jobs: i32) -> OrgContext {
    let context = worker_ctx(Uuid::new_v4(), Uuid::new_v4());
    let slug = slug.to_string();
    with_org_txn(pool, &context, {
        let context = context.clone();
        move |txn| {
            Box::pin(async move {
                orgs::ensure_exists(txn, &context, &slug, &slug).await?;
                orgs::ensure_user(
                    txn,
                    &context,
                    context.user_id(),
                    &format!("{slug}@example.test"),
                    &slug,
                )
                .await?;
                orgs::ensure_membership(txn, &context).await?;
                txn.execute(
                    "INSERT INTO org_quotas (
                        org_id, max_storage_bytes, max_documents,
                        max_concurrent_jobs, max_monthly_tokens
                     )
                     VALUES ($1, $2, $3, $4, $5)",
                    &[
                        &context.org_id(),
                        &1_000_000_i64,
                        &1_000_i32,
                        &max_concurrent_jobs,
                        &1_000_000_i64,
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed org");
    context
}

async fn enqueue_convert_jobs(pool: &Pool, ctx: &OrgContext, label: &str, count: usize) {
    for index in 0..count {
        let outcome = jobs::enqueue(
            pool,
            ctx,
            EnqueueJob::new(
                JobType::Convert,
                JobPayload::default(),
                format!("{label}-{index}"),
            ),
        )
        .await
        .expect("enqueue");
        assert!(outcome.created);
    }
}

/// Claim at most one job for the org and complete it immediately; returns the
/// served org id, or `None` when the org has nothing claimable (empty backlog
/// or no free `concurrent_jobs` slot).
async fn claim_and_complete_one(pool: &Pool, ctx: &OrgContext) -> Result<Option<Uuid>, String> {
    let claimed = jobs::claim(pool, ctx, "fair-worker", 1, LEASE_TTL)
        .await
        .map_err(|error| error.to_string())?;
    let Some(job) = claimed.into_iter().next() else {
        return Ok(None);
    };
    let lease = job.lease_owner.clone().expect("lease token");
    jobs::complete(pool, ctx, job.id, &lease, job.attempts)
        .await
        .map_err(|error| error.to_string())?;
    Ok(Some(ctx.org_id()))
}

/// Org A floods the queue (20 jobs) while org B has 4. Round-robin rotation
/// must serve them strictly alternating until B drains: between two
/// consecutive B jobs at most one A job runs (bound = N - 1 orgs), so B's
/// k-th job is served at position 2k regardless of A's backlog size.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn noisy_org_backlog_does_not_starve_quiet_org() {
    let Some(admin) = admin_database_url() else {
        return;
    };
    let Some(app) = app_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_app_pool(&admin, &app).await;

    let ctx_a = seed_org(&pool, "noisy-a", 8).await;
    let ctx_b = seed_org(&pool, "quiet-b", 8).await;
    enqueue_convert_jobs(&pool, &ctx_a, "noisy", 20).await;
    enqueue_convert_jobs(&pool, &ctx_b, "quiet", 4).await;

    let rotation = OrgRotation::new(vec![ctx_a.clone(), ctx_b.clone()]).expect("rotation");
    let cycle_started = Instant::now();
    let mut served = Vec::new();
    for _ in 0..100 {
        let outcome = rotation
            .run_cycle(|ctx| {
                let pool = pool.clone();
                async move { claim_and_complete_one(&pool, &ctx).await }
            })
            .await
            .expect("cycle");
        match outcome {
            Some(org) => served.push(org),
            None => break,
        }
    }

    let a = ctx_a.org_id();
    let b = ctx_b.org_id();
    assert_eq!(served.len(), 24, "all jobs of both orgs must drain");
    assert_eq!(
        served[..8],
        [a, b, a, b, a, b, a, b],
        "while org B has backlog, services must alternate (fairness bound N-1)"
    );
    assert!(
        served[8..].iter().all(|org| *org == a),
        "after org B drains, org A continues without idle gaps"
    );
    assert_eq!(served.iter().filter(|org| **org == b).count(), 4);
    assert_eq!(served.iter().filter(|org| **org == a).count(), 20);

    let quiet_ms = elapsed_ms(cycle_started) / 4;
    let starvation_events = served
        .windows(2)
        .filter(|window| window[0] == window[1] && window[0] == b)
        .count();
    emit_probe_result(
        "noisy_neighbor_fairness",
        json!({
            "quiet_org_query_p95_ms": quiet_ms,
            "starvation_events": starvation_events,
        }),
    );

    ephemeral.drop().await;
}

/// Org A holds its only `concurrent_jobs` slot with a long-running (never
/// completed) lease and still has pending backlog. The 1C-09 admission clamp
/// makes A's claim come back empty, and the rotation must fall through to
/// org B within the SAME cycle — a slot-starved noisy org cannot block the
/// quiet org for even one cycle.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn slot_exhausted_noisy_org_falls_through_to_quiet_org_same_cycle() {
    let Some(admin) = admin_database_url() else {
        return;
    };
    let Some(app) = app_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_app_pool(&admin, &app).await;

    let ctx_a = seed_org(&pool, "slot-noisy-a", 1).await;
    let ctx_b = seed_org(&pool, "slot-quiet-b", 8).await;
    enqueue_convert_jobs(&pool, &ctx_a, "slot-noisy", 3).await;
    enqueue_convert_jobs(&pool, &ctx_b, "slot-quiet", 1).await;

    // Occupy org A's single slot without completing (long-running job).
    let held = jobs::claim(&pool, &ctx_a, "hog-worker", 1, LEASE_TTL)
        .await
        .expect("claim hog job");
    assert_eq!(held.len(), 1, "org A must hold its only slot");

    let rotation = OrgRotation::new(vec![ctx_a.clone(), ctx_b.clone()]).expect("rotation");
    let outcome = rotation
        .run_cycle(|ctx| {
            let pool = pool.clone();
            async move { claim_and_complete_one(&pool, &ctx).await }
        })
        .await
        .expect("cycle");
    assert_eq!(
        outcome,
        Some(ctx_b.org_id()),
        "cycle must skip the slot-exhausted org and serve org B immediately"
    );

    // Org A remains pending (not lost, not dead-lettered) for when a slot frees.
    let next = rotation
        .run_cycle(|ctx| {
            let pool = pool.clone();
            async move { claim_and_complete_one(&pool, &ctx).await }
        })
        .await
        .expect("cycle");
    assert_eq!(
        next, None,
        "with B drained and A slot-blocked, the cycle is idle (no busy loop)"
    );

    ephemeral.drop().await;
}
