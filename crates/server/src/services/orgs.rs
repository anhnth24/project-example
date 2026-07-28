//! Organization lifecycle: create / list / detail (1C-01, full slice).
//!
//! `switch` itself lives in [`crate::auth::session::switch_org`] next to the
//! other token-minting flows (login/refresh) since it mints a fresh session;
//! this module covers create (unblocked by 1C-03's global RBAC catalog,
//! migrations/0030) and the read paths list/detail need.
//!
//! Every check here re-verifies current PostgreSQL state — the caller's
//! bearer access token only proves *who* they are (`sub`), never *which org*
//! they may act in or authority over a not-yet-existing org. `org_id` values
//! in requests (path or body) are always untrusted client input, same trust
//! level as a JWT `org_id` claim.

use deadpool_postgres::Pool;
use thiserror::Error;
use tokio_postgres::error::SqlState;
use uuid::Uuid;

use crate::auth::context::{OrgContext, OrgContextError};
use crate::auth::permissions::ResolveError;
use crate::db::error::DbError;
use crate::db::models::{MembershipRole, Org};
use crate::db::orgs;
use crate::db::pool::{with_org_txn, with_org_txn_typed};
use crate::services::audit::{self, AuditAction, AuditRecord};

/// Org + the caller's (always `owner`) role in it, same shape list/detail
/// return via [`OrgSummary`]/[`OrgDetail`].
pub struct CreatedOrg {
    pub org: Org,
    pub role: MembershipRole,
}

/// One organization the caller currently has an active membership in.
pub struct OrgSummary {
    pub org: Org,
    pub role: MembershipRole,
}

pub struct OrgDetail {
    pub org: Org,
    pub role: MembershipRole,
}

#[derive(Debug, Error)]
pub enum CreateOrgError {
    #[error("slug already taken")]
    SlugTaken,
    #[error("database error")]
    Database,
}

impl From<DbError> for CreateOrgError {
    fn from(error: DbError) -> Self {
        match error {
            DbError::Query(pg_error) if pg_error.code() == Some(&SqlState::UNIQUE_VIOLATION) => {
                Self::SlugTaken
            }
            _ => Self::Database,
        }
    }
}

/// Creates a new org + an `owner` membership for `user_id`, in one
/// transaction. Provisions the new org's full RBAC (`roles`/
/// `role_permissions`) from the canonical global catalog
/// (`provision_org_role_catalog`, migrations/0030) — no per-org seed
/// migration required, unlike the POC org (migrations/0011).
///
/// `slug`/`name` must already be validated/trimmed by the caller (route
/// layer); this only re-asserts non-empty as a defense-in-depth floor. A
/// slug collision with an existing org surfaces as [`CreateOrgError::SlugTaken`]
/// (mapped from the `orgs_slug_key` unique-violation the `INSERT` below can
/// hit — `db::orgs::ensure_exists`'s `ON CONFLICT (id)` only suppresses a
/// conflict on the fresh `id`, never on `slug`).
pub async fn create_org(
    pool: &Pool,
    user_id: Uuid,
    slug: &str,
    name: &str,
    request_id: &str,
) -> Result<CreatedOrg, CreateOrgError> {
    if slug.trim().is_empty() || name.trim().is_empty() {
        return Err(CreateOrgError::Database);
    }
    let org_id = Uuid::new_v4();
    let ctx = OrgContext::try_new(org_id, user_id, [] as [&str; 0], [])
        .map_err(|_: OrgContextError| CreateOrgError::Database)?;
    let request_id = request_id.to_string();
    let slug = slug.to_string();
    let name = name.to_string();

    let org = with_org_txn_typed(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let org = orgs::ensure_exists(txn, &ctx, &slug, &name).await?;

                // Global canonical RBAC catalog -> this org's roles/grants
                // (migrations/0030). No manual per-org seed needed.
                txn.execute("SELECT provision_org_role_catalog($1)", &[&ctx.org_id()])
                    .await
                    .map_err(DbError::from)?;

                // Caller becomes the first (and, right now, only) owner.
                orgs::ensure_membership(txn, &ctx).await?;

                let resource_id = org.id.to_string();
                audit::record_in_txn(
                    txn,
                    &ctx,
                    AuditRecord {
                        request_id: &request_id,
                        action: AuditAction::OrgCreate.as_str(),
                        resource_type: "org",
                        resource_id: Some(&resource_id),
                        outcome: crate::db::models::AuditOutcome::Success,
                        metadata: serde_json::json!({
                            "slug_chars": org.slug.len() as i64,
                            "name_chars": org.name.len() as i64,
                        }),
                    },
                )
                .await?;

                Ok::<Org, CreateOrgError>(org)
            })
        }
    })
    .await?;

    Ok(CreatedOrg {
        org,
        role: MembershipRole::Owner,
    })
}

/// Lists organizations where `user_id` currently holds an ACTIVE membership.
///
/// `orgs` is a global table (no RLS); `org_memberships` is FORCE RLS'd on
/// `app.org_id` (migrations/0002), so membership can only be checked one
/// candidate org at a time inside that org's own transaction-scoped GUC —
/// the same shape `auth::session::find_user_org` already uses for login,
/// generalized here to collect every match instead of stopping at the
/// first. Fail-closed per candidate: a disabled user, or a missing/
/// suspended/forged membership, simply excludes that org from the result —
/// it can never *widen* what comes back. This is also the acceptance
/// contract for "chỉ thấy org của mình": an org the caller does not belong
/// to (or no longer belongs to) is indistinguishable from one that does not
/// exist at all.
pub async fn list_user_orgs(pool: &Pool, user_id: Uuid) -> Result<Vec<OrgSummary>, ResolveError> {
    let client = pool.get().await.map_err(|_| ResolveError::Database)?;
    let rows = client
        .query(
            "SELECT id, slug, name, created_at, updated_at FROM orgs ORDER BY created_at",
            &[],
        )
        .await
        .map_err(|_| ResolveError::Database)?;
    drop(client);

    let mut summaries = Vec::with_capacity(rows.len());
    for row in &rows {
        let org_id: Uuid = row.get(0);
        if let Some(role) = active_role(pool, org_id, user_id).await? {
            summaries.push(OrgSummary {
                org: Org {
                    id: org_id,
                    slug: row.get(1),
                    name: row.get(2),
                    created_at: row.get(3),
                    updated_at: row.get(4),
                },
                role,
            });
        }
    }
    Ok(summaries)
}

/// Fetches org detail — `Ok(None)` covers both "no such org" and "exists but
/// caller is not an active member" uniformly (no existence oracle for
/// non-members; the two cases must render the same HTTP response).
pub async fn get_org_detail(
    pool: &Pool,
    user_id: Uuid,
    org_id: Uuid,
) -> Result<Option<OrgDetail>, ResolveError> {
    let Some(role) = active_role(pool, org_id, user_id).await? else {
        return Ok(None);
    };
    let ctx = OrgContext::try_new(org_id, user_id, [] as [&str; 0], [])
        .map_err(|_| ResolveError::InvalidContext)?;
    let org = with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| Box::pin(async move { orgs::get(txn, &ctx).await })
    })
    .await
    .map_err(|_| ResolveError::Database)?;
    Ok(Some(OrgDetail { org, role }))
}

/// Active-membership probe for one candidate org: disabled user or missing/
/// suspended row both resolve to `None`, mirroring the same
/// `state = 'active'` fail-closed rule `auth::permissions::resolve_org_context`
/// enforces for the request-authorization path (kept as a small, separate
/// query here because list/detail only need the role, not the full
/// permission/collection resolution that function also does).
async fn active_role(
    pool: &Pool,
    org_id: Uuid,
    user_id: Uuid,
) -> Result<Option<MembershipRole>, ResolveError> {
    let ctx = OrgContext::try_new(org_id, user_id, [] as [&str; 0], [])
        .map_err(|_| ResolveError::InvalidContext)?;
    let role_raw: Option<String> = with_org_txn(pool, &ctx, move |txn| {
        Box::pin(async move {
            let user_row = txn
                .query_opt("SELECT disabled_at FROM users WHERE id = $1", &[&user_id])
                .await?;
            let Some(user_row) = user_row else {
                return Ok(None);
            };
            let disabled_at: Option<chrono::DateTime<chrono::Utc>> = user_row.get(0);
            if disabled_at.is_some() {
                return Ok(None);
            }
            let membership = txn
                .query_opt(
                    "SELECT role FROM org_memberships
                     WHERE org_id = $1 AND user_id = $2 AND state = 'active'",
                    &[&org_id, &user_id],
                )
                .await?;
            Ok(membership.map(|row| row.get::<_, String>(0)))
        })
    })
    .await
    .map_err(|_| ResolveError::Database)?;

    match role_raw {
        None => Ok(None),
        // Fail closed on an unexpected role value rather than surfacing it.
        Some(raw) => Ok(MembershipRole::parse(&raw).ok()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn org_context_construction_rejects_nil_ids() {
        // Defense-in-depth unit coverage for the fail-closed context guard
        // this module leans on; DB-gated behavior lives in tests/orgs.rs.
        assert!(OrgContext::try_new(Uuid::nil(), Uuid::new_v4(), [] as [&str; 0], []).is_err());
        assert!(OrgContext::try_new(Uuid::new_v4(), Uuid::nil(), [] as [&str; 0], []).is_err());
    }
}
