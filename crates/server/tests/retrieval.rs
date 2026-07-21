//! Integration tests for tenant-scoped hybrid retrieval (P1B-R01).
//!
//! Hermetic acceptance coverage lives in `services/retrieval` unit tests.
//! Live PostgreSQL tests are ignored unless `MARKHAND_TEST_DATABASE_URL` is set.

use std::collections::BTreeSet;
use std::time::Instant;

use chrono::{TimeZone, Utc};
use fileconv_knowledge::rank::VECTOR_WEIGHT;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::database::apply_migrations;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::db::search::{self, VersionVisibility};
use fileconv_server::services::retrieval::{
    resolve_scope, same_lineage_pair, validate_request, RetrievalError, RetrievalRequest,
    VersionMode, PERMISSION_QA_HISTORY, PERMISSION_QA_QUERY,
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

fn sha64(ch: char) -> String {
    ch.to_string().repeat(64)
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
    let role = Uuid::new_v4();
    let document = Uuid::new_v4();
    let v1 = Uuid::new_v4();
    let v2 = Uuid::new_v4();
    let v3 = Uuid::new_v4();
    let ctx = OrgContext::try_new(
        org,
        user,
        [PERMISSION_QA_QUERY, PERMISSION_QA_HISTORY],
        [collection],
    )
    .unwrap();

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
                     VALUES ($1, $2, 'u', 'test-hash')",
                    &[&ctx.user_id(), &user_email],
                )
                .await?;
                txn.execute(
                    "INSERT INTO org_memberships (org_id, user_id, role)
                     VALUES ($1, $2, 'viewer')",
                    &[&ctx.org_id(), &ctx.user_id()],
                )
                .await?;
                txn.execute(
                    "INSERT INTO roles (id, org_id, code, name, is_system)
                     VALUES ($1, $2, 'viewer', 'Viewer', true)",
                    &[&role, &ctx.org_id()],
                )
                .await?;
                txn.execute(
                    "INSERT INTO role_permissions (org_id, role_id, permission_id)
                     SELECT $1, $2, id
                     FROM permissions
                     WHERE code IN ('qa.query', 'qa.history')",
                    &[&ctx.org_id(), &role],
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

/// Live regression: `ts_rank_cd` is PG `real`; decode as f32 (f64 get panics).
/// Also gates accent-fold-v1 FTS parity and active-generation-only filtering.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn fts_rank_accent_fold_and_active_generation_gates() {
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
    let role = Uuid::new_v4();
    let document = Uuid::new_v4();
    let version = Uuid::new_v4();
    let meta_active = Uuid::new_v4();
    let meta_shadow = Uuid::new_v4();
    let chunk_active = Uuid::new_v4();
    let chunk_shadow = Uuid::new_v4();
    let sig_active = sha64('a');
    let sig_shadow = sha64('b');
    let identity_active = sha64('c');
    let identity_shadow = sha64('d');
    let content_sha = sha64('e');
    let ctx = OrgContext::try_new(
        org,
        user,
        [PERMISSION_QA_QUERY, PERMISSION_QA_HISTORY],
        [collection],
    )
    .unwrap();

    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let sig_active = sig_active.clone();
        let sig_shadow = sig_shadow.clone();
        let identity_active = identity_active.clone();
        let identity_shadow = identity_shadow.clone();
        let content_sha = content_sha.clone();
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
                     VALUES ($1, $2, 'u', 'test-hash')",
                    &[&ctx.user_id(), &user_email],
                )
                .await?;
                txn.execute(
                    "INSERT INTO org_memberships (org_id, user_id, role)
                     VALUES ($1, $2, 'viewer')",
                    &[&ctx.org_id(), &ctx.user_id()],
                )
                .await?;
                txn.execute(
                    "INSERT INTO roles (id, org_id, code, name, is_system)
                     VALUES ($1, $2, 'viewer', 'Viewer', true)",
                    &[&role, &ctx.org_id()],
                )
                .await?;
                txn.execute(
                    "INSERT INTO role_permissions (org_id, role_id, permission_id)
                     SELECT $1, $2, id
                     FROM permissions
                     WHERE code IN ('qa.query', 'qa.history')",
                    &[&ctx.org_id(), &role],
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
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state, is_current,
                        content_sha256, original_object_key, effective_from, created_by_user_id
                     ) VALUES ($1,$2,$3,1,'published',true,$4,'k1', now(), $5)",
                    &[
                        &version,
                        &ctx.org_id(),
                        &document,
                        &content_sha,
                        &ctx.user_id(),
                    ],
                )
                .await?;
                txn.execute(
                    "UPDATE documents SET current_version_id = $1 WHERE id = $2",
                    &[&version, &document],
                )
                .await?;
                txn.execute(
                    "INSERT INTO index_metadata (
                        id, org_id, collection_id, index_signature_sha256, embedding_family,
                        embedding_revision, dimensions, runtime_path, generation, is_active, state
                     ) VALUES
                     ($1,$2,$3,$4,'f','r',8,'local-hash',1,true,'active'),
                     ($5,$2,$3,$6,'f','r',8,'local-hash',2,false,'shadow')",
                    &[
                        &meta_active,
                        &ctx.org_id(),
                        &collection,
                        &sig_active,
                        &meta_shadow,
                        &sig_shadow,
                    ],
                )
                .await?;
                // Accented Vietnamese body — query uses accent-fold-v1 ("doi soat").
                txn.execute(
                    "INSERT INTO chunks (
                        id, org_id, document_id, version_id, ordinal, heading_path, body,
                        chunk_identity_sha256, index_metadata_id, index_signature
                     ) VALUES
                     ($1,$2,$3,$4,0,ARRAY['Đối soát'],'Đối soát giao dịch theo ngày',
                      $5,$6,$7),
                     ($8,$2,$3,$4,1,ARRAY['Shadow'],'Đối soát chỉ ở shadow generation',
                      $9,$10,$11)",
                    &[
                        &chunk_active,
                        &ctx.org_id(),
                        &document,
                        &version,
                        &identity_active,
                        &meta_active,
                        &sig_active,
                        &chunk_shadow,
                        &identity_shadow,
                        &meta_shadow,
                        &sig_shadow,
                    ],
                )
                .await?;

                let started = Instant::now();
                let hits = search::fts_search(
                    txn,
                    &ctx,
                    &[collection],
                    "Đối soát",
                    &VersionVisibility::Current,
                    10,
                )
                .await?;
                let elapsed = started.elapsed();

                assert_eq!(
                    hits.len(),
                    1,
                    "active-generation + accent-fold must match exactly one chunk"
                );
                assert_eq!(hits[0].chunk_id, chunk_active);
                assert_eq!(hits[0].chunk_identity_sha256, identity_active);
                // Live regression for Sol finding #1: REAL rank must decode as f32.
                let rank: f32 = hits[0].rank;
                assert!(rank.is_finite() && rank > 0.0);
                assert!(
                    elapsed.as_secs() < 2,
                    "FTS latency gate exceeded: {elapsed:?}"
                );

                // Direct REAL decode path used by map_fts_candidate.
                let row = txn
                    .query_one(
                        "SELECT ts_rank_cd(
                            to_tsvector('simple', markhand_accent_fold('Đối soát')),
                            plainto_tsquery('simple', markhand_accent_fold('doi soat'))
                         )::real AS rank",
                        &[],
                    )
                    .await?;
                let rank_f32: f32 = search::read_pg_real_rank(&row, "rank");
                assert!(rank_f32 > 0.0);

                let hydrated = search::hydrate_chunks_by_identity(
                    txn,
                    &ctx,
                    &[collection],
                    std::slice::from_ref(&identity_active),
                    &VersionVisibility::Current,
                )
                .await?;
                assert_eq!(hydrated.len(), 1);

                let historical_visibility =
                    VersionVisibility::VersionIds(BTreeSet::from([version]));
                let historical = search::hydrate_chunks_by_identity(
                    txn,
                    &ctx,
                    &[collection],
                    std::slice::from_ref(&identity_active),
                    &historical_visibility,
                )
                .await?;
                assert_eq!(historical.len(), 1);

                txn.execute(
                    "DELETE FROM role_permissions
                     WHERE org_id = $1
                       AND role_id = $2
                       AND permission_id = (
                         SELECT id FROM permissions WHERE code = $3
                       )",
                    &[&ctx.org_id(), &role, &PERMISSION_QA_HISTORY],
                )
                .await?;
                let current_after_history_revoke = search::hydrate_chunks_by_identity(
                    txn,
                    &ctx,
                    &[collection],
                    std::slice::from_ref(&identity_active),
                    &VersionVisibility::Current,
                )
                .await?;
                assert_eq!(current_after_history_revoke.len(), 1);
                let denied_historical = search::hydrate_chunks_by_identity(
                    txn,
                    &ctx,
                    &[collection],
                    std::slice::from_ref(&identity_active),
                    &historical_visibility,
                )
                .await?;
                assert!(
                    denied_historical.is_empty(),
                    "historical hydration must recheck qa.history instead of trusting stale OrgContext"
                );

                txn.execute(
                    "DELETE FROM org_memberships WHERE org_id = $1 AND user_id = $2",
                    &[&ctx.org_id(), &ctx.user_id()],
                )
                .await?;
                let denied_after_membership_revoke = search::hydrate_chunks_by_identity(
                    txn,
                    &ctx,
                    &[collection],
                    std::slice::from_ref(&identity_active),
                    &VersionVisibility::Current,
                )
                .await?;
                assert!(
                    denied_after_membership_revoke.is_empty(),
                    "hydration must recheck current membership instead of trusting stale OrgContext"
                );

                Ok(())
            })
        }
    })
    .await
    .expect("fts fixture");

    ephemeral.drop().await;
}
