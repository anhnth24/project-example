//! 1C-08 runtime pool defense + dedicated worker role.
//!
//! Requires `MARKHAND_TEST_DATABASE_URL` (bootstrap, CREATEDB/CREATEROLE) and
//! `MARKHAND_TEST_APP_DATABASE_URL` (markhand_app). The worker-role test
//! provisions `markhand_worker` itself (deploy-role.rs precedent) so the
//! grants in migration `0035_expand_worker_role.sql` actually apply.

mod common;

use std::time::{Duration, Instant};

use common::{
    admin_database_url, app_database_url, boot_app_pool_with_max_size, connect_raw,
    rewrite_database_url, DualRoleEphemeralDb,
};
use fileconv_server::auth::context::OrgContext;
use fileconv_server::db::models::JobType;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::jobs::{self, EnqueueJob, JobPayload};
use uuid::Uuid;

const WORKER_TEST_PASSWORD: &str = "markhand_worker_test";
const CONTAMINATION_ADVISORY_KEY: i64 = 424_242;

/// Swap the userinfo of a `postgres://user:pass@host/db?query` URL.
fn rewrite_user(base_url: &str, user: &str, password: &str) -> String {
    let (scheme, rest) = base_url.split_once("://").expect("scheme");
    let hostpart = match rest.split_once('@') {
        Some((_userinfo, hostpart)) => hostpart,
        None => rest,
    };
    format!("{scheme}://{user}:{password}@{hostpart}")
}

/// A pooled connection contaminated with *session-level* state (misuse of a
/// raw checkout: `set_config(..., false)` + a session advisory lock) must be
/// handed out clean on the next checkout ([`RecyclingMethod::Clean`] in
/// `db/pool.rs`). Transaction-local GUCs are already covered by
/// `repositories.rs::pool_does_not_leak_tenant_gucs`.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn contaminated_pool_connection_is_reset_on_next_checkout() {
    let Some(admin) = admin_database_url() else {
        return;
    };
    let Some(app) = app_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_app_pool_with_max_size(&admin, &app, 1).await;

    let rogue_org = Uuid::new_v4();
    let contaminated_pid: i32 = {
        let client = pool.get().await.expect("first checkout");
        // Simulated misuse: session-level GUC (is_local = false) outside any
        // transaction + a session advisory lock that is never released.
        client
            .execute(
                "SELECT set_config('app.org_id', $1, false)",
                &[&rogue_org.to_string()],
            )
            .await
            .expect("session-level set_config");
        client
            .execute(
                "SELECT pg_advisory_lock($1)",
                &[&CONTAMINATION_ADVISORY_KEY],
            )
            .await
            .expect("session advisory lock");
        let visible: Option<String> = client
            .query_one(
                "SELECT NULLIF(current_setting('app.org_id', true), '')",
                &[],
            )
            .await
            .expect("probe GUC")
            .get(0);
        assert_eq!(
            visible.as_deref(),
            Some(rogue_org.to_string().as_str()),
            "contamination must be in place before checkin"
        );
        client
            .query_one("SELECT pg_backend_pid()", &[])
            .await
            .expect("pid")
            .get(0)
    }; // connection returns to the pool contaminated

    let client = pool.get().await.expect("second checkout");
    let reused_pid: i32 = client
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .expect("pid")
        .get(0);
    assert_eq!(
        contaminated_pid, reused_pid,
        "max_size=1 pool must hand back the same backend, else the test proves nothing"
    );

    let leaked: Option<String> = client
        .query_one(
            "SELECT NULLIF(current_setting('app.org_id', true), '')",
            &[],
        )
        .await
        .expect("probe GUC after recycle")
        .get(0);
    assert!(
        leaked.is_none(),
        "session-level app.org_id must not survive checkin/checkout, got {leaked:?}"
    );
    // The exact predicate RLS policies evaluate must also be NULL.
    let rls_org: Option<Uuid> = client
        .query_one("SELECT markhand_current_org_id()", &[])
        .await
        .expect("rls helper probe")
        .get(0);
    assert!(
        rls_org.is_none(),
        "markhand_current_org_id() must be NULL on a fresh checkout, got {rls_org:?}"
    );
    let advisory_held: i64 = client
        .query_one(
            "SELECT count(*)::bigint FROM pg_locks
             WHERE locktype = 'advisory' AND pid = pg_backend_pid()",
            &[],
        )
        .await
        .expect("advisory lock probe")
        .get(0);
    assert_eq!(
        advisory_held, 0,
        "session advisory locks must be released on recycle"
    );
    drop(client);

    // Not an assertion — rough per-checkout cost of the Clean recycle round
    // trip, for the 1C-08 trade-off record.
    let started = Instant::now();
    const ROUNDS: u32 = 200;
    for _ in 0..ROUNDS {
        let client = pool.get().await.expect("bench checkout");
        drop(client);
    }
    eprintln!(
        "pool_defense: {ROUNDS} recycled checkouts in {:?} (~{:?}/checkout)",
        started.elapsed(),
        started.elapsed() / ROUNDS
    );

    ephemeral.drop().await;
}

/// The dedicated `markhand_worker` role (migration 0035) can run the job
/// queue under an org context but is not owner, cannot bypass RLS, cannot see
/// other orgs' jobs, and has no access at all to auth/ACL tables.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn worker_role_is_rls_scoped_and_least_privilege() {
    let Some(admin) = admin_database_url() else {
        return;
    };
    let Some(app) = app_database_url() else {
        return;
    };

    // Provision the role BEFORE migrations so 0035's guarded grants apply.
    let admin_maintenance = rewrite_database_url(&admin, "postgres");
    let maintenance = connect_raw(&admin_maintenance).await;
    let password = WORKER_TEST_PASSWORD.replace('\'', "''");
    maintenance
        .batch_execute(&format!(
            "DO $$ BEGIN
               IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'markhand_worker') THEN
                 CREATE ROLE markhand_worker LOGIN PASSWORD '{password}'
                   NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS NOINHERIT;
               ELSE
                 ALTER ROLE markhand_worker WITH LOGIN PASSWORD '{password}'
                   NOSUPERUSER NOBYPASSRLS;
               END IF;
             END $$;"
        ))
        .await
        .expect("ensure markhand_worker role");

    let ephemeral = DualRoleEphemeralDb::create(&admin, &app).await;
    let admin_on_db = connect_raw(&ephemeral.admin_db_url).await;
    admin_on_db
        .batch_execute(&format!(
            "GRANT CONNECT ON DATABASE \"{}\" TO markhand_worker",
            ephemeral.db_name()
        ))
        .await
        .expect("grant connect to worker role");

    let worker_url = rewrite_user(&ephemeral.app_url, "markhand_worker", WORKER_TEST_PASSWORD);
    let worker_pool = create_pool(&worker_url).expect("worker pool");

    // Role posture: correct user, no superuser, no BYPASSRLS, not table owner.
    {
        let client = worker_pool.get().await.expect("worker checkout");
        let row = client
            .query_one(
                "SELECT current_user::text, rolsuper, rolbypassrls
                 FROM pg_roles WHERE rolname = current_user",
                &[],
            )
            .await
            .expect("role probe");
        let current_user: String = row.get(0);
        let rolsuper: bool = row.get(1);
        let rolbypassrls: bool = row.get(2);
        assert_eq!(current_user, "markhand_worker");
        assert!(!rolsuper, "markhand_worker must not be superuser");
        assert!(!rolbypassrls, "markhand_worker must not bypass RLS");
        let jobs_owner: String = client
            .query_one(
                "SELECT pg_get_userbyid(c.relowner)
                 FROM pg_class c
                 JOIN pg_namespace n ON n.oid = c.relnamespace
                 WHERE n.nspname = 'public' AND c.relname = 'jobs'",
                &[],
            )
            .await
            .expect("owner probe")
            .get(0);
        assert_ne!(
            jobs_owner, "markhand_worker",
            "worker must not own RLS-forced tables"
        );

        // Least privilege: auth/ACL/chat tables are unreachable entirely.
        for denied in [
            "SELECT count(*) FROM refresh_tokens",
            "SELECT count(*) FROM org_memberships",
            "SELECT count(*) FROM org_invites",
            "SELECT count(*) FROM collection_user_access",
            "SELECT count(*) FROM qa_chat_sessions",
            "SELECT count(*) FROM upload_operations",
        ] {
            let error = client
                .batch_execute(denied)
                .await
                .expect_err(&format!("worker must be denied: {denied}"));
            // Display is just "db error"; the SQLSTATE lives on the DbError.
            let code = error
                .as_db_error()
                .unwrap_or_else(|| panic!("expected db error for {denied}, got {error:?}"))
                .code()
                .clone();
            assert_eq!(
                code,
                tokio_postgres::error::SqlState::INSUFFICIENT_PRIVILEGE,
                "expected permission denied (42501) for {denied}, got {error:?}"
            );
        }
        // Append-only audit + no schema mutation.
        assert!(
            client
                .batch_execute("UPDATE audit_log SET outcome = 'deny' WHERE false")
                .await
                .is_err(),
            "worker must not UPDATE audit_log"
        );
        assert!(
            client
                .batch_execute("CREATE TABLE worker_must_not_create (id int)")
                .await
                .is_err(),
            "worker must not CREATE TABLE"
        );
    }

    // Seed FK targets as admin (worker has no grants on orgs/users — by design).
    let org_a = Uuid::new_v4();
    let org_b = Uuid::new_v4();
    admin_on_db
        .batch_execute(&format!(
            "INSERT INTO orgs (id, slug, name) VALUES
               ('{org_a}', 'worker-a', 'Worker A'),
               ('{org_b}', 'worker-b', 'Worker B');
             INSERT INTO users (id, email, display_name) VALUES
               ('{org_a}', 'worker-a@example.test', 'Worker A'),
               ('{org_b}', 'worker-b@example.test', 'Worker B');"
        ))
        .await
        .expect("seed orgs/users");

    let ctx_a = OrgContext::try_new(org_a, org_a, [] as [&str; 0], []).expect("ctx a");
    let ctx_b = OrgContext::try_new(org_b, org_b, [] as [&str; 0], []).expect("ctx b");

    // Worker can drive the job queue for its org: enqueue + claim + heartbeat.
    let outcome = jobs::enqueue(
        &worker_pool,
        &ctx_a,
        EnqueueJob::new(JobType::Convert, JobPayload::default(), "worker-role-job"),
    )
    .await
    .expect("worker enqueue must work");
    assert!(outcome.created, "job must be freshly created");

    let claimed = jobs::claim(
        &worker_pool,
        &ctx_a,
        "worker-role-test",
        1,
        Duration::from_secs(30),
    )
    .await
    .expect("worker claim must work");
    assert_eq!(claimed.len(), 1, "org A worker must claim its own job");
    assert_eq!(claimed[0].id, outcome.job.id);

    // RLS blocks cross-org: org B context sees nothing to claim or count.
    let stolen = jobs::claim(
        &worker_pool,
        &ctx_b,
        "worker-role-test",
        1,
        Duration::from_secs(30),
    )
    .await
    .expect("cross-org claim query must run");
    assert!(stolen.is_empty(), "org B must not claim org A jobs");
    let visible_to_b: i64 = with_org_txn(&worker_pool, &ctx_b, |txn| {
        Box::pin(async move {
            let row = txn
                .query_one("SELECT count(*)::bigint FROM jobs", &[])
                .await?;
            Ok(row.get(0))
        })
    })
    .await
    .expect("cross-org count");
    assert_eq!(visible_to_b, 0, "RLS must hide org A jobs from org B");

    // Without any org context the queue is invisible (FORCE RLS, not grants).
    let no_context = worker_pool.get().await.expect("raw worker checkout");
    let visible_without_ctx: i64 = no_context
        .query_one("SELECT count(*)::bigint FROM jobs", &[])
        .await
        .expect("no-context count")
        .get(0);
    assert_eq!(
        visible_without_ctx, 0,
        "no app.org_id context must mean zero visible jobs"
    );
    drop(no_context);

    drop(admin_on_db);
    worker_pool.close();
    ephemeral.drop().await;
}
