//! Integration tests for tenant-scoped hybrid retrieval (P1B-R01).
//!
//! Hermetic acceptance coverage lives in `services/retrieval` unit tests.
//! Live PostgreSQL tests are ignored unless `MARKHAND_TEST_DATABASE_URL` is set.
//!
//! Fixtures seed `qa.query` / `qa.history` directly on the viewer role (not via
//! `seed_user_with_permissions`) so FTS/hydration ACL predicates match the 1C
//! `(qa.query, read)` projection without widening unrelated integration seeds.

mod common;

use std::collections::BTreeSet;
use std::time::Instant;

use chrono::{TimeZone, Utc};
use fileconv_knowledge::rank::VECTOR_WEIGHT;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::database::apply_migrations;
use fileconv_server::db::error::DbError;
use fileconv_server::db::pool::{create_pool, with_org_txn};
use fileconv_server::db::search::{self, VersionVisibility};
use fileconv_server::services::retrieval::{
    resolve_scope, same_lineage_pair, validate_request, RetrievalError, RetrievalRequest,
    VersionMode, PERMISSION_QA_HISTORY, PERMISSION_QA_QUERY,
};
use tokio_postgres::NoTls;
use uuid::Uuid;

fn test_database_url() -> Option<String> {
    common::admin_database_url()
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
        admin
            .batch_execute(&format!(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity                  WHERE datname = '{}' AND pid <> pg_backend_pid()",
                self.db_name
            ))
            .await
            .unwrap_or_else(|error| panic!("terminate backends failed: {error}"));
        admin
            .batch_execute(&format!(
                "DROP DATABASE IF EXISTS \"{}\" WITH (FORCE)",
                self.db_name
            ))
            .await
            .unwrap_or_else(|error| panic!("DROP DATABASE WITH (FORCE) failed: {error}"));
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

                // Regression (multi_org_denial flake root cause): PostgreSQL's
                // lexer tokenizes `<digits>e<digits>` hex runs as scientific-
                // notation floats, so hyphenated ids like
                // `phase1c-marker-alpha-6571e715…` produce tsvector tokens the
                // space-joined normalized query can never AND-match. fts_search
                // must OR a separator-preserving folded query so identifiers
                // stay findable both raw and token-split.
                let chunk_sci = Uuid::new_v4();
                let identity_sci = sha64('f');
                txn.execute(
                    "INSERT INTO chunks (
                        id, org_id, document_id, version_id, ordinal, heading_path, body,
                        chunk_identity_sha256, index_metadata_id, index_signature
                     ) VALUES ($1,$2,$3,$4,2,ARRAY['Marker'],
                       'Đánh dấu phase1c-marker-alpha-6571e715cb974ca2a8c26cdf50bbf797 trong nội dung',
                       $5,$6,$7)",
                    &[
                        &chunk_sci,
                        &ctx.org_id(),
                        &document,
                        &version,
                        &identity_sci,
                        &meta_active,
                        &sig_active,
                    ],
                )
                .await?;
                // Truy vấn identifier NGUYÊN VĂN (như multi_org_denial dán
                // marker): trước fix, normalize_fts_query space-join làm PG lex
                // token hex đứng-một-mình thành float `6571e715` & `cb974…`
                // (khác token trong tsv) → 0 hit vĩnh viễn. Leg fold giữ
                // separator phải giải quyết ca này. (Giới hạn còn lại có chủ
                // đích: người dùng tự gõ token RỜI của một identifier hex thì
                // lexer standalone-vs-compound của PG vẫn có thể lệch.)
                let raw_identifier_query =
                    "phase1c-marker-alpha-6571e715cb974ca2a8c26cdf50bbf797";
                let hits = search::fts_search(
                    txn,
                    &ctx,
                    &[collection],
                    raw_identifier_query,
                    &VersionVisibility::Current,
                    10,
                )
                .await?;
                assert!(
                    hits.iter().any(|hit| hit.chunk_id == chunk_sci),
                    "scientific-notation-shaped identifier must stay findable \
                     via its raw form; hits = {}",
                    hits.len()
                );
                // Cùng identifier qua đường VersionIds (nhánh SQL thứ hai).
                let historical_hits = search::fts_search(
                    txn,
                    &ctx,
                    &[collection],
                    raw_identifier_query,
                    &VersionVisibility::VersionIds(BTreeSet::from([version])),
                    10,
                )
                .await?;
                assert!(
                    historical_hits
                        .iter()
                        .any(|hit| hit.chunk_id == chunk_sci),
                    "identifier must stay findable via the version-scoped leg"
                );

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
                    "INSERT INTO role_permissions (org_id, role_id, permission_id)
                     SELECT $1, $2, id FROM permissions WHERE code = $3",
                    &[&ctx.org_id(), &role, &PERMISSION_QA_HISTORY],
                )
                .await?;
                txn.execute(
                    "DELETE FROM role_permissions
                     WHERE org_id = $1
                       AND role_id = $2
                       AND permission_id = (
                         SELECT id FROM permissions WHERE code = $3
                       )",
                    &[&ctx.org_id(), &role, &PERMISSION_QA_QUERY],
                )
                .await?;
                let denied_without_query = search::hydrate_chunks_by_identity(
                    txn,
                    &ctx,
                    &[collection],
                    std::slice::from_ref(&identity_active),
                    &historical_visibility,
                )
                .await?;
                assert!(
                    denied_without_query.is_empty(),
                    "historical hydration must recheck qa.query as well as qa.history"
                );
                txn.execute(
                    "INSERT INTO role_permissions (org_id, role_id, permission_id)
                     SELECT $1, $2, id FROM permissions WHERE code = $3",
                    &[&ctx.org_id(), &role, &PERMISSION_QA_QUERY],
                )
                .await?;

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

/// 1C-06: the FTS *candidate* leg must carry its own ACL predicate instead
/// of relying solely on the hydration re-check downstream. This test calls
/// `search::fts_search` directly with a `collection_ids` scope that
/// includes a collection the caller has no ACL grant on — i.e. it
/// deliberately bypasses `resolve_scope`'s allow-list intersection, the
/// same shape a stale cached `OrgContext.allowed_collection_ids` (context
/// cache TTL, or a long-lived ask-stream session) could produce. Before
/// this round `fts_search` had no ACL check at all and would have returned
/// the denied-collection candidate for hydration to filter out later; now
/// it must never surface it as a candidate in the first place.
///
/// Also covers the membership-`state` gap found while building the shared
/// ACL predicate: a *suspended* (not deleted) `org_memberships` row must
/// deny both the FTS candidate leg and hydration, matching 1C-02's
/// "suspended resolves like missing" invariant. The existing
/// `fts_rank_accent_fold_and_active_generation_gates` test only exercised
/// full membership deletion, which trivially denies via a missing JOIN row
/// regardless of whether `state` is checked.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn fts_candidate_leg_and_hydration_deny_acl_and_suspended_membership() {
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
    let other_user = Uuid::new_v4();
    let collection_allowed = Uuid::new_v4();
    let collection_denied = Uuid::new_v4();
    let role = Uuid::new_v4();
    let document_allowed = Uuid::new_v4();
    let document_denied = Uuid::new_v4();
    let version_allowed = Uuid::new_v4();
    let version_denied = Uuid::new_v4();
    let meta_allowed = Uuid::new_v4();
    let meta_denied = Uuid::new_v4();
    let chunk_allowed = Uuid::new_v4();
    let chunk_denied = Uuid::new_v4();
    let identity_allowed = sha64('1');
    let identity_denied = sha64('2');
    let ctx = OrgContext::try_new(
        org,
        user,
        [PERMISSION_QA_QUERY, PERMISSION_QA_HISTORY],
        [collection_allowed, collection_denied],
    )
    .unwrap();

    with_org_txn(&pool, &ctx, {
        let ctx = ctx.clone();
        let identity_allowed = identity_allowed.clone();
        let identity_denied = identity_denied.clone();
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO orgs (id, slug, name) VALUES ($1, $2, $3)",
                    &[&ctx.org_id(), &format!("org-{}", ctx.org_id()), &"org"],
                )
                .await?;
                for (id, label) in [(ctx.user_id(), "u"), (other_user, "o")] {
                    let email = format!("{id}@example.test");
                    txn.execute(
                        "INSERT INTO users (id, email, display_name, password_hash)
                         VALUES ($1, $2, $3, 'test-hash')",
                        &[&id, &email, &label],
                    )
                    .await?;
                }
                // Only the acting user is a member of the org; `other_user`
                // merely owns the denied collection, proving that ownership
                // by a non-member cannot leak access to a member either.
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

                // `collection_allowed`: acting user is the owner -> ACL grants access.
                txn.execute(
                    "INSERT INTO collections (
                        id, org_id, name, slug, owner_user_id, visibility
                     ) VALUES ($1, $2, 'allowed', $3, $4, 'private')",
                    &[
                        &collection_allowed,
                        &ctx.org_id(),
                        &format!("c-allowed-{collection_allowed}"),
                        &ctx.user_id(),
                    ],
                )
                .await?;
                // `collection_denied`: owned by someone else, private, no
                // direct grant to the acting user -> ACL must deny it, even
                // though it is included in this request's `collection_ids`.
                txn.execute(
                    "INSERT INTO collections (
                        id, org_id, name, slug, owner_user_id, visibility
                     ) VALUES ($1, $2, 'denied', $3, $4, 'private')",
                    &[
                        &collection_denied,
                        &ctx.org_id(),
                        &format!("c-denied-{collection_denied}"),
                        &other_user,
                    ],
                )
                .await?;

                for (doc, coll, version, meta, chunk, identity, tag) in [
                    (
                        document_allowed,
                        collection_allowed,
                        version_allowed,
                        meta_allowed,
                        chunk_allowed,
                        identity_allowed.clone(),
                        "allowed",
                    ),
                    (
                        document_denied,
                        collection_denied,
                        version_denied,
                        meta_denied,
                        chunk_denied,
                        identity_denied.clone(),
                        "denied",
                    ),
                ] {
                    txn.execute(
                        "INSERT INTO documents (
                            id, org_id, collection_id, title, state, created_by_user_id
                         ) VALUES ($1, $2, $3, $4, 'indexed', $5)",
                        &[&doc, &ctx.org_id(), &coll, &tag, &ctx.user_id()],
                    )
                    .await?;
                    let content_sha = sha64(tag.chars().next().unwrap());
                    txn.execute(
                        "INSERT INTO document_versions (
                            id, org_id, document_id, version_number, publication_state,
                            is_current, content_sha256, original_object_key, effective_from,
                            created_by_user_id
                         ) VALUES ($1,$2,$3,1,'published',true,$4,$5, now(), $6)",
                        &[
                            &version,
                            &ctx.org_id(),
                            &doc,
                            &content_sha,
                            &format!("k-{tag}"),
                            &ctx.user_id(),
                        ],
                    )
                    .await?;
                    txn.execute(
                        "UPDATE documents SET current_version_id = $1 WHERE id = $2",
                        &[&version, &doc],
                    )
                    .await?;
                    let sig = sha64(tag.chars().next().unwrap());
                    txn.execute(
                        "INSERT INTO index_metadata (
                            id, org_id, collection_id, index_signature_sha256,
                            embedding_family, embedding_revision, dimensions, runtime_path,
                            generation, is_active, state
                         ) VALUES ($1,$2,$3,$4,'f','r',8,'local-hash',1,true,'active')",
                        &[&meta, &ctx.org_id(), &coll, &sig],
                    )
                    .await?;
                    txn.execute(
                        "INSERT INTO chunks (
                            id, org_id, document_id, version_id, ordinal, heading_path, body,
                            chunk_identity_sha256, index_metadata_id, index_signature
                         ) VALUES ($1,$2,$3,$4,0,ARRAY['Đối soát'],
                                   'Đối soát giao dịch theo ngày',$5,$6,$7)",
                        &[
                            &chunk,
                            &ctx.org_id(),
                            &doc,
                            &version,
                            &identity,
                            &meta,
                            &sig,
                        ],
                    )
                    .await?;
                }

                // --- Candidate leg (FTS) must not surface the denied collection ---
                let hits = search::fts_search(
                    txn,
                    &ctx,
                    &[collection_allowed, collection_denied],
                    "Đối soát",
                    &VersionVisibility::Current,
                    10,
                )
                .await?;
                assert_eq!(
                    hits.len(),
                    1,
                    "FTS candidate leg must exclude the ACL-denied collection even when \
                     it is present in collection_ids"
                );
                assert_eq!(hits[0].chunk_id, chunk_allowed);

                // --- Hydration must agree (non-regression) ---
                let hydrated = search::hydrate_chunks_by_identity(
                    txn,
                    &ctx,
                    &[collection_allowed, collection_denied],
                    &[identity_allowed.clone(), identity_denied.clone()],
                    &VersionVisibility::Current,
                )
                .await?;
                assert_eq!(hydrated.len(), 1);
                assert_eq!(hydrated[0].chunk_id, chunk_allowed);

                // --- Suspended (not deleted) membership must deny both legs ---
                txn.execute(
                    "UPDATE org_memberships SET state = 'suspended'
                     WHERE org_id = $1 AND user_id = $2",
                    &[&ctx.org_id(), &ctx.user_id()],
                )
                .await?;
                let hits_suspended = search::fts_search(
                    txn,
                    &ctx,
                    &[collection_allowed],
                    "Đối soát",
                    &VersionVisibility::Current,
                    10,
                )
                .await?;
                assert!(
                    hits_suspended.is_empty(),
                    "FTS candidate leg must deny a suspended (not just deleted) membership"
                );
                let hydrated_suspended = search::hydrate_chunks_by_identity(
                    txn,
                    &ctx,
                    &[collection_allowed],
                    std::slice::from_ref(&identity_allowed),
                    &VersionVisibility::Current,
                )
                .await?;
                assert!(
                    hydrated_suspended.is_empty(),
                    "hydration must deny a suspended (not just deleted) membership"
                );

                // --- Reactivate: sanity check this isn't a broader false-deny bug ---
                txn.execute(
                    "UPDATE org_memberships SET state = 'active'
                     WHERE org_id = $1 AND user_id = $2",
                    &[&ctx.org_id(), &ctx.user_id()],
                )
                .await?;
                let hits_reactivated = search::fts_search(
                    txn,
                    &ctx,
                    &[collection_allowed],
                    "Đối soát",
                    &VersionVisibility::Current,
                    10,
                )
                .await?;
                assert_eq!(
                    hits_reactivated.len(),
                    1,
                    "reactivated membership must see its own collection again"
                );

                Ok(())
            })
        }
    })
    .await
    .expect("acl predicate fixture");

    ephemeral.drop().await;
}

/// Seeds one indexed, published, chunked, currently-active document in
/// `collection_id` under `ctx`'s org. `distinguishing_char` feeds the
/// content/identity/index-signature hashes so two docs seeded across two
/// different orgs (as in the 1C-12 cross-org test below) never collide on
/// `chunk_identity_sha256`.
async fn seed_indexed_chunk_doc(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    collection_id: Uuid,
    title: &str,
    body: &str,
    distinguishing_char: char,
) -> Result<(Uuid, Uuid), DbError> {
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let meta_id = Uuid::new_v4();
    let chunk_id = Uuid::new_v4();
    let sig = sha64(distinguishing_char);
    let identity = sha64(distinguishing_char);
    let content_sha = sha64(distinguishing_char);
    txn.execute(
        "INSERT INTO documents (
            id, org_id, collection_id, title, state, created_by_user_id
         ) VALUES ($1, $2, $3, $4, 'indexed', $5)",
        &[
            &document_id,
            &ctx.org_id(),
            &collection_id,
            &title,
            &ctx.user_id(),
        ],
    )
    .await?;
    txn.execute(
        "INSERT INTO document_versions (
            id, org_id, document_id, version_number, publication_state,
            is_current, content_sha256, original_object_key, effective_from,
            created_by_user_id
         ) VALUES ($1,$2,$3,1,'published',true,$4,$5, now(), $6)",
        &[
            &version_id,
            &ctx.org_id(),
            &document_id,
            &content_sha,
            &format!("k-{document_id}"),
            &ctx.user_id(),
        ],
    )
    .await?;
    txn.execute(
        "UPDATE documents SET current_version_id = $1 WHERE id = $2",
        &[&version_id, &document_id],
    )
    .await?;
    txn.execute(
        "INSERT INTO index_metadata (
            id, org_id, collection_id, index_signature_sha256, embedding_family,
            embedding_revision, dimensions, runtime_path, generation, is_active, state
         ) VALUES ($1,$2,$3,$4,'f','r',8,'local-hash',1,true,'active')",
        &[&meta_id, &ctx.org_id(), &collection_id, &sig],
    )
    .await?;
    txn.execute(
        "INSERT INTO chunks (
            id, org_id, document_id, version_id, ordinal, heading_path, body,
            chunk_identity_sha256, index_metadata_id, index_signature
         ) VALUES ($1,$2,$3,$4,0,$5,$6,$7,$8,$9)",
        &[
            &chunk_id,
            &ctx.org_id(),
            &document_id,
            &version_id,
            &vec![title.to_string()],
            &body,
            &identity,
            &meta_id,
            &sig,
        ],
    )
    .await?;
    Ok((document_id, chunk_id))
}

/// 1C-12 (B1): org A's FTS candidate + hydration legs must never surface
/// org B's chunks/documents — even when org B's own collection id is
/// explicitly included in the request's `collection_ids`, as an attacker
/// guessing/passing it would do. Both orgs' collections share the identical
/// name "Shared Docs" (see `TwoOrgFixture`), so this also proves the ACL
/// predicate scopes by `org_id` + collection UUID, never by name/slug.
///
/// This exercises the same low-level legs as
/// `fts_candidate_leg_and_hydration_deny_acl_and_suspended_membership`
/// above (which is single-org); the invariant proven here is different:
/// `c.org_id = $1` in both `fts_search` and `hydrate_chunks_by_identity`'s
/// SQL structurally cannot match a different org's collection row, no
/// matter what grants exist, because collection ids are looked up jointly
/// with `org_id` — there is no grant shape that could make this leak.
#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL"]
async fn cross_org_fts_search_never_returns_other_org_documents() {
    let Some(base_url) = test_database_url() else {
        return;
    };
    let ephemeral = EphemeralDb::create(&base_url).await;
    apply_migrations(&ephemeral.url)
        .await
        .expect("migrate ephemeral db");
    let pool = create_pool(&ephemeral.url).expect("pool");

    let fixture = common::multi_org_fixture::TwoOrgFixture::seed(&pool).await;

    let ctx_a = OrgContext::try_new(
        fixture.org_a,
        fixture.org_a_users.owner,
        [PERMISSION_QA_QUERY, PERMISSION_QA_HISTORY],
        [fixture.org_a_collections.shared_docs],
    )
    .unwrap();
    let ctx_b = OrgContext::try_new(
        fixture.org_b,
        fixture.org_b_users.owner,
        [PERMISSION_QA_QUERY, PERMISSION_QA_HISTORY],
        [fixture.org_b_collections.shared_docs],
    )
    .unwrap();

    let needle = "Đối soát giao dịch liên chi nhánh";
    let collection_a = fixture.org_a_collections.shared_docs;
    let collection_b = fixture.org_b_collections.shared_docs;

    let (doc_a, chunk_a) = with_org_txn(&pool, &ctx_a, {
        let ctx_a = ctx_a.clone();
        move |txn| {
            Box::pin(async move {
                seed_indexed_chunk_doc(txn, &ctx_a, collection_a, "Doc A", needle, 'a').await
            })
        }
    })
    .await
    .expect("seed org a doc");

    let (doc_b, chunk_b) = with_org_txn(&pool, &ctx_b, {
        let ctx_b = ctx_b.clone();
        move |txn| {
            Box::pin(async move {
                seed_indexed_chunk_doc(txn, &ctx_b, collection_b, "Doc B", needle, 'b').await
            })
        }
    })
    .await
    .expect("seed org b doc");

    with_org_txn(&pool, &ctx_a, {
        let ctx_a = ctx_a.clone();
        move |txn| {
            Box::pin(async move {
                // Org B's collection id is explicitly requested alongside
                // org A's own — must never widen org A's results.
                let hits = search::fts_search(
                    txn,
                    &ctx_a,
                    &[collection_a, collection_b],
                    needle,
                    &VersionVisibility::Current,
                    10,
                )
                .await?;
                assert_eq!(
                    hits.len(),
                    1,
                    "org A's FTS leg must never surface org B's chunk, even when \
                     org B's collection id is explicitly passed in the request"
                );
                assert_eq!(hits[0].chunk_id, chunk_a);
                assert_ne!(hits[0].chunk_id, chunk_b);
                assert_ne!(hits[0].document_id, doc_b);

                let hydrated = search::hydrate_chunks_by_identity(
                    txn,
                    &ctx_a,
                    &[collection_a, collection_b],
                    std::slice::from_ref(&hits[0].chunk_identity_sha256),
                    &VersionVisibility::Current,
                )
                .await?;
                assert_eq!(
                    hydrated.len(),
                    1,
                    "hydration must never surface org B's chunk either"
                );
                assert_eq!(hydrated[0].chunk_id, chunk_a);
                assert_eq!(hydrated[0].document_id, doc_a);
                Ok(())
            })
        }
    })
    .await
    .expect("cross-org fts/hydration assertions");

    ephemeral.drop().await;
}
