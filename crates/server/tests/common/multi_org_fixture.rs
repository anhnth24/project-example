//! Shared two-organization fixture for the 1C-12 "multi-org denial" suite.
//!
//! Seeds two fully independent orgs (org A / org B), each with four
//! principals (owner/admin/member/viewer) and three collections whose
//! *names* are deliberately duplicated across the two orgs ("Shared Docs",
//! "Private Notes", "Team Space" exist verbatim in both org A and org B, as
//! different rows/ids) so name/slug-based lookup bugs have something real to
//! trip over. Visibility is mixed per org (`org`/`private`/`groups`) so
//! ACL-shaped cross-org tests have real grant plumbing to probe instead of
//! two empty orgs.
//!
//! Every existing cross-org test in this suite (see `members.rs`,
//! `storage.rs`, `jobs.rs`, `chat_history.rs`, ...) builds its own ad-hoc
//! `org_a`/`org_b` pair inline; this module exists so 1C-12's new tests (and
//! future ones) share one reviewed fixture instead of N more copies of the
//! same boilerplate. See `plans/.../1c-12-scope.md`'s gap-analysis item 1.

use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::auth::jwt::JwtKeys;
use fileconv_server::auth::session;
use fileconv_server::db::models::AccessLevel;
use fileconv_server::db::orgs;
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::services::download::{issue_capability, CapabilityKeys, DownloadPurpose};
use tokio_postgres::Transaction;
use uuid::Uuid;

use super::acl_fixture::{grant_group_access, grant_role_access};
use super::{seed_user_with_permissions, test_auth_config};

pub const PASSWORD: &str = "correct-password-1";

/// Permissions granted to the `owner` role in each seeded org.
pub const OWNER_PERMISSIONS: &[&str] = &[
    "qa.query",
    "qa.history",
    "doc.upload",
    "doc.delete",
    "member.manage",
];
/// Permissions granted to the `admin` role.
pub const ADMIN_PERMISSIONS: &[&str] = &["qa.query", "qa.history", "doc.upload", "doc.delete"];
/// Permissions granted to the `member` role.
pub const MEMBER_PERMISSIONS: &[&str] = &["qa.query", "doc.upload"];
/// Permissions granted to the `viewer` role.
pub const VIEWER_PERMISSIONS: &[&str] = &["qa.query"];

/// Four seeded principals for one org, one per role tier.
#[derive(Debug, Clone, Copy)]
pub struct OrgUsers {
    pub owner: Uuid,
    pub admin: Uuid,
    pub member: Uuid,
    pub viewer: Uuid,
}

/// Three seeded collections for one org (see module docs: names are
/// duplicated verbatim on the sibling org, only ids differ).
#[derive(Debug, Clone, Copy)]
pub struct OrgCollections {
    /// `visibility = 'org'`, owned by the org's owner — any member can reach it.
    pub shared_docs: Uuid,
    /// `visibility = 'private'`, owned by the owner, no grants to anyone else.
    pub private_notes: Uuid,
    /// `visibility = 'groups'`, reachable via `group_id` (member, write) or
    /// `viewer_role_id` (viewer, read).
    pub team_space: Uuid,
    pub group_id: Uuid,
    pub viewer_role_id: Uuid,
}

/// Two fully independent orgs, each with [`OrgUsers`] + [`OrgCollections`].
#[derive(Debug, Clone, Copy)]
pub struct TwoOrgFixture {
    pub org_a: Uuid,
    pub org_b: Uuid,
    pub org_a_users: OrgUsers,
    pub org_b_users: OrgUsers,
    pub org_a_collections: OrgCollections,
    pub org_b_collections: OrgCollections,
}

impl TwoOrgFixture {
    /// Seeds both orgs into `pool`. Works with any already-migrated pool —
    /// the dual-role app pool from [`super::boot_app_pool`] or a
    /// single admin-role pool, whichever the calling test file already uses
    /// (both patterns exist across this suite; this fixture does not care).
    pub async fn seed(pool: &Pool) -> Self {
        let (org_a, org_a_users, org_a_collections) = seed_one_org(pool, "org-a").await;
        let (org_b, org_b_users, org_b_collections) = seed_one_org(pool, "org-b").await;
        Self {
            org_a,
            org_b,
            org_a_users,
            org_b_users,
            org_a_collections,
            org_b_collections,
        }
    }
}

async fn seed_one_org(pool: &Pool, label: &str) -> (Uuid, OrgUsers, OrgCollections) {
    let org = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let admin = Uuid::new_v4();
    let member = Uuid::new_v4();
    let viewer = Uuid::new_v4();

    // Creates the org + quotas + the owner user, and grants OWNER_PERMISSIONS
    // to the (hardcoded, see `seed_user_with_permissions`'s own doc comment)
    // 'owner' role code. That helper is written for single-principal
    // fixtures, so admin/member/viewer below are seeded by hand instead of
    // reusing it three more times (which would just re-touch the same
    // 'owner' role for every call).
    seed_user_with_permissions(
        pool,
        org,
        owner,
        &format!("{label}-owner-{}@multi-org-fixture.test", owner.simple()),
        PASSWORD,
        OWNER_PERMISSIONS,
    )
    .await;

    let owner_ctx = OrgContext::try_new(org, owner, OWNER_PERMISSIONS.iter().copied(), []).unwrap();

    for (user_id, tag) in [(admin, "admin"), (member, "member"), (viewer, "viewer")] {
        let email = format!("{label}-{tag}-{}@multi-org-fixture.test", user_id.simple());
        with_org_txn(pool, &owner_ctx, {
            let owner_ctx = owner_ctx.clone();
            let email = email.clone();
            let tag = tag.to_string();
            move |txn| Box::pin(async move { orgs::ensure_user(txn, &owner_ctx, user_id, &email, &tag).await })
        })
        .await
        .expect("ensure extra fixture user");
        session::set_password_hash(pool, user_id, PASSWORD, &test_auth_config().argon2)
            .await
            .expect("set password for extra fixture user");
    }

    let group_id = Uuid::new_v4();
    let (viewer_role_id, shared_docs, private_notes, team_space) = with_org_txn(pool, &owner_ctx, {
        let owner_ctx = owner_ctx.clone();
        move |txn| {
            Box::pin(async move {
                create_role(txn, org, "admin", "Admin", ADMIN_PERMISSIONS).await?;
                // `org_memberships.role` is constrained to
                // ('owner','admin','editor','viewer') by migration 0001/0003
                // (`org_memberships_role_check`) — there is no 'member' role
                // code in that check constraint, so the app-level "member"
                // tier is stored as 'editor' at the DB layer everywhere
                // (`roles.code` here and `org_memberships.role` below); the
                // Rust-level field name stays `member` for readability.
                create_role(txn, org, "editor", "Member", MEMBER_PERMISSIONS).await?;
                let viewer_role_id =
                    create_role(txn, org, "viewer", "Viewer", VIEWER_PERMISSIONS).await?;

                for (user_id, role_code) in
                    [(admin, "admin"), (member, "editor"), (viewer, "viewer")]
                {
                    txn.execute(
                        "INSERT INTO org_memberships (org_id, user_id, role)
                         VALUES ($1, $2, $3)
                         ON CONFLICT (org_id, user_id) DO UPDATE SET role = EXCLUDED.role",
                        &[&owner_ctx.org_id(), &user_id, &role_code],
                    )
                    .await?;
                }

                txn.execute(
                    "INSERT INTO groups (id, org_id, name) VALUES ($1, $2, 'Team')",
                    &[&group_id, &org],
                )
                .await?;
                txn.execute(
                    "INSERT INTO group_memberships (org_id, group_id, user_id)
                     VALUES ($1, $2, $3)",
                    &[&org, &group_id, &member],
                )
                .await?;

                // Duplicate names on purpose across org A / org B — see module docs.
                let shared_docs =
                    insert_named_collection(txn, org, owner, "org", "Shared Docs", "shared-docs")
                        .await?;
                let private_notes = insert_named_collection(
                    txn,
                    org,
                    owner,
                    "private",
                    "Private Notes",
                    "private-notes",
                )
                .await?;
                let team_space =
                    insert_named_collection(txn, org, owner, "groups", "Team Space", "team-space")
                        .await?;
                grant_group_access(txn, org, team_space, group_id, AccessLevel::Write).await?;
                grant_role_access(txn, org, team_space, viewer_role_id, AccessLevel::Read).await?;

                Ok((viewer_role_id, shared_docs, private_notes, team_space))
            })
        }
    })
    .await
    .expect("seed org roles/collections");

    (
        org,
        OrgUsers {
            owner,
            admin,
            member,
            viewer,
        },
        OrgCollections {
            shared_docs,
            private_notes,
            team_space,
            group_id,
            viewer_role_id,
        },
    )
}

async fn insert_named_collection(
    txn: &Transaction<'_>,
    org: Uuid,
    owner_user_id: Uuid,
    visibility: &str,
    name: &str,
    slug_prefix: &str,
) -> Result<Uuid, tokio_postgres::Error> {
    let id = Uuid::new_v4();
    let slug = format!("{slug_prefix}-{}", &id.simple().to_string()[..8]);
    txn.execute(
        "INSERT INTO collections (id, org_id, name, slug, owner_user_id, visibility)
         VALUES ($1, $2, $3, $4, $5, $6)",
        &[&id, &org, &name, &slug, &owner_user_id, &visibility],
    )
    .await?;
    Ok(id)
}

async fn create_role(
    txn: &Transaction<'_>,
    org: Uuid,
    code: &str,
    name: &str,
    permissions: &[&str],
) -> Result<Uuid, tokio_postgres::Error> {
    let role_id = Uuid::new_v4();
    txn.execute(
        "INSERT INTO roles (id, org_id, code, name, is_system)
         VALUES ($1, $2, $3, $4, true)
         ON CONFLICT (org_id, code) DO NOTHING",
        &[&role_id, &org, &code, &name],
    )
    .await?;
    let role_id: Uuid = txn
        .query_one(
            "SELECT id FROM roles WHERE org_id = $1 AND code = $2",
            &[&org, &code],
        )
        .await?
        .get(0);
    for permission in permissions {
        txn.execute(
            "INSERT INTO permissions (id, code, description)
             VALUES ($1, $2, $2)
             ON CONFLICT (code) DO NOTHING",
            &[&Uuid::new_v4(), permission],
        )
        .await?;
        let perm_id: Uuid = txn
            .query_one("SELECT id FROM permissions WHERE code = $1", &[permission])
            .await?
            .get(0);
        txn.execute(
            "INSERT INTO role_permissions (org_id, role_id, permission_id)
             VALUES ($1, $2, $3)
             ON CONFLICT DO NOTHING",
            &[&org, &role_id, &perm_id],
        )
        .await?;
    }
    Ok(role_id)
}

/// Mints a structurally valid access token (good signature, unexpired,
/// correct issuer/audience/kid — matches [`super::test_auth_config`]) for
/// `user_id` in `org_id`.
///
/// [Inference] Access tokens in this codebase are stateless — `auth::jwt`'s
/// own doc comment on `AccessClaims` says "org_id / sid are hints only —
/// authorization loads from PG". Nothing is looked up or revoked in the DB
/// at mint time *or* at verify time. So a "stale/revoked" token is not a
/// distinct token shape; it is an ordinary token for a principal whose
/// `org_memberships` row the caller has since suspended/removed, e.g. via
/// `UPDATE org_memberships SET state = 'suspended' WHERE org_id = $1 AND
/// user_id = $2` (see `retrieval.rs`'s
/// `fts_candidate_leg_and_hydration_deny_acl_and_suspended_membership` test
/// for the exact, already-established pattern this mirrors). The token
/// decodes and verifies fine regardless; it is the ACL-check layer
/// (`auth::permissions::resolve_org_context_in_txn`, re-run fresh on every
/// request) that must deny it. Callers seed the "stale" DB condition
/// themselves and use this helper only to mint the token that should then be
/// rejected.
pub fn mint_stale_token(org_id: Uuid, user_id: Uuid) -> String {
    let keys = JwtKeys::from_auth(&test_auth_config()).expect("jwt keys from test auth config");
    keys.sign_access_token(user_id, org_id, Uuid::new_v4())
        .expect("sign stale token")
        .expose()
        .to_string()
}

/// Issues a download capability for `document_id`/`version_id` as `user_id`
/// acting under `org_id`'s context, scoped to `collection_id`.
///
/// [Unverified deviation from the original scope note] The 1C-12 scope
/// describes this helper as `issue_download_capability_as(org, doc, user) ->
/// String`. The real `services::download::issue_capability` requires a live
/// `Pool` (it re-checks ACL + publication state fresh from Postgres on every
/// issuance), a `CapabilityKeys` signer, and the document's `version_id` —
/// none of which have a safe implicit default, so they are explicit
/// parameters here rather than hidden globals. `collection_id` is passed so
/// the constructed `OrgContext` carries it in its allow-list, matching this
/// suite's direct-service-layer test pattern (see
/// `tests/direct_service_authz.rs`) rather than a full DB ACL resolve.
pub async fn issue_download_capability_as(
    pool: &Pool,
    keys: &CapabilityKeys,
    org_id: Uuid,
    collection_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
    user_id: Uuid,
) -> String {
    let ctx = OrgContext::try_new(org_id, user_id, ["qa.query", "qa.history"], [collection_id])
        .expect("org context for capability issuance");
    let issued = issue_capability(
        pool,
        &ctx,
        keys,
        document_id,
        version_id,
        DownloadPurpose::Original,
        None,
    )
    .await
    .expect("issue capability under org context");
    issued.token.expose().to_string()
}
