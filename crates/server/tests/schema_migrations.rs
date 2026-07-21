//! Integration tests for P1B-F03 schema migrations against real PostgreSQL.
//!
//! Skips gracefully when `MARKHAND_TEST_DATABASE_URL` is unset so CI without a
//! database stays green. Local runs must set the URL and exercise real Postgres.

use fileconv_server::database::{apply_migrations, embedded_migrations, migration_checksum};
use fileconv_server::db::models::expected_table_columns;
use std::collections::BTreeSet;
use tokio_postgres::GenericClient;
use tokio_postgres::{Client, NoTls};
use uuid::Uuid;

const BUSINESS_TABLES: &[&str] = &[
    "org_memberships",
    "roles",
    "role_permissions",
    "groups",
    "group_memberships",
    "refresh_tokens",
    "org_invites",
    "collections",
    "collection_user_access",
    "collection_group_access",
    "collection_role_access",
    "documents",
    "document_versions",
    "derived_artifacts",
    "index_metadata",
    "index_generation_backfills",
    "chunks",
    "embedding_batches",
    "claims",
    "conflicts",
    "conflict_evidence",
    "jobs",
    "outbox_events",
    "event_log",
    "org_quotas",
    "usage_counters",
    "quota_reservations",
    "audit_log",
    "download_capabilities",
    "authz_epochs",
    "document_authz_epochs",
];

const GLOBAL_TABLES: &[&str] = &["orgs", "users", "permissions"];

const POC_ORG: &str = "11111111-1111-1111-1111-111111111111";
const POC_USER: &str = "22222222-2222-2222-2222-222222222201";
const POC_COLLECTION: &str = "55555555-5555-5555-5555-555555555501";

fn test_database_url() -> Option<String> {
    match std::env::var("MARKHAND_TEST_DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => {
            eprintln!(
                "skipped: MARKHAND_TEST_DATABASE_URL unset — schema migration integration tests require PostgreSQL"
            );
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

fn admin_database_url(base_url: &str) -> String {
    rewrite_database_url(base_url, "postgres")
}

async fn connect(database_url: &str) -> Client {
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
        let db_name = format!("markhand_it_{}", Uuid::new_v4().simple());
        let admin_url = admin_database_url(base_url);
        let admin = connect(&admin_url).await;
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
        let admin = connect(&self.admin_url).await;
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

async fn set_org<C: GenericClient>(client: &C, org_id: Uuid) {
    client
        .batch_execute(&format!("SET LOCAL app.org_id = '{org_id}'"))
        .await
        .expect("SET LOCAL app.org_id");
}

fn pg_error_text(error: &tokio_postgres::Error) -> String {
    if let Some(db) = error.as_db_error() {
        format!("{} {}", db.message(), db.detail().unwrap_or(""))
    } else {
        format!("{error:?}")
    }
}

fn sha64(ch: char) -> String {
    std::iter::repeat_n(ch, 64).collect()
}

#[allow(clippy::too_many_arguments)]
async fn insert_draft_version<C: GenericClient>(
    client: &C,
    org: Uuid,
    document_id: Uuid,
    version_id: Uuid,
    number: i32,
    parent: Option<Uuid>,
    user: Uuid,
    sha: &str,
) {
    client
        .execute(
            "INSERT INTO document_versions (
                id, org_id, document_id, version_number, parent_version_id,
                publication_state, is_current, content_sha256, original_object_key,
                effective_from, created_by_user_id
             ) VALUES ($1,$2,$3,$4,$5,'draft',false,$6,$7, clock_timestamp() - interval '1 hour', $8)",
            &[
                &version_id,
                &org,
                &document_id,
                &number,
                &parent,
                &sha,
                &format!("k/{number}"),
                &user,
            ],
        )
        .await
        .unwrap();
}

async fn publish<C: GenericClient>(client: &C, org: Uuid, document_id: Uuid, version_id: Uuid) {
    client
        .query_one(
            "SELECT markhand_publish_document_version($1, $2, $3)",
            &[&org, &document_id, &version_id],
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn schema_migrations_fresh_apply_idempotent_and_exact_columns() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.expect("fresh apply");
    apply_migrations(&ephemeral.url)
        .await
        .expect("re-apply must be idempotent");

    let client = connect(&ephemeral.url).await;
    let applied: i64 = client
        .query_one("SELECT count(*) FROM markhand_schema_migrations", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(applied, embedded_migrations().len() as i64);

    for table in BUSINESS_TABLES {
        let nullable: String = client
            .query_one(
                "SELECT is_nullable FROM information_schema.columns \
                 WHERE table_schema = 'public' AND table_name = $1 AND column_name = 'org_id'",
                &[table],
            )
            .await
            .unwrap_or_else(|_| panic!("missing org_id on {table}"))
            .get(0);
        assert_eq!(nullable, "NO", "{table}.org_id must be NOT NULL");
    }
    for table in GLOBAL_TABLES {
        let count: i64 = client
            .query_one(
                "SELECT count(*) FROM information_schema.columns \
                 WHERE table_schema = 'public' AND table_name = $1 AND column_name = 'org_id'",
                &[table],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(count, 0, "{table} is global");
    }

    // Exact-set both-directions drift guard for every modeled table.
    let expected_tables: BTreeSet<&str> =
        expected_table_columns().iter().map(|(t, _)| *t).collect();
    assert!(
        expected_tables.len() >= 28,
        "expected full table coverage, got {}",
        expected_tables.len()
    );
    for (table, expected_cols) in expected_table_columns() {
        let actual: BTreeSet<String> = client
            .query(
                "SELECT column_name FROM information_schema.columns
                 WHERE table_schema = 'public' AND table_name = $1",
                &[table],
            )
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get(0))
            .collect();
        let expected: BTreeSet<String> = expected_cols.iter().map(|c| (*c).to_string()).collect();
        assert_eq!(
            actual, expected,
            "column drift on {table}: actual={actual:?} expected={expected:?}"
        );
    }

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn supported_upgrade_from_0001_0002_then_remainder() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    let mut client = connect(&ephemeral.url).await;
    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS markhand_schema_migrations (
                name text PRIMARY KEY,
                checksum text NOT NULL,
                applied_at timestamptz NOT NULL DEFAULT now()
            )",
        )
        .await
        .unwrap();
    for &(name, source) in embedded_migrations().iter().take(2) {
        let tx = client.transaction().await.unwrap();
        tx.batch_execute(source).await.unwrap();
        let checksum = migration_checksum(source);
        tx.execute(
            "INSERT INTO markhand_schema_migrations (name, checksum) VALUES ($1, $2)",
            &[&name, &checksum],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    apply_migrations(&ephemeral.url)
        .await
        .expect("upgrade apply");
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn legal_transitions_reject_illegal_publication_mutations() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.unwrap();
    let mut client = connect(&ephemeral.url).await;

    let org = Uuid::parse_str(POC_ORG).unwrap();
    let user = Uuid::parse_str(POC_USER).unwrap();
    let collection = Uuid::parse_str(POC_COLLECTION).unwrap();
    let document_id = Uuid::new_v4();
    let v1 = Uuid::new_v4();
    let v2 = Uuid::new_v4();
    let sha = sha64('a');

    let tx = client.transaction().await.unwrap();
    set_org(&tx, org).await;
    tx.execute(
        "INSERT INTO documents (id, org_id, collection_id, title, state, created_by_user_id)
         VALUES ($1,$2,$3,'Legal Transitions','indexed',$4)",
        &[&document_id, &org, &collection, &user],
    )
    .await
    .unwrap();
    insert_draft_version(&tx, org, document_id, v1, 1, None, user, &sha).await;
    publish(&tx, org, document_id, v1).await;
    insert_draft_version(&tx, org, document_id, v2, 2, Some(v1), user, &sha).await;
    publish(&tx, org, document_id, v2).await;
    tx.commit().await.unwrap();

    let tx = client.transaction().await.unwrap();
    set_org(&tx, org).await;
    // Even with a bogus publishing GUC, illegal transitions must fail.
    tx.batch_execute("SELECT set_config('markhand.publishing', '1', true)")
        .await
        .unwrap();

    tx.batch_execute("SAVEPOINT sp_back").await.unwrap();
    let back = tx
        .execute(
            "UPDATE document_versions SET publication_state = 'draft' WHERE id = $1",
            &[&v2],
        )
        .await
        .expect_err("published→draft must fail");
    assert!(
        pg_error_text(&back).contains("illegal publication_state")
            || pg_error_text(&back).contains("immutable"),
        "{}",
        pg_error_text(&back)
    );
    tx.batch_execute("ROLLBACK TO SAVEPOINT sp_back")
        .await
        .unwrap();

    tx.batch_execute("SAVEPOINT sp_eff").await.unwrap();
    let rewrite_eff = tx
        .execute(
            "UPDATE document_versions SET effective_to = clock_timestamp() + interval '1 day'
             WHERE id = $1",
            &[&v1],
        )
        .await
        .expect_err("effective_to rewrite must fail");
    assert!(
        pg_error_text(&rewrite_eff).contains("effective_to"),
        "{}",
        pg_error_text(&rewrite_eff)
    );
    tx.batch_execute("ROLLBACK TO SAVEPOINT sp_eff")
        .await
        .unwrap();

    tx.batch_execute("SAVEPOINT sp_content").await.unwrap();
    let content = tx
        .execute(
            "UPDATE document_versions SET content_sha256 = $1 WHERE id = $2",
            &[&sha64('b'), &v2],
        )
        .await
        .expect_err("content UPDATE must fail");
    assert!(pg_error_text(&content).contains("immutable"));
    tx.batch_execute("ROLLBACK TO SAVEPOINT sp_content")
        .await
        .unwrap();

    tx.commit().await.unwrap();
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn two_currents_rejected_even_if_org_context_cleared() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.unwrap();
    let mut client = connect(&ephemeral.url).await;

    let org = Uuid::parse_str(POC_ORG).unwrap();
    let user = Uuid::parse_str(POC_USER).unwrap();
    let collection = Uuid::parse_str(POC_COLLECTION).unwrap();
    let document_id = Uuid::new_v4();
    let v1 = Uuid::new_v4();
    let v2 = Uuid::new_v4();
    let sha = sha64('c');

    let tx = client.transaction().await.unwrap();
    set_org(&tx, org).await;
    tx.execute(
        "INSERT INTO documents (id, org_id, collection_id, title, state, created_by_user_id)
         VALUES ($1,$2,$3,'Two Currents','indexed',$4)",
        &[&document_id, &org, &collection, &user],
    )
    .await
    .unwrap();
    insert_draft_version(&tx, org, document_id, v1, 1, None, user, &sha).await;
    publish(&tx, org, document_id, v1).await;
    insert_draft_version(&tx, org, document_id, v2, 2, Some(v1), user, &sha).await;
    // Leave v1 current; promote v2 without superseding via direct legal updates would fail unique.
    // Force two currents: set v2 current while v1 still current.
    let dup = tx
        .execute(
            "UPDATE document_versions
             SET publication_state = 'published', is_current = true
             WHERE id = $1",
            &[&v2],
        )
        .await
        .expect_err("two currents must hit unique index");
    let text = pg_error_text(&dup);
    assert!(
        text.contains("uq_document_versions__document_current") || text.contains("duplicate key"),
        "{text}"
    );
    tx.rollback().await.unwrap();

    // Unique index is physical / RLS-immune: multi-row insert of two currents fails.
    let tx = client.transaction().await.unwrap();
    set_org(&tx, org).await;
    let doc2 = Uuid::new_v4();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    tx.execute(
        "INSERT INTO documents (id, org_id, collection_id, title, state, created_by_user_id)
         VALUES ($1,$2,$3,'Two Currents B','indexed',$4)",
        &[&doc2, &org, &collection, &user],
    )
    .await
    .unwrap();
    let multi = tx
        .execute(
            "INSERT INTO document_versions (
                id, org_id, document_id, version_number, publication_state, is_current,
                content_sha256, original_object_key, effective_from, created_by_user_id
             ) VALUES
             ($1,$2,$3,1,'published',true,$4,'ka', clock_timestamp() - interval '1 hour', $5),
             ($6,$2,$3,2,'published',true,$7,'kb', clock_timestamp() - interval '1 hour', $5)",
            &[&a, &org, &doc2, &sha64('1'), &user, &b, &sha64('2')],
        )
        .await
        .expect_err("multi-row insert of two currents must fail");
    assert!(
        pg_error_text(&multi).contains("uq_document_versions__document_current")
            || pg_error_text(&multi).contains("duplicate key"),
        "{}",
        pg_error_text(&multi)
    );
    // Clearing tenant context cannot undo a unique-index rejection (already statement-failed).
    tx.batch_execute("RESET app.org_id").await.ok();
    tx.rollback().await.unwrap();
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn acl_cascade_and_lineage_fks() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.unwrap();
    let mut client = connect(&ephemeral.url).await;

    let org = Uuid::parse_str(POC_ORG).unwrap();
    let user = Uuid::parse_str(POC_USER).unwrap();
    let collection = Uuid::parse_str(POC_COLLECTION).unwrap();
    let group_id = Uuid::new_v4();
    let doc_a = Uuid::new_v4();
    let doc_b = Uuid::new_v4();
    let va = Uuid::new_v4();
    let vb = Uuid::new_v4();
    let chunk_b = Uuid::new_v4();
    let meta = Uuid::new_v4();
    let sha = sha64('d');
    let sig = sha64('e');

    let tx = client.transaction().await.unwrap();
    set_org(&tx, org).await;
    tx.execute(
        "INSERT INTO groups (id, org_id, name) VALUES ($1,$2,'Editors')",
        &[&group_id, &org],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO collection_group_access (org_id, collection_id, group_id, access_level)
         VALUES ($1,$2,$3,'read')",
        &[&org, &collection, &group_id],
    )
    .await
    .unwrap();
    let access_count: i64 = tx
        .query_one(
            "SELECT count(*) FROM collection_group_access WHERE group_id = $1",
            &[&group_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(access_count, 1);
    tx.execute("DELETE FROM groups WHERE id = $1", &[&group_id])
        .await
        .unwrap();
    let dangling: i64 = tx
        .query_one(
            "SELECT count(*) FROM collection_group_access WHERE group_id = $1",
            &[&group_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(dangling, 0, "group delete must cascade ACL rows");

    for (id, title) in [(doc_a, "A"), (doc_b, "B")] {
        tx.execute(
            "INSERT INTO documents (id, org_id, collection_id, title, state, created_by_user_id)
             VALUES ($1,$2,$3,$4,'indexed',$5)",
            &[&id, &org, &collection, &title, &user],
        )
        .await
        .unwrap();
    }
    insert_draft_version(&tx, org, doc_a, va, 1, None, user, &sha).await;
    publish(&tx, org, doc_a, va).await;
    insert_draft_version(&tx, org, doc_b, vb, 1, None, user, &sha).await;
    publish(&tx, org, doc_b, vb).await;
    tx.execute(
        "INSERT INTO index_metadata (
            id, org_id, collection_id, index_signature_sha256, embedding_family,
            embedding_revision, dimensions, runtime_path, generation, is_active
         ) VALUES ($1,$2,$3,$4,'f','r',8,'local-hash',1,true)",
        &[&meta, &org, &collection, &sig],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO chunks (
            id, org_id, document_id, version_id, ordinal, body, chunk_identity_sha256,
            index_metadata_id, index_signature, tsv
         ) VALUES ($1,$2,$3,$4,0,'body',$5,$6,$7,to_tsvector('simple','body'))",
        &[&chunk_b, &org, &doc_b, &vb, &sha64('f'), &meta, &sig],
    )
    .await
    .unwrap();

    tx.batch_execute("SAVEPOINT sp_claim").await.unwrap();
    let cross_claim = tx
        .execute(
            "INSERT INTO claims (
                id, org_id, document_id, version_id, chunk_id, claim_key, subject, predicate,
                value_type, value_text, scope, effective_from
             ) VALUES ($1,$2,$3,$4,$5,'k','s','p','text','x','', now())",
            &[&Uuid::new_v4(), &org, &doc_a, &va, &chunk_b],
        )
        .await
        .expect_err("cross-document chunk citation must fail");
    assert!(
        pg_error_text(&cross_claim).contains("foreign key")
            || pg_error_text(&cross_claim).contains("fk_claims__chunk"),
        "{}",
        pg_error_text(&cross_claim)
    );
    tx.batch_execute("ROLLBACK TO SAVEPOINT sp_claim")
        .await
        .unwrap();

    tx.batch_execute("SAVEPOINT sp_event").await.unwrap();
    let bad_event = tx
        .execute(
            "INSERT INTO event_log (
                id, org_id, sequence_no, event_type, payload, document_id, version_id
             ) VALUES ($1,$2,1,'x','{}'::jsonb,$3,$4)",
            &[&Uuid::new_v4(), &org, &doc_a, &vb],
        )
        .await
        .expect_err("mismatched document/version event must fail");
    assert!(
        pg_error_text(&bad_event).contains("foreign key")
            || pg_error_text(&bad_event).contains("fk_event_log__version"),
        "{}",
        pg_error_text(&bad_event)
    );
    tx.batch_execute("ROLLBACK TO SAVEPOINT sp_event")
        .await
        .unwrap();

    tx.batch_execute("SAVEPOINT sp_meta").await.unwrap();
    let meta_mut = tx
        .execute(
            "UPDATE index_metadata SET index_signature_sha256 = $1 WHERE id = $2",
            &[&sha64('0'), &meta],
        )
        .await
        .expect_err("index_metadata signature UPDATE must fail");
    assert!(
        pg_error_text(&meta_mut).contains("immutable"),
        "{}",
        pg_error_text(&meta_mut)
    );
    tx.batch_execute("ROLLBACK TO SAVEPOINT sp_meta")
        .await
        .unwrap();

    tx.commit().await.unwrap();
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn set_null_preserves_org_id_on_optional_fk() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.unwrap();
    let mut client = connect(&ephemeral.url).await;

    let org = Uuid::parse_str(POC_ORG).unwrap();
    let user = Uuid::parse_str(POC_USER).unwrap();
    let collection = Uuid::parse_str(POC_COLLECTION).unwrap();
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let outbox_id = Uuid::new_v4();
    let sha = sha64('7');

    let tx = client.transaction().await.unwrap();
    set_org(&tx, org).await;
    tx.execute(
        "INSERT INTO documents (id, org_id, collection_id, title, state, created_by_user_id)
         VALUES ($1,$2,$3,'Set Null','indexed',$4)",
        &[&document_id, &org, &collection, &user],
    )
    .await
    .unwrap();
    insert_draft_version(&tx, org, document_id, version_id, 1, None, user, &sha).await;
    publish(&tx, org, document_id, version_id).await;
    tx.execute(
        "INSERT INTO jobs (
            id, org_id, job_type, status, idempotency_key, document_id, version_id, payload
         ) VALUES ($1,$2,'convert','pending','k1',$3,$4,'{}'::jsonb)",
        &[&job_id, &org, &document_id, &version_id],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO outbox_events (
            id, org_id, event_type, idempotency_key, job_id, payload
         ) VALUES ($1,$2,'job.created','o1',$3,'{}'::jsonb)",
        &[&outbox_id, &org, &job_id],
    )
    .await
    .unwrap();
    tx.execute("DELETE FROM jobs WHERE id = $1", &[&job_id])
        .await
        .unwrap();
    let row = tx
        .query_one(
            "SELECT org_id, job_id FROM outbox_events WHERE id = $1",
            &[&outbox_id],
        )
        .await
        .unwrap();
    let kept_org: Uuid = row.get(0);
    let nulled_job: Option<Uuid> = row.get(1);
    assert_eq!(
        kept_org, org,
        "org_id must survive ON DELETE SET NULL (job_id)"
    );
    assert!(nulled_job.is_none());

    // Conflict → version FKs must use column-list SET NULL (not unrestricted SET NULL
    // that would also null NOT NULL org_id). Versions are immutable, so we assert the
    // constraint definition and that a version DELETE cannot wipe conflict.org_id.
    let claim_a = Uuid::new_v4();
    let claim_b = Uuid::new_v4();
    let (low, high) = if claim_a < claim_b {
        (claim_a, claim_b)
    } else {
        (claim_b, claim_a)
    };
    let conflict_id = Uuid::new_v4();
    tx.execute(
        "INSERT INTO claims (
            id, org_id, document_id, version_id, claim_key, subject, predicate,
            value_type, value_money, unit, scope, effective_from
         ) VALUES ($1,$2,$3,$4,'k','s','p','money',1,'VND','', now())",
        &[&low, &org, &document_id, &version_id],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO claims (
            id, org_id, document_id, version_id, claim_key, subject, predicate,
            value_type, value_money, unit, scope, effective_from
         ) VALUES ($1,$2,$3,$4,'k2','s','p','money',2,'VND','', now())",
        &[&high, &org, &document_id, &version_id],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO conflicts (
            id, org_id, status, severity, conflict_type, claim_a_id, claim_b_id,
            first_detected_version_id
         ) VALUES ($1,$2,'open','warning','numeric',$3,$4,$5)",
        &[&conflict_id, &org, &low, &high, &version_id],
    )
    .await
    .unwrap();

    for (name, col) in [
        (
            "fk_conflicts__detected_version_org",
            "first_detected_version_id",
        ),
        ("fk_conflicts__resolution_a_org", "resolution_version_a_id"),
        ("fk_conflicts__resolution_b_org", "resolution_version_b_id"),
    ] {
        let def: String = tx
            .query_one(
                "SELECT pg_get_constraintdef(oid) FROM pg_constraint WHERE conname = $1",
                &[&name],
            )
            .await
            .unwrap_or_else(|_| panic!("missing constraint {name}"))
            .get(0);
        assert!(
            def.contains(&format!("ON DELETE SET NULL ({col})")),
            "{name} must use column-list SET NULL ({col}), got: {def}"
        );
    }

    tx.batch_execute("SAVEPOINT sp_ver_del").await.unwrap();
    let ver_del = tx
        .execute(
            "DELETE FROM document_versions WHERE id = $1",
            &[&version_id],
        )
        .await
        .expect_err("versions are immutable");
    assert!(pg_error_text(&ver_del).contains("immutable"));
    tx.batch_execute("ROLLBACK TO SAVEPOINT sp_ver_del")
        .await
        .unwrap();

    let conflict_row = tx
        .query_one(
            "SELECT org_id, first_detected_version_id FROM conflicts WHERE id = $1",
            &[&conflict_id],
        )
        .await
        .unwrap();
    assert_eq!(conflict_row.get::<_, Uuid>(0), org);
    assert_eq!(conflict_row.get::<_, Option<Uuid>>(1), Some(version_id));

    tx.commit().await.unwrap();
    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn concurrent_publish_and_lineage_as_of() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.unwrap();

    let org = Uuid::parse_str(POC_ORG).unwrap();
    let user = Uuid::parse_str(POC_USER).unwrap();
    let collection = Uuid::parse_str(POC_COLLECTION).unwrap();
    let document_id = Uuid::new_v4();
    let draft_a = Uuid::new_v4();
    let draft_b = Uuid::new_v4();
    let sha = sha64('9');

    {
        let mut client = connect(&ephemeral.url).await;
        let tx = client.transaction().await.unwrap();
        set_org(&tx, org).await;
        tx.execute(
            "INSERT INTO documents (id, org_id, collection_id, title, state, created_by_user_id)
             VALUES ($1,$2,$3,'Concurrent','indexed',$4)",
            &[&document_id, &org, &collection, &user],
        )
        .await
        .unwrap();
        insert_draft_version(&tx, org, document_id, draft_a, 1, None, user, &sha).await;
        insert_draft_version(&tx, org, document_id, draft_b, 2, Some(draft_a), user, &sha).await;
        tx.commit().await.unwrap();
    }

    let url = ephemeral.url.clone();
    let publish_one = |version_id: Uuid| {
        let url = url.clone();
        async move {
            let mut client = connect(&url).await;
            let tx = client.transaction().await.unwrap();
            set_org(&tx, org).await;
            match tx
                .query_one(
                    "SELECT markhand_publish_document_version($1,$2,$3)",
                    &[&org, &document_id, &version_id],
                )
                .await
            {
                Ok(_) => tx.commit().await.is_ok(),
                Err(_) => {
                    let _ = tx.rollback().await;
                    false
                }
            }
        }
    };
    let left_ok = tokio::spawn(publish_one(draft_a)).await.unwrap();
    let right_ok = tokio::spawn(publish_one(draft_b)).await.unwrap();
    assert!(left_ok || right_ok);

    let client = connect(&ephemeral.url).await;
    client
        .batch_execute(&format!("SET app.org_id = '{org}'"))
        .await
        .unwrap();
    let currents: i64 = client
        .query_one(
            "SELECT count(*) FROM document_versions WHERE document_id = $1 AND is_current",
            &[&document_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(currents, 1);

    // as-of lineage fixture
    let mut client = connect(&ephemeral.url).await;
    let doc = Uuid::new_v4();
    let v1 = Uuid::new_v4();
    let v2 = Uuid::new_v4();
    let v3 = Uuid::new_v4();
    let tx = client.transaction().await.unwrap();
    set_org(&tx, org).await;
    tx.execute(
        "INSERT INTO documents (id, org_id, collection_id, title, state, created_by_user_id)
         VALUES ($1,$2,$3,'Lineage','indexed',$4)",
        &[&doc, &org, &collection, &user],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO document_versions (
            id, org_id, document_id, version_number, publication_state, is_current,
            content_sha256, original_object_key, effective_from, effective_to, created_by_user_id
         ) VALUES ($1,$2,$3,1,'published',false,$4,'k1','2024-01-01Z','2024-04-01Z',$5)",
        &[&v1, &org, &doc, &sha64('1'), &user],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO document_versions (
            id, org_id, document_id, version_number, parent_version_id, publication_state, is_current,
            content_sha256, original_object_key, effective_from, effective_to, created_by_user_id
         ) VALUES ($1,$2,$3,2,$4,'published',false,$5,'k2','2024-04-01Z','2024-08-01Z',$6)",
        &[&v2, &org, &doc, &v1, &sha64('2'), &user],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO document_versions (
            id, org_id, document_id, version_number, parent_version_id, publication_state, is_current,
            content_sha256, original_object_key, effective_from, created_by_user_id
         ) VALUES ($1,$2,$3,3,$4,'published',true,$5,'k3','2024-08-01Z',$6)",
        &[&v3, &org, &doc, &v2, &sha64('3'), &user],
    )
    .await
    .unwrap();
    tx.execute(
        "UPDATE documents SET current_version_id = $1 WHERE id = $2",
        &[&v3, &doc],
    )
    .await
    .unwrap();
    let as_of: Uuid = tx
        .query_one(
            "SELECT id FROM document_versions WHERE document_id = $1
               AND publication_state = 'published'
               AND effective_from <= timestamptz '2024-02-15Z'
               AND (effective_to IS NULL OR effective_to > timestamptz '2024-02-15Z')
             ORDER BY version_number DESC LIMIT 1",
            &[&doc],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(as_of, v1);
    tx.commit().await.unwrap();

    ephemeral.drop().await;
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn rls_all_tenant_tables_and_pool_context_reset() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url).await.unwrap();
    let mut client = connect(&ephemeral.url).await;

    let org_a = Uuid::parse_str(POC_ORG).unwrap();
    let org_b = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    let collection_b = Uuid::new_v4();

    client
        .execute(
            "INSERT INTO orgs (id, slug, name) VALUES ($1,'other-org','Other')",
            &[&org_b],
        )
        .await
        .unwrap();
    client
        .execute(
            "INSERT INTO users (id, email, display_name) VALUES ($1,'other@example.com','O')",
            &[&user_b],
        )
        .await
        .unwrap();
    let tx = client.transaction().await.unwrap();
    set_org(&tx, org_b).await;
    tx.execute(
        "INSERT INTO org_memberships (org_id, user_id, role) VALUES ($1,$2,'owner')",
        &[&org_b, &user_b],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO collections (id, org_id, name, slug, owner_user_id, visibility)
         VALUES ($1,$2,'B','b-lib',$3,'org')",
        &[&collection_b, &org_b, &user_b],
    )
    .await
    .unwrap();
    tx.execute(
        "INSERT INTO org_quotas (org_id, max_storage_bytes, max_documents, max_concurrent_jobs, max_monthly_tokens)
         VALUES ($1,1,1,1,1)",
        &[&org_b],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    for table in BUSINESS_TABLES {
        let row = client
            .query_one(
                "SELECT c.relrowsecurity, c.relforcerowsecurity FROM pg_class c
                 JOIN pg_namespace n ON n.oid = c.relnamespace
                 WHERE n.nspname = 'public' AND c.relname = $1",
                &[table],
            )
            .await
            .unwrap_or_else(|_| panic!("missing {table}"));
        assert!(row.get::<_, bool>(0) && row.get::<_, bool>(1), "{table}");
    }

    let tx = client.transaction().await.unwrap();
    set_org(&tx, org_a).await;
    let leaked: i64 = tx
        .query_one(
            "SELECT count(*) FROM collections WHERE org_id = $1",
            &[&org_b],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(leaked, 0);
    tx.commit().await.unwrap();

    let tx = client.transaction().await.unwrap();
    set_org(&tx, org_a).await;
    assert!(
        tx.query_one("SELECT count(*) FROM collections", &[])
            .await
            .unwrap()
            .get::<_, i64>(0)
            >= 1
    );
    tx.commit().await.unwrap();
    let tx = client.transaction().await.unwrap();
    assert_eq!(
        tx.query_one("SELECT count(*) FROM collections", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0,
        "no pool leak"
    );
    tx.commit().await.unwrap();

    ephemeral.drop().await;
}
