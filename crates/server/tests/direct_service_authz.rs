//! RED: direct-service / DB-entry authorization bypasses for Phase 1C Task 8.
//!
//! Each active permission below is exercised by calling the real service or
//! DB-facing entry with an otherwise valid same-org `OrgContext` that omits
//! only that permission. Assertions require a dedicated `PermissionDenied`
//! result (not bare `Ok` / `NotFound`) and zero relevant side effects.
//!
//! Authorized setup runs after the denial path so a missing fixture cannot
//! masquerade as a successful deny.
//!
//! Skips cleanly without live Postgres (`boot_app_pool`). Must run under
//! GitHub `rust-integration` with `--include-ignored`.

mod common;

use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::db::audit::AuditListFilter;
use fileconv_server::db::collections::{self, NewCollection};
use fileconv_server::db::documents::{self, NewDocument};
use fileconv_server::db::models::{CollectionVisibility, DocumentState, JobType, MembershipRole};
use fileconv_server::db::orgs;
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::jobs::{self, EnqueueJob, JobPayload};
use fileconv_server::services::access;
use fileconv_server::services::audit_query;
use fileconv_server::services::deletion::{self, DeleteRequestOutcome};
use fileconv_server::services::members;
use fileconv_server::services::publish;
use uuid::Uuid;

use common::{
    admin_database_url, app_database_url, boot_app_pool, seed_user_with_permissions,
    DualRoleEphemeralDb,
};

const PASSWORD: &str = "correct-password-1";
const CONTENT_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

async fn boot_pool() -> Option<(DualRoleEphemeralDb, Pool)> {
    let admin = admin_database_url()?;
    let app = app_database_url()?;
    Some(boot_app_pool(&admin, &app).await)
}

/// Fail closed unless the error is specifically permission-denied.
///
/// Accepting any `Err` (especially `NotFound`) would false-green routes that
/// hide existence or fail for fixture reasons.
fn assert_permission_denied<T, E>(result: Result<T, E>, label: &str)
where
    T: std::fmt::Debug,
    E: std::fmt::Debug,
{
    match result {
        Ok(value) => {
            panic!("{label}: expected PermissionDenied, got Ok({value:?}) — service authz bypass")
        }
        Err(err) => {
            let rendered = format!("{err:?}");
            assert!(
                rendered.contains("PermissionDenied"),
                "{label}: expected PermissionDenied, got {err:?} \
                 (not generic success/not-found)"
            );
        }
    }
}

async fn document_tombstone_snapshot(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
) -> (String, bool) {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT state, deleted_at IS NOT NULL
                         FROM documents WHERE org_id = $1 AND id = $2",
                        &[&ctx.org_id(), &document_id],
                    )
                    .await?;
                let state: String = row.get(0);
                let deleted: bool = row.get(1);
                Ok((state, deleted))
            })
        }
    })
    .await
    .expect("document snapshot")
}

async fn count_document_tombstone_audits(pool: &Pool, ctx: &OrgContext, document_id: Uuid) -> i64 {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint FROM audit_log
                         WHERE org_id = $1
                           AND action = 'document.tombstone'
                           AND resource_id = $2",
                        &[&ctx.org_id(), &document_id.to_string()],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("tombstone audit count")
}

async fn count_outbox_for_document(pool: &Pool, ctx: &OrgContext, document_id: Uuid) -> i64 {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let key = format!("document.delete_requested.{document_id}");
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint FROM outbox_events
                         WHERE org_id = $1 AND idempotency_key = $2",
                        &[&ctx.org_id(), &key],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("outbox count")
}

async fn version_publication_snapshot(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    version_id: Uuid,
) -> (String, bool) {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT publication_state, is_current
                         FROM document_versions
                         WHERE org_id = $1 AND document_id = $2 AND id = $3",
                        &[&ctx.org_id(), &document_id, &version_id],
                    )
                    .await?;
                let state: String = row.get(0);
                let is_current: bool = row.get(1);
                Ok((state, is_current))
            })
        }
    })
    .await
    .expect("version snapshot")
}

async fn count_action_audits(pool: &Pool, ctx: &OrgContext, action: &str) -> i64 {
    let action = action.to_string();
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_one(
                        "SELECT count(*)::bigint FROM audit_log
                         WHERE org_id = $1 AND action = $2",
                        &[&ctx.org_id(), &action],
                    )
                    .await?;
                Ok(row.get(0))
            })
        }
    })
    .await
    .expect("audit action count")
}

async fn membership_role(pool: &Pool, ctx: &OrgContext, user_id: Uuid) -> Option<String> {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_opt(
                        "SELECT role FROM org_memberships
                         WHERE org_id = $1 AND user_id = $2",
                        &[&ctx.org_id(), &user_id],
                    )
                    .await?;
                Ok(row.map(|r| r.get::<_, String>(0)))
            })
        }
    })
    .await
    .expect("membership role")
}

async fn seed_org_document_draft(
    pool: &Pool,
    org: Uuid,
    user: Uuid,
    permissions: &[&str],
) -> (OrgContext, Uuid, Uuid, Uuid) {
    seed_user_with_permissions(
        pool,
        org,
        user,
        &format!("{user}@direct-authz.test"),
        PASSWORD,
        permissions,
    )
    .await;
    let collection_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let version_id = Uuid::new_v4();
    let ctx = OrgContext::try_new(org, user, permissions.iter().copied(), [collection_id]).unwrap();
    with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: collection_id,
                        name: "Direct Authz Collection",
                        slug: &format!("direct-authz-{}", collection_id.simple()),
                        description: Some("task-8"),
                        visibility: CollectionVisibility::Org,
                    },
                )
                .await?;
                documents::insert(
                    txn,
                    &ctx,
                    NewDocument {
                        id: document_id,
                        collection_id,
                        title: "Direct Authz Doc",
                    },
                )
                .await?;
                // Draft + not current — valid input for markhand_publish_document_version.
                txn.execute(
                    "INSERT INTO document_versions (
                        id, org_id, document_id, version_number, publication_state,
                        is_current, content_sha256, original_object_key,
                        source_content_type, byte_size, created_by_user_id
                     ) VALUES ($1,$2,$3,1,'draft',false,$4,$5,'text/plain',12,$6)",
                    &[
                        &version_id,
                        &ctx.org_id(),
                        &document_id,
                        &CONTENT_SHA.to_string(),
                        &format!("orgs/{}/objects/direct-authz.bin", ctx.org_id()),
                        &ctx.user_id(),
                    ],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed draft document");
    (ctx, collection_id, document_id, version_id)
}

async fn seed_viewer_member(pool: &Pool, org: Uuid, owner: Uuid, viewer: Uuid) {
    let ctx = OrgContext::try_new(org, owner, ["member.manage"], []).unwrap();
    with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                orgs::ensure_user(
                    txn,
                    &ctx,
                    viewer,
                    &format!("{viewer}@direct-authz-viewer.test"),
                    "Viewer",
                )
                .await?;
                txn.execute(
                    "INSERT INTO org_memberships (org_id, user_id, role)
                     VALUES ($1, $2, 'viewer')
                     ON CONFLICT (org_id, user_id) DO NOTHING",
                    &[&org, &viewer],
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .expect("seed viewer member");
}

// ---------------------------------------------------------------------
// doc.delete — services::deletion::request_delete
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn doc_delete_permission_required_at_deletion_service() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };

    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let (full_ctx, collection_id, document_id, _version_id) =
        seed_org_document_draft(&pool, org, user, &["qa.query", "doc.upload", "doc.delete"]).await;

    let missing_delete =
        OrgContext::try_new(org, user, ["qa.query", "doc.upload"], [collection_id]).unwrap();

    let before_state = document_tombstone_snapshot(&pool, &full_ctx, document_id).await;
    let before_audits = count_document_tombstone_audits(&pool, &full_ctx, document_id).await;
    let before_outbox = count_outbox_for_document(&pool, &full_ctx, document_id).await;
    assert_eq!(before_state.0, DocumentState::Uploaded.as_str());
    assert!(!before_state.1);

    let denied = deletion::request_delete(&pool, &missing_delete, document_id).await;
    assert_permission_denied(denied, "request_delete without doc.delete");

    let after_state = document_tombstone_snapshot(&pool, &full_ctx, document_id).await;
    let after_audits = count_document_tombstone_audits(&pool, &full_ctx, document_id).await;
    let after_outbox = count_outbox_for_document(&pool, &full_ctx, document_id).await;
    assert_eq!(
        after_state, before_state,
        "denied delete must not tombstone the document"
    );
    assert_eq!(
        after_audits, before_audits,
        "denied delete must not write document.tombstone audit"
    );
    assert_eq!(
        after_outbox, before_outbox,
        "denied delete must not enqueue document.delete_requested outbox"
    );

    // Authorized precondition: the same document is deletable when permission is present.
    let allowed = deletion::request_delete(&pool, &full_ctx, document_id)
        .await
        .expect("authorized request_delete must succeed — fixture was valid");
    assert!(matches!(
        allowed,
        DeleteRequestOutcome::Requested(_) | DeleteRequestOutcome::AlreadyRequested(_)
    ));
    let (state, deleted) = document_tombstone_snapshot(&pool, &full_ctx, document_id).await;
    assert_eq!(state, DocumentState::Tombstoned.as_str());
    assert!(deleted);

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// doc.publish — services::publish::publish_version (authz + atomic audit)
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn doc_publish_permission_required_at_direct_db_publish() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };

    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let (full_ctx, collection_id, document_id, version_id) =
        seed_org_document_draft(&pool, org, user, &["qa.query", "doc.upload", "doc.publish"]).await;

    let missing_publish =
        OrgContext::try_new(org, user, ["qa.query", "doc.upload"], [collection_id]).unwrap();

    let before_pub = version_publication_snapshot(&pool, &full_ctx, document_id, version_id).await;
    let before_audit = count_action_audits(&pool, &full_ctx, "document.publish").await;
    assert_eq!(before_pub.0, "draft");
    assert!(!before_pub.1);

    let denied = publish::publish_version(
        &pool,
        &missing_publish,
        &Uuid::new_v4().to_string(),
        document_id,
        version_id,
    )
    .await;
    assert_permission_denied(denied, "publish_version without doc.publish");

    let after_pub = version_publication_snapshot(&pool, &full_ctx, document_id, version_id).await;
    let after_audit = count_action_audits(&pool, &full_ctx, "document.publish").await;
    assert_eq!(
        after_pub, before_pub,
        "denied publish must leave the version draft/non-current"
    );
    assert_eq!(
        after_audit, before_audit,
        "denied publish must not write document.publish audit"
    );

    // Authorized path must publish and co-commit document.publish audit.
    publish::publish_version(
        &pool,
        &full_ctx,
        &Uuid::new_v4().to_string(),
        document_id,
        version_id,
    )
    .await
    .expect("authorized publish_version must succeed — fixture was valid");

    let (state, is_current) =
        version_publication_snapshot(&pool, &full_ctx, document_id, version_id).await;
    assert_eq!(state, "published");
    assert!(is_current);

    let publish_audits = count_action_audits(&pool, &full_ctx, "document.publish").await;
    assert!(
        publish_audits > before_audit,
        "authorized publish must record document.publish audit"
    );

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// member.manage — services::members::{change_role, remove_member}
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn member_manage_permission_required_at_direct_service_patch_and_delete() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };

    let org = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let viewer = Uuid::new_v4();
    seed_user_with_permissions(
        &pool,
        org,
        owner,
        "owner@direct-member-authz.test",
        PASSWORD,
        &["member.manage"],
    )
    .await;
    seed_viewer_member(&pool, org, owner, viewer).await;

    let authorized = OrgContext::try_new(org, owner, ["member.manage"], []).unwrap();
    let missing_manage = OrgContext::try_new(org, owner, ["qa.query"], []).unwrap();

    let before_role = membership_role(&pool, &authorized, viewer)
        .await
        .expect("viewer membership must exist before deny probes");
    assert_eq!(before_role, "viewer");
    let before_role_audits = count_action_audits(&pool, &authorized, "member.role_change").await;
    let before_remove_audits = count_action_audits(&pool, &authorized, "member.remove").await;

    let denied_patch = members::change_role(
        &pool,
        &missing_manage,
        &Uuid::new_v4().to_string(),
        viewer,
        MembershipRole::Editor,
    )
    .await;
    assert_permission_denied(denied_patch, "change_role without member.manage");

    let after_patch_role = membership_role(&pool, &authorized, viewer)
        .await
        .expect("viewer must still exist after denied patch");
    assert_eq!(
        after_patch_role, before_role,
        "denied change_role must not mutate membership role"
    );
    assert_eq!(
        count_action_audits(&pool, &authorized, "member.role_change").await,
        before_role_audits,
        "denied change_role must not write member.role_change success audit"
    );

    let denied_delete =
        members::remove_member(&pool, &missing_manage, &Uuid::new_v4().to_string(), viewer).await;
    assert_permission_denied(denied_delete, "remove_member without member.manage");

    assert_eq!(
        membership_role(&pool, &authorized, viewer).await.as_deref(),
        Some("viewer"),
        "denied remove_member must not delete the membership row"
    );
    assert_eq!(
        count_action_audits(&pool, &authorized, "member.remove").await,
        before_remove_audits,
        "denied remove_member must not write member.remove success audit"
    );

    // Authorized precondition: patch then delete succeed against the live row.
    members::change_role(
        &pool,
        &authorized,
        &Uuid::new_v4().to_string(),
        viewer,
        MembershipRole::Editor,
    )
    .await
    .expect("authorized change_role must succeed — fixture was valid");
    assert_eq!(
        membership_role(&pool, &authorized, viewer).await.as_deref(),
        Some("editor")
    );
    members::remove_member(&pool, &authorized, &Uuid::new_v4().to_string(), viewer)
        .await
        .expect("authorized remove_member must succeed — fixture was valid");
    assert!(
        membership_role(&pool, &authorized, viewer).await.is_none(),
        "authorized remove must delete the membership"
    );

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// audit.view — services::audit_query::list_page
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn audit_view_permission_required_at_direct_list_page() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };

    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    seed_user_with_permissions(
        &pool,
        org,
        user,
        "auditor@direct-audit-authz.test",
        PASSWORD,
        &["member.manage", "audit.view"],
    )
    .await;

    let authorized = OrgContext::try_new(org, user, ["member.manage", "audit.view"], []).unwrap();
    let missing_view = OrgContext::try_new(org, user, ["member.manage"], []).unwrap();

    // Seed at least one audit row via an authorized member mutation so list_page
    // has real data (cannot pass merely because the log is empty).
    let viewer = Uuid::new_v4();
    seed_viewer_member(&pool, org, user, viewer).await;
    members::change_role(
        &pool,
        &authorized,
        &Uuid::new_v4().to_string(),
        viewer,
        MembershipRole::Editor,
    )
    .await
    .expect("seed audit row via authorized role change");
    let seeded_rows = audit_query::list_page(
        &pool,
        &authorized,
        &AuditListFilter::default(),
        10,
        None,
        None,
    )
    .await
    .expect("authorized list_page precondition");
    assert!(
        !seeded_rows.is_empty(),
        "authorized audit list must return seeded rows — fixture was valid"
    );
    let before_read_audits = count_action_audits(&pool, &authorized, "audit.read").await;

    let denied = audit_query::list_page(
        &pool,
        &missing_view,
        &AuditListFilter::default(),
        10,
        None,
        None,
    )
    .await;
    assert_permission_denied(denied, "list_page without audit.view");

    assert_eq!(
        count_action_audits(&pool, &authorized, "audit.read").await,
        before_read_audits,
        "denied direct audit list must not record audit.read success"
    );

    ephemeral.drop().await;
}

// ---------------------------------------------------------------------
// jobs.system — services::access::resolve_job_access (documentless jobs)
// ---------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]
async fn jobs_system_permission_required_for_documentless_job_access() {
    let Some((ephemeral, pool)) = boot_pool().await else {
        return;
    };

    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    seed_user_with_permissions(
        &pool,
        org,
        user,
        "jobs@direct-jobs-authz.test",
        PASSWORD,
        &["qa.query", "jobs.system"],
    )
    .await;

    let authorized = OrgContext::try_new(org, user, ["qa.query", "jobs.system"], []).unwrap();
    let missing_system = OrgContext::try_new(org, user, ["qa.query"], []).unwrap();

    let enqueued = jobs::enqueue(
        &pool,
        &authorized,
        EnqueueJob::new(
            JobType::Reconcile,
            JobPayload::default(),
            format!("direct-authz-system-job-{}", Uuid::new_v4().simple()),
        ),
    )
    .await
    .expect("enqueue documentless reconcile job");
    let job_id = enqueued.job.id;
    assert!(
        enqueued.job.document_id.is_none(),
        "system-job fixture must be documentless"
    );

    // Authorized precondition first: proves the job is visible with jobs.system.
    let visible = access::resolve_job_access(&pool, &authorized, job_id)
        .await
        .expect("authorized resolve_job_access must succeed — fixture was valid");
    assert_eq!(visible.id, job_id);

    let denied = access::resolve_job_access(&pool, &missing_system, job_id).await;
    // Must be PermissionDenied — NotFound would hide a same-org system job and
    // is the current (incorrect for direct-service) behavior.
    assert_permission_denied(denied, "resolve_job_access without jobs.system");

    ephemeral.drop().await;
}
