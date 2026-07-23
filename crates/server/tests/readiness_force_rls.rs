//! Cross-org readiness aggregates under FORCE RLS (P1B-R06).
//!
//! Requires dual-role URLs: admin (`MARKHAND_TEST_DATABASE_URL`) to create an
//! ephemeral DB and seed rows, and `markhand_app` (`MARKHAND_TEST_APP_DATABASE_URL`)
//! for FORCE RLS probes + SECURITY DEFINER helpers.

mod common;

use common::{
    admin_database_url, app_database_url, assert_markhand_app_role, boot_app_pool, connect_raw,
    drop_database_force, rewrite_database_url, DualRoleEphemeralDb,
};
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL (markhand_app)"]
async fn app_role_sees_two_org_generation_consistency_via_definer() {
    let Some(admin_url) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let (ephemeral, pool) = boot_app_pool(&admin_url, &app_url).await;
    assert_markhand_app_role(&pool).await;

    let org_a = Uuid::new_v4();
    let org_b = Uuid::new_v4();
    let org_c = Uuid::new_v4();
    let collection_a = Uuid::new_v4();
    let collection_b = Uuid::new_v4();
    let collection_c = Uuid::new_v4();
    let gen_a = Uuid::new_v4();
    let gen_b = Uuid::new_v4();
    let gen_b_new = Uuid::new_v4();
    let gen_c = Uuid::new_v4();
    let sig_b = "b".repeat(64);
    let sig_c = "c".repeat(64);

    let admin = connect_raw(&ephemeral.admin_db_url).await;
    // Org A active@b, org B active@c → consistent(b) false until B matches.
    admin
        .batch_execute(&format!(
            "
            INSERT INTO orgs (id, slug, name) VALUES
              ('{org_a}', 'ready-a', 'Ready A'),
              ('{org_b}', 'ready-b', 'Ready B');
            INSERT INTO users (id, email, display_name) VALUES
              ('{org_a}', 'a@example.test', 'A'),
              ('{org_b}', 'b@example.test', 'B');
            INSERT INTO collections (
                id, org_id, name, slug, owner_user_id, visibility
            ) VALUES
              ('{collection_a}', '{org_a}', 'Ready A', 'ready-a', '{org_a}', 'private'),
              ('{collection_b}', '{org_b}', 'Ready B', 'ready-b', '{org_b}', 'private');
            INSERT INTO index_metadata (
                id, org_id, collection_id, index_signature_sha256, embedding_family,
                embedding_revision, dimensions, runtime_path, generation, is_active, state
            ) VALUES
              ('{gen_a}', '{org_a}', '{collection_a}', '{sig_b}', 'test',
               'r1', 8, 'vllm-local', 1, true, 'active'),
              ('{gen_b}', '{org_b}', '{collection_b}', '{sig_c}', 'test',
               'r1', 8, 'vllm-local', 1, true, 'active');
            "
        ))
        .await
        .expect("seed readiness generations");

    let app = connect_raw(&ephemeral.app_url).await;
    let role: String = app
        .query_one("SELECT current_user::text", &[])
        .await
        .expect("current_user")
        .get(0);
    assert_eq!(role, "markhand_app");

    // Direct table reads without app.org_id GUC must see zero rows under FORCE RLS.
    let direct_generations: i64 = app
        .query_one("SELECT count(*)::bigint FROM index_metadata", &[])
        .await
        .expect("direct generation count")
        .get(0);
    assert_eq!(
        direct_generations, 0,
        "FORCE RLS hides index_metadata without GUC"
    );
    let direct_fences: i64 = app
        .query_one("SELECT count(*)::bigint FROM ops_fences", &[])
        .await
        .expect("direct fence count")
        .get(0);
    assert_eq!(direct_fences, 0, "FORCE RLS hides ops_fences without GUC");
    let direct_jobs: i64 = app
        .query_one("SELECT count(*)::bigint FROM jobs", &[])
        .await
        .expect("direct jobs count")
        .get(0);
    assert_eq!(direct_jobs, 0, "FORCE RLS hides jobs without GUC");

    let mismatched: bool = app
        .query_one("SELECT markhand_index_generation_consistent($1)", &[&sig_b])
        .await
        .expect("A=b B=c → consistent(b)")
        .get(0);
    assert!(
        !mismatched,
        "org A sig b + org B sig c must report false for expected b"
    );

    // Signature is immutable — retire B@c and insert B@b.
    admin
        .batch_execute(&format!(
            "
            UPDATE index_metadata SET is_active = false, state = 'retired'
             WHERE id = '{gen_b}';
            INSERT INTO index_metadata (
                id, org_id, collection_id, index_signature_sha256, embedding_family,
                embedding_revision, dimensions, runtime_path, generation, is_active, state
            ) VALUES (
                '{gen_b_new}', '{org_b}', '{collection_b}', '{sig_b}', 'test',
                'r1', 8, 'vllm-local', 2, true, 'active'
            );
            "
        ))
        .await
        .expect("align B to signature b");

    let aligned: bool = app
        .query_one("SELECT markhand_index_generation_consistent($1)", &[&sig_b])
        .await
        .expect("aligned A+B")
        .get(0);
    assert!(aligned, "after B→b, consistent(b) must be true");

    // Contract: org with index_metadata but no matching active generation → false.
    admin
        .batch_execute(&format!(
            "
            INSERT INTO orgs (id, slug, name) VALUES ('{org_c}', 'ready-c', 'Ready C');
            INSERT INTO users (id, email, display_name)
             VALUES ('{org_c}', 'c@example.test', 'C');
            INSERT INTO collections (
                id, org_id, name, slug, owner_user_id, visibility
            ) VALUES (
                '{collection_c}', '{org_c}', 'Ready C', 'ready-c', '{org_c}', 'private'
            );
            INSERT INTO index_metadata (
                id, org_id, collection_id, index_signature_sha256, embedding_family,
                embedding_revision, dimensions, runtime_path, generation, is_active, state
            ) VALUES (
                '{gen_c}', '{org_c}', '{collection_c}', '{sig_b}', 'test',
                'r1', 8, 'vllm-local', 1, false, 'retired'
            );
            "
        ))
        .await
        .expect("seed org C without active generation");

    let missing_active: bool = app
        .query_one("SELECT markhand_index_generation_consistent($1)", &[&sig_b])
        .await
        .expect("missing active")
        .get(0);
    assert!(
        !missing_active,
        "org with only retired generation must report false"
    );

    admin
        .batch_execute(&format!(
            "UPDATE index_metadata SET is_active = true, state = 'active'
             WHERE id = '{gen_c}'"
        ))
        .await
        .expect("activate C");

    let consistent: bool = app
        .query_one("SELECT markhand_index_generation_consistent($1)", &[&sig_b])
        .await
        .expect("all orgs active@b")
        .get(0);
    assert!(
        consistent,
        "after C gains active@b, consistent(b) must be true"
    );

    let fence_before: bool = app
        .query_one("SELECT markhand_any_blocking_fence_active()", &[])
        .await
        .expect("fence before")
        .get(0);
    assert!(!fence_before, "no blocking fence initially");

    admin
        .batch_execute(
            "INSERT INTO ops_fences (name, reason, active)
             VALUES ('restore', 'readiness-test', true)
             ON CONFLICT (name) DO UPDATE
             SET reason = EXCLUDED.reason, active = true, cleared_at = NULL",
        )
        .await
        .expect("activate fence");
    let fence_active: bool = app
        .query_one("SELECT markhand_any_blocking_fence_active()", &[])
        .await
        .expect("fence active")
        .get(0);
    assert!(fence_active, "blocking fence must report true");

    admin
        .batch_execute(
            "UPDATE ops_fences
             SET active = false, cleared_at = clock_timestamp()
             WHERE name = 'restore'",
        )
        .await
        .expect("clear fence");
    let fence_cleared: bool = app
        .query_one("SELECT markhand_any_blocking_fence_active()", &[])
        .await
        .expect("fence cleared")
        .get(0);
    assert!(!fence_cleared, "cleared fence must report false");

    let reconcile_before: bool = app
        .query_one("SELECT markhand_any_reconcile_running()", &[])
        .await
        .expect("reconcile before")
        .get(0);
    assert!(!reconcile_before, "no reconcile job initially");

    admin
        .batch_execute(&format!(
            "INSERT INTO jobs (
                id, org_id, job_type, status, idempotency_key, document_id, version_id, payload
             ) VALUES (
                '{job}', '{org_a}', 'reconcile', 'running', 'ready-reconcile',
                NULL, NULL, '{{}}'::jsonb
             )",
            job = Uuid::new_v4()
        ))
        .await
        .expect("seed reconcile job");
    let reconcile_running: bool = app
        .query_one("SELECT markhand_any_reconcile_running()", &[])
        .await
        .expect("reconcile running")
        .get(0);
    assert!(reconcile_running, "running reconcile must report true");

    admin
        .batch_execute(
            "UPDATE jobs SET status = 'succeeded', finished_at = clock_timestamp()
             WHERE job_type = 'reconcile' AND status = 'running'",
        )
        .await
        .expect("finish reconcile");
    let reconcile_done: bool = app
        .query_one("SELECT markhand_any_reconcile_running()", &[])
        .await
        .expect("reconcile done")
        .get(0);
    assert!(!reconcile_done, "finished reconcile must report false");

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL (markhand_app)"]
async fn ephemeral_drop_database_with_force_removes_pg_database() {
    let Some(admin_url) = admin_database_url() else {
        return;
    };
    let Some(app_url) = app_database_url() else {
        return;
    };
    let ephemeral = DualRoleEphemeralDb::create(&admin_url, &app_url).await;
    let db_name = ephemeral.db_name().to_string();
    let maintenance = rewrite_database_url(&admin_url, "postgres");
    let admin = connect_raw(&maintenance).await;
    let present: bool = admin
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)",
            &[&db_name],
        )
        .await
        .expect("pg_database present")
        .get(0);
    assert!(present, "created ephemeral DB must appear in pg_database");

    ephemeral.drop().await;

    let admin = connect_raw(&maintenance).await;
    let absent: bool = admin
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)",
            &[&db_name],
        )
        .await
        .expect("pg_database absent probe")
        .get(0);
    assert!(
        !absent,
        "DROP DATABASE WITH (FORCE) must remove pg_database row"
    );

    let leftovers: i64 = admin
        .query_one(
            "SELECT count(*)::bigint FROM pg_database WHERE datname = $1",
            &[&db_name],
        )
        .await
        .expect("leftover count")
        .get(0);
    assert_eq!(leftovers, 0);

    let bare_name = format!("markhand_it_{}", Uuid::new_v4().simple());
    admin
        .batch_execute(&format!("CREATE DATABASE \"{bare_name}\""))
        .await
        .expect("CREATE DATABASE bare");
    drop_database_force(&maintenance, &bare_name)
        .await
        .expect("direct drop_database_force");
    let gone: bool = admin
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)",
            &[&bare_name],
        )
        .await
        .expect("bare gone")
        .get(0);
    assert!(!gone);
}
