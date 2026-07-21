//! Integration tests for tenant-scoped hybrid retrieval (P1B-R01).
//!
//! Hermetic acceptance coverage lives in `services/retrieval` unit tests.
//! Live PostgreSQL tests are ignored unless `MARKHAND_TEST_DATABASE_URL` is set.

use std::collections::BTreeSet;

use chrono::{TimeZone, Utc};
use fileconv_knowledge::rank::VECTOR_WEIGHT;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::database::apply_migrations;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::db::search;
use fileconv_server::services::retrieval::{
    resolve_scope, same_lineage_pair, validate_request, RetrievalError, RetrievalRequest,
    VersionMode, PERMISSION_QA_QUERY,
};
use tokio_postgres::NoTls;
use uuid::Uuid;

fn test_database_url() -> Option<String> {
    match std::env::var("MARKHAND_TEST_DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => None,
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
        let suffix = Uuid::new_v4().simple();
        let db_name = format!("markhand_ret_{suffix}");
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

#[test]
fn frozen_vector_weight_matches_knowledge_contract() {
    assert!((VECTOR_WEIGHT - 0.55).abs() < f32::EPSILON);
}

#[test]
fn hermetic_scope_and_lineage_gates() {
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let collection = Uuid::new_v4();
    let ctx = OrgContext::try_new(org, user, [PERMISSION_QA_QUERY], [collection]).unwrap();
    assert!(resolve_scope(&ctx, None).is_ok());
    assert!(matches!(
        resolve_scope(
            &OrgContext::try_new(org, user, [PERMISSION_QA_QUERY], []).unwrap(),
            None
        ),
        Err(RetrievalError::EmptyScope)
    ));

    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    assert!(same_lineage_pair(&[(a, 1, None), (b, 2, Some(a))], a, b));
    assert!(!same_lineage_pair(&[(a, 1, None)], a, b));

    let bad = RetrievalRequest {
        query: String::new(),
        collection_ids: Some(BTreeSet::from([collection])),
        mode: VersionMode::Current,
        limit: 8,
        conflict_ids: vec![],
    };
    assert!(validate_request(&bad).is_err());
}

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn as_of_resolves_effective_version_from_postgres() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url)
        .await
        .expect("migrate ephemeral db");
    let pool = create_pool(&ephemeral.url).expect("pool");

    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let collection = Uuid::new_v4();
    let document = Uuid::new_v4();
    let v1 = Uuid::new_v4();
    let v2 = Uuid::new_v4();
    let v3 = Uuid::new_v4();
    let ctx = OrgContext::try_new(org, user, [PERMISSION_QA_QUERY], [collection]).unwrap();

    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO orgs (id, slug, name) VALUES ($1, $2, $3)",
                    &[&ctx.org_id(), &format!("org-{}", ctx.org_id()), &"org"],
                )
                .await?;
                let user_email = format!("{}@example.test", ctx.user_id());
                txn.execute(
                    "INSERT INTO users (id, email, display_name, password_hash)
                     VALUES ($1, $2, 'u', 'x')",
                    &[&ctx.user_id(), &user_email],
                )
                .await?;
                txn.execute(
                    "INSERT INTO collections (
                        id, org_id, name, slug, owner_user_id, visibility
                     ) VALUES ($1, $2, 'c', $3, $4, 'org')",
                    &[
                        &collection,
                        &ctx.org_id(),
                        &format!("c-{collection}"),
                        &ctx.user_id(),
                    ],
                )
                .await?;
                txn.execute(
                    "INSERT INTO documents (
                        id, org_id, collection_id, title, state, created_by_user_id
                     ) VALUES ($1, $2, $3, 'doc', 'indexed', $4)",
                    &[&document, &ctx.org_id(), &collection, &ctx.user_id()],
                )
                .await?;
                let sha_prefix = "a".repeat(63);
                let sha1 = format!("{sha_prefix}1");
                let sha2 = format!("{sha_prefix}2");
                let sha3 = format!("{sha_prefix}3");
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state, is_current,
                        content_sha256, original_object_key, effective_from, effective_to,
                        created_by_user_id
                     ) VALUES
                     ($1,$2,$3,1,'published',false,$4,'k1','2024-01-01Z','2024-04-01Z',$7),
                     ($5,$2,$3,2,'published',false,$6,'k2','2024-04-01Z','2024-08-01Z',$7),
                     ($8,$2,$3,3,'published',true,$9,'k3','2024-08-01Z',NULL,$7)",
                    &[
                        &v1,
                        &ctx.org_id(),
                        &document,
                        &sha1,
                        &v2,
                        &sha2,
                        &ctx.user_id(),
                        &v3,
                        &sha3,
                    ],
                )
                .await?;
                txn.execute(
                    "UPDATE documents SET current_version_id = $1 WHERE id = $2",
                    &[&v3, &document],
                )
                .await?;
                let as_of = Utc.with_ymd_and_hms(2024, 2, 15, 0, 0, 0).unwrap();
                let ids =
                    search::resolve_as_of_version_ids(txn, &ctx, &[collection], as_of).await?;
                assert_eq!(ids, BTreeSet::from([v1]));
                Ok(())
            })
        }
    })
    .await
    .expect("as_of fixture");

    ephemeral.drop().await;
}
