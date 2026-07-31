//! Shared ACL integration fixtures for Phase 1C resolver / SQL equivalence tests.
//!
//! Seeds real PostgreSQL rows (groups, grants, memberships) and compares
//! `resolve_org_context_in_txn` against `db::acl_sql::allowed_collections_sql`.

use std::collections::BTreeSet;

use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::auth::permissions::resolve_org_context_in_txn;
use fileconv_server::db::acl_sql::{acl_predicate_sql, allowed_collections_sql};
use fileconv_server::db::error::DbError;
use fileconv_server::db::models::AccessLevel;
use fileconv_server::db::pool::with_org_txn;
use tokio_postgres::Transaction;
use uuid::Uuid;

use super::{admin_database_url, app_database_url, boot_app_pool, seed_user_with_permissions};

pub const PERMISSION_QA_QUERY: &str = "qa.query";
pub const PASSWORD: &str = "correct-password-1";

/// One org with owner, acting member, secondary user, group, and viewer role.
#[derive(Debug, Clone)]
pub struct AclOrgFixture {
    pub org: Uuid,
    pub owner: Uuid,
    pub member: Uuid,
    pub other_user: Uuid,
    pub group_id: Uuid,
    pub viewer_role_id: Uuid,
}

/// Collection ids keyed by scenario label for matrix assertions.
#[derive(Debug, Clone, Default)]
pub struct AclCollectionMatrix {
    pub org_visible: Uuid,
    pub private_owned: Uuid,
    pub private_foreign: Uuid,
    pub private_user_grant: Uuid,
    /// Private collection used by `private_visibility_ignores_group_and_role_grants`
    /// to prove migration 0036 rejects dormant group grants (never seeded here).
    pub private_group_leak: Uuid,
    /// Private collection used by `private_visibility_ignores_group_and_role_grants`
    /// to prove migration 0036 rejects dormant role grants (never seeded here).
    pub private_role_leak: Uuid,
    pub groups_via_group: Uuid,
    pub groups_via_role: Uuid,
    pub groups_denied: Uuid,
    pub groups_read_grant: Uuid,
    pub containment: Uuid,
}

pub async fn boot_acl_pool() -> Option<(super::DualRoleEphemeralDb, Pool)> {
    let admin = admin_database_url()?;
    let app = app_database_url()?;
    Some(boot_app_pool(&admin, &app).await)
}

/// Seeds org/users/roles/group; `member` and `other_user` receive `qa.query`.
pub async fn seed_acl_org(pool: &Pool) -> AclOrgFixture {
    let org = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let member = Uuid::new_v4();
    let other_user = Uuid::new_v4();
    let group_id = Uuid::new_v4();
    let viewer_role_id = Uuid::new_v4();

    seed_user_with_permissions(
        pool,
        org,
        owner,
        &format!("owner-{}@acl-fixture.test", owner.simple()),
        PASSWORD,
        &[PERMISSION_QA_QUERY],
    )
    .await;
    seed_user_with_permissions(
        pool,
        org,
        member,
        &format!("member-{}@acl-fixture.test", member.simple()),
        PASSWORD,
        &[PERMISSION_QA_QUERY],
    )
    .await;
    seed_user_with_permissions(
        pool,
        org,
        other_user,
        &format!("other-{}@acl-fixture.test", other_user.simple()),
        PASSWORD,
        &[PERMISSION_QA_QUERY],
    )
    .await;

    let owner_ctx = OrgContext::try_new(org, owner, [PERMISSION_QA_QUERY], []).unwrap();
    let org_id = org;
    let resolved_viewer_role_id = with_org_txn(pool, &owner_ctx, {
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "INSERT INTO groups (id, org_id, name) VALUES ($1, $2, 'Editors')",
                    &[&group_id, &org_id],
                )
                .await?;
                txn.execute(
                    "INSERT INTO roles (id, org_id, code, name, is_system)
                     VALUES ($1, $2, 'viewer', 'Viewer', true)
                     ON CONFLICT (org_id, code) DO NOTHING",
                    &[&viewer_role_id, &org_id],
                )
                .await?;
                let resolved_viewer_role_id: Uuid = txn
                    .query_one(
                        "SELECT id FROM roles WHERE org_id = $1 AND code = 'viewer'",
                        &[&org_id],
                    )
                    .await?
                    .get(0);
                txn.execute(
                    "INSERT INTO role_permissions (org_id, role_id, permission_id)
                     SELECT $1, $2, p.id
                     FROM permissions p
                     WHERE p.code = $3
                     ON CONFLICT DO NOTHING",
                    &[&org_id, &resolved_viewer_role_id, &PERMISSION_QA_QUERY],
                )
                .await?;
                txn.execute(
                    "INSERT INTO group_memberships (org_id, group_id, user_id)
                     VALUES ($1, $2, $3)",
                    &[&org_id, &group_id, &member],
                )
                .await?;
                txn.execute(
                    "INSERT INTO org_memberships (org_id, user_id, role)
                     VALUES ($1, $2, 'viewer')
                     ON CONFLICT (org_id, user_id) DO UPDATE SET role = EXCLUDED.role",
                    &[&org_id, &member],
                )
                .await?;
                Ok(resolved_viewer_role_id)
            })
        }
    })
    .await
    .expect("seed group and viewer role");

    AclOrgFixture {
        org,
        owner,
        member,
        other_user,
        group_id,
        viewer_role_id: resolved_viewer_role_id,
    }
}

pub async fn insert_collection(
    txn: &Transaction<'_>,
    org: Uuid,
    owner_user_id: Uuid,
    visibility: &str,
    slug_suffix: &str,
) -> Result<Uuid, tokio_postgres::Error> {
    let id = Uuid::new_v4();
    let slug = format!("acl-{slug_suffix}-{}", &id.simple().to_string()[..8]);
    let name = format!("ACL {slug_suffix}");
    txn.execute(
        "INSERT INTO collections (id, org_id, name, slug, owner_user_id, visibility)
         VALUES ($1, $2, $3, $4, $5, $6)",
        &[&id, &org, &name, &slug, &owner_user_id, &visibility],
    )
    .await?;
    Ok(id)
}

pub async fn grant_user_access(
    txn: &Transaction<'_>,
    org: Uuid,
    collection_id: Uuid,
    user_id: Uuid,
    access_level: AccessLevel,
) -> Result<(), tokio_postgres::Error> {
    txn.execute(
        "INSERT INTO collection_user_access (id, org_id, collection_id, user_id, access_level)
         VALUES ($1, $2, $3, $4, $5)",
        &[
            &Uuid::new_v4(),
            &org,
            &collection_id,
            &user_id,
            &access_level.as_str(),
        ],
    )
    .await?;
    Ok(())
}

pub async fn grant_group_access(
    txn: &Transaction<'_>,
    org: Uuid,
    collection_id: Uuid,
    group_id: Uuid,
    access_level: AccessLevel,
) -> Result<(), tokio_postgres::Error> {
    txn.execute(
        "INSERT INTO collection_group_access (id, org_id, collection_id, group_id, access_level)
         VALUES ($1, $2, $3, $4, $5)",
        &[
            &Uuid::new_v4(),
            &org,
            &collection_id,
            &group_id,
            &access_level.as_str(),
        ],
    )
    .await?;
    Ok(())
}

pub async fn grant_role_access(
    txn: &Transaction<'_>,
    org: Uuid,
    collection_id: Uuid,
    role_id: Uuid,
    access_level: AccessLevel,
) -> Result<(), tokio_postgres::Error> {
    txn.execute(
        "INSERT INTO collection_role_access (id, org_id, collection_id, role_id, access_level)
         VALUES ($1, $2, $3, $4, $5)",
        &[
            &Uuid::new_v4(),
            &org,
            &collection_id,
            &role_id,
            &access_level.as_str(),
        ],
    )
    .await?;
    Ok(())
}

/// Seeds the full matrix of collection visibility/grant combinations for `fixture.member`.
pub async fn seed_acl_collection_matrix(
    pool: &Pool,
    fixture: &AclOrgFixture,
) -> AclCollectionMatrix {
    let ctx = OrgContext::try_new(fixture.org, fixture.owner, [PERMISSION_QA_QUERY], []).unwrap();
    with_org_txn(pool, &ctx, {
        let fixture = fixture.clone();
        move |txn| {
            Box::pin(async move {
                let mut matrix = AclCollectionMatrix::default();

                matrix.org_visible =
                    insert_collection(txn, fixture.org, fixture.owner, "org", "org").await?;
                matrix.private_owned =
                    insert_collection(txn, fixture.org, fixture.member, "private", "owned").await?;
                matrix.private_foreign =
                    insert_collection(txn, fixture.org, fixture.owner, "private", "foreign")
                        .await?;
                matrix.private_user_grant =
                    insert_collection(txn, fixture.org, fixture.owner, "private", "user-grant")
                        .await?;
                grant_user_access(
                    txn,
                    fixture.org,
                    matrix.private_user_grant,
                    fixture.member,
                    AccessLevel::Write,
                )
                .await?;

                matrix.private_group_leak =
                    insert_collection(txn, fixture.org, fixture.owner, "private", "group-leak")
                        .await?;

                matrix.private_role_leak =
                    insert_collection(txn, fixture.org, fixture.owner, "private", "role-leak")
                        .await?;

                matrix.groups_via_group =
                    insert_collection(txn, fixture.org, fixture.owner, "groups", "via-group")
                        .await?;
                grant_group_access(
                    txn,
                    fixture.org,
                    matrix.groups_via_group,
                    fixture.group_id,
                    AccessLevel::Write,
                )
                .await?;

                matrix.groups_via_role =
                    insert_collection(txn, fixture.org, fixture.owner, "groups", "via-role")
                        .await?;
                grant_role_access(
                    txn,
                    fixture.org,
                    matrix.groups_via_role,
                    fixture.viewer_role_id,
                    AccessLevel::Write,
                )
                .await?;

                matrix.groups_denied =
                    insert_collection(txn, fixture.org, fixture.owner, "groups", "denied").await?;

                matrix.groups_read_grant =
                    insert_collection(txn, fixture.org, fixture.owner, "groups", "read-grant")
                        .await?;
                grant_user_access(
                    txn,
                    fixture.org,
                    matrix.groups_read_grant,
                    fixture.member,
                    AccessLevel::Read,
                )
                .await?;

                matrix.containment =
                    insert_collection(txn, fixture.org, fixture.member, "groups", "contain")
                        .await?;
                grant_group_access(
                    txn,
                    fixture.org,
                    matrix.containment,
                    fixture.group_id,
                    AccessLevel::Write,
                )
                .await?;
                grant_role_access(
                    txn,
                    fixture.org,
                    matrix.containment,
                    fixture.viewer_role_id,
                    AccessLevel::Write,
                )
                .await?;
                grant_user_access(
                    txn,
                    fixture.org,
                    matrix.containment,
                    fixture.member,
                    AccessLevel::Write,
                )
                .await?;
                grant_user_access(
                    txn,
                    fixture.org,
                    matrix.containment,
                    fixture.other_user,
                    AccessLevel::Write,
                )
                .await?;

                Ok(matrix)
            })
        }
    })
    .await
    .expect("seed acl collection matrix")
}

/// Collection ids `member` should see under the `(qa.query, read)` projection.
/// Excludes private collections with no user grant and collections denied by
/// missing group/role grants; dormant grants on private are not seedable (0036).
pub fn expected_member_read_projection(matrix: &AclCollectionMatrix) -> BTreeSet<Uuid> {
    [
        matrix.org_visible,
        matrix.private_owned,
        matrix.private_user_grant,
        matrix.groups_via_group,
        matrix.groups_via_role,
        matrix.groups_read_grant,
        matrix.containment,
    ]
    .into_iter()
    .collect()
}

pub async fn resolver_allowed_collection_ids(pool: &Pool, org: Uuid, user: Uuid) -> BTreeSet<Uuid> {
    let ctx = resolve_org_context_in_txn(pool, org, user)
        .await
        .expect("resolve org context");
    ctx.allowed_collection_ids().clone()
}

pub async fn sql_allowed_collection_ids(
    pool: &Pool,
    org: Uuid,
    user: Uuid,
    permission: &str,
    required_access: AccessLevel,
) -> BTreeSet<Uuid> {
    let permission = permission.to_string();
    let access = required_access.as_str().to_string();
    let provisional = OrgContext::try_new(org, user, [] as [&str; 0], []).unwrap();
    with_org_txn(pool, &provisional, move |txn| {
        Box::pin(async move {
            let sql = format!(
                "SELECT c.id FROM collections c WHERE {}",
                allowed_collections_sql("$1", "$2", "$3", "$4")
            );
            let rows = txn
                .query(&sql, &[&org, &user, &permission, &access])
                .await?;
            Ok(rows
                .iter()
                .map(|row| row.get::<_, Uuid>(0))
                .collect::<BTreeSet<_>>())
        })
    })
    .await
    .expect("sql allowed collections")
}

pub async fn sql_collection_access_exists(
    pool: &Pool,
    org: Uuid,
    user: Uuid,
    collection_id: Uuid,
    permission: &str,
    required_access: AccessLevel,
) -> bool {
    let permission = permission.to_string();
    let access = required_access.as_str().to_string();
    let provisional = OrgContext::try_new(org, user, [] as [&str; 0], []).unwrap();
    with_org_txn(pool, &provisional, move |txn| {
        Box::pin(async move {
            let sql = format!(
                "SELECT EXISTS (
                   SELECT 1 FROM collections c
                   WHERE c.org_id = $1 AND c.id = $2 AND c.deleted_at IS NULL
                     AND ({})
                 )",
                acl_predicate_sql("c.org_id", "c.id", "$3", "$4", "$5")
            );
            let allowed: bool = txn
                .query_one(&sql, &[&org, &collection_id, &user, &permission, &access])
                .await?
                .get(0);
            Ok(allowed)
        })
    })
    .await
    .expect("sql collection access exists")
}

pub async fn group_grant_count(pool: &Pool, org: Uuid, collection_id: Uuid) -> i64 {
    let ctx = OrgContext::try_new(org, Uuid::new_v4(), [] as [&str; 0], []).unwrap();
    with_org_txn(pool, &ctx, move |txn| {
        Box::pin(async move {
            let count: i64 = txn
                .query_one(
                    "SELECT count(*)::bigint FROM collection_group_access
                     WHERE org_id = $1 AND collection_id = $2",
                    &[&org, &collection_id],
                )
                .await?
                .get(0);
            Ok(count)
        })
    })
    .await
    .expect("group grant count")
}

pub async fn role_grant_count(pool: &Pool, org: Uuid, collection_id: Uuid) -> i64 {
    let ctx = OrgContext::try_new(org, Uuid::new_v4(), [] as [&str; 0], []).unwrap();
    with_org_txn(pool, &ctx, move |txn| {
        Box::pin(async move {
            let count: i64 = txn
                .query_one(
                    "SELECT count(*)::bigint FROM collection_role_access
                     WHERE org_id = $1 AND collection_id = $2",
                    &[&org, &collection_id],
                )
                .await?
                .get(0);
            Ok(count)
        })
    })
    .await
    .expect("role grant count")
}

pub async fn user_grant_count(pool: &Pool, org: Uuid, collection_id: Uuid, user_id: Uuid) -> i64 {
    let ctx = OrgContext::try_new(org, user_id, [] as [&str; 0], []).unwrap();
    with_org_txn(pool, &ctx, move |txn| {
        Box::pin(async move {
            let count: i64 = txn
                .query_one(
                    "SELECT count(*)::bigint FROM collection_user_access
                     WHERE org_id = $1 AND collection_id = $2 AND user_id = $3",
                    &[&org, &collection_id, &user_id],
                )
                .await?
                .get(0);
            Ok(count)
        })
    })
    .await
    .expect("user grant count")
}

fn pg_error_text(error: &tokio_postgres::Error) -> String {
    if let Some(db) = error.as_db_error() {
        format!("{} {}", db.message(), db.detail().unwrap_or(""))
    } else {
        format!("{error:?}")
    }
}

/// Attempt a group grant in its own transaction (migration 0036 may reject).
pub async fn attempt_grant_group_access(
    pool: &Pool,
    fixture: &AclOrgFixture,
    collection_id: Uuid,
    access_level: AccessLevel,
) -> Result<(), String> {
    let ctx = OrgContext::try_new(fixture.org, fixture.owner, [PERMISSION_QA_QUERY], []).unwrap();
    with_org_txn(pool, &ctx, {
        let fixture = fixture.clone();
        move |txn| {
            Box::pin(async move {
                grant_group_access(
                    txn,
                    fixture.org,
                    collection_id,
                    fixture.group_id,
                    access_level,
                )
                .await
                .map_err(|error| DbError::Config(pg_error_text(&error)))
            })
        }
    })
    .await
    .map_err(|error| error.to_string())
}

/// Attempt a role grant in its own transaction (migration 0036 may reject).
pub async fn attempt_grant_role_access(
    pool: &Pool,
    fixture: &AclOrgFixture,
    collection_id: Uuid,
    access_level: AccessLevel,
) -> Result<(), String> {
    let ctx = OrgContext::try_new(fixture.org, fixture.owner, [PERMISSION_QA_QUERY], []).unwrap();
    with_org_txn(pool, &ctx, {
        let fixture = fixture.clone();
        move |txn| {
            Box::pin(async move {
                grant_role_access(
                    txn,
                    fixture.org,
                    collection_id,
                    fixture.viewer_role_id,
                    access_level,
                )
                .await
                .map_err(|error| DbError::Config(pg_error_text(&error)))
            })
        }
    })
    .await
    .map_err(|error| error.to_string())
}
