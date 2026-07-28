//! Tenant-scoped membership + invite repository (ADR 0007, P2-11 / 1C-02).
//!
//! All functions require an [`OrgContext`] and must run inside
//! `pool::with_org_txn` / `with_org_txn_typed` so RLS `app.org_id` is set
//! before any row is touched: `org_memberships` (migrations 0001/0002) and
//! `org_invites` (migrations 0003/0010) are both `FORCE ROW LEVEL SECURITY`.
//!
//! This module is pure data access — the last-owner invariant, invite hash
//! verification, and token lifecycle rules live in `services::members`.

use chrono::{DateTime, Utc};
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::{MembershipRole, MembershipState, OrgInvite, OrgMembership, SecretHash};

/// One owner-role membership row read under `FOR UPDATE` for the last-owner
/// invariant (see [`lock_owner_rows`] and `services::members::check_last_owner_invariant`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerRow {
    pub user_id: Uuid,
    pub state: MembershipState,
}

/// Lists every membership row for the tenant (both states — admins must be
/// able to see suspended members in order to reactivate them).
pub async fn list(txn: &Transaction<'_>, ctx: &OrgContext) -> Result<Vec<OrgMembership>, DbError> {
    let rows = txn
        .query(
            "SELECT org_id, user_id, role, state, created_at
             FROM org_memberships
             WHERE org_id = $1
             ORDER BY created_at, user_id",
            &[&ctx.org_id()],
        )
        .await?;
    rows.iter().map(map_membership).collect()
}

/// Fetches one membership row regardless of state.
pub async fn get(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    user_id: Uuid,
) -> Result<OrgMembership, DbError> {
    let row = txn
        .query_opt(
            "SELECT org_id, user_id, role, state, created_at
             FROM org_memberships
             WHERE org_id = $1 AND user_id = $2",
            &[&ctx.org_id(), &user_id],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_membership(&row)
}

/// Inserts a new active membership; `None` when `(org_id, user_id)` already
/// exists (caller maps this to a typed "already a member" error).
pub async fn try_insert(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    user_id: Uuid,
    role: MembershipRole,
) -> Result<Option<OrgMembership>, DbError> {
    let role_str = role.as_str();
    let row = txn
        .query_opt(
            "INSERT INTO org_memberships (org_id, user_id, role)
             VALUES ($1, $2, $3)
             ON CONFLICT (org_id, user_id) DO NOTHING
             RETURNING org_id, user_id, role, state, created_at",
            &[&ctx.org_id(), &user_id, &role_str],
        )
        .await?;
    row.as_ref().map(map_membership).transpose()
}

/// Updates only the role of an existing membership.
pub async fn update_role(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    user_id: Uuid,
    role: MembershipRole,
) -> Result<OrgMembership, DbError> {
    let role_str = role.as_str();
    let row = txn
        .query_opt(
            "UPDATE org_memberships
             SET role = $3
             WHERE org_id = $1 AND user_id = $2
             RETURNING org_id, user_id, role, state, created_at",
            &[&ctx.org_id(), &user_id, &role_str],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_membership(&row)
}

/// Sets membership state (suspend/reactivate). The row and its role/history
/// are preserved either way — this is deliberately not a delete.
pub async fn set_state(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    user_id: Uuid,
    state: MembershipState,
) -> Result<OrgMembership, DbError> {
    let state_str = state.as_str();
    let row = txn
        .query_opt(
            "UPDATE org_memberships
             SET state = $3
             WHERE org_id = $1 AND user_id = $2
             RETURNING org_id, user_id, role, state, created_at",
            &[&ctx.org_id(), &user_id, &state_str],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_membership(&row)
}

/// Hard-deletes a membership row.
///
/// `refresh_tokens(org_id, user_id)` carries `FOREIGN KEY ... REFERENCES
/// org_memberships(org_id, user_id)` with no `ON DELETE` action
/// (migrations/0003_expand_auth_sessions_rbac.sql), i.e. `NO ACTION`/RESTRICT:
/// any refresh_tokens row for this user in this org — even an already-revoked
/// one — blocks this DELETE. This function does not clear those rows itself
/// (so a caller that only wants a plain delete, e.g. a test fixture, gets an
/// honest FK error instead of a surprise cascading session wipe);
/// `services::members::remove_member` calls [`purge_refresh_tokens`] first in
/// the same transaction.
pub async fn delete(txn: &Transaction<'_>, ctx: &OrgContext, user_id: Uuid) -> Result<(), DbError> {
    let deleted = txn
        .execute(
            "DELETE FROM org_memberships WHERE org_id = $1 AND user_id = $2",
            &[&ctx.org_id(), &user_id],
        )
        .await?;
    if deleted == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Hard-deletes every refresh-token row for `user_id` in the tenant so
/// [`delete`] can satisfy the FK above. This is full session teardown, not a
/// soft revoke — for paths that keep the membership row (role downgrade,
/// suspend) use `auth::session::revoke_all_user_families` instead, which only
/// sets `revoked_at`.
pub async fn purge_refresh_tokens(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    user_id: Uuid,
) -> Result<u64, DbError> {
    let deleted = txn
        .execute(
            "DELETE FROM refresh_tokens WHERE org_id = $1 AND user_id = $2",
            &[&ctx.org_id(), &user_id],
        )
        .await?;
    Ok(deleted)
}

/// Row-locks every owner-role membership in the tenant (`SELECT ... FOR
/// UPDATE`) so concurrent remove/downgrade/suspend operations serialize on
/// the same rows before counting remaining active owners. Must run inside
/// the same transaction as the mutation it guards — see
/// `services::members::check_last_owner_invariant` for the (DB-free) counting
/// logic and `plans/reports/plan-260728-0231-...` section 4 for the invariant.
pub async fn lock_owner_rows(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
) -> Result<Vec<OwnerRow>, DbError> {
    let rows = txn
        .query(
            "SELECT user_id, state
             FROM org_memberships
             WHERE org_id = $1 AND role = 'owner'
             FOR UPDATE",
            &[&ctx.org_id()],
        )
        .await?;
    rows.iter()
        .map(|row| {
            let state: String = row.get("state");
            Ok(OwnerRow {
                user_id: row.get("user_id"),
                state: MembershipState::parse(&state).map_err(DbError::Config)?,
            })
        })
        .collect()
}

fn map_membership(row: &Row) -> Result<OrgMembership, DbError> {
    let role: String = row.get("role");
    let state: String = row.get("state");
    Ok(OrgMembership {
        org_id: row.get("org_id"),
        user_id: row.get("user_id"),
        role: MembershipRole::parse(&role).map_err(DbError::Config)?,
        state: MembershipState::parse(&state).map_err(DbError::Config)?,
        created_at: row.get("created_at"),
    })
}

// ---------------------------------------------------------------------
// Invites
// ---------------------------------------------------------------------

/// Input for inserting a new single-use invite. Only the hash is stored —
/// the plaintext token is minted/returned exactly once by
/// `services::members::create_invite` and never persisted.
pub struct NewInvite<'a> {
    pub id: Uuid,
    pub email: &'a str,
    pub role: MembershipRole,
    pub token_hash: &'a str,
    pub invited_by_user_id: Uuid,
    pub expires_at: DateTime<Utc>,
}

pub async fn insert_invite(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: NewInvite<'_>,
) -> Result<OrgInvite, DbError> {
    let role_str = input.role.as_str();
    let row = txn
        .query_one(
            "INSERT INTO org_invites (
                id, org_id, email, role, token_hash, invited_by_user_id, expires_at
             ) VALUES ($1, $2, $3, $4, $5, $6, $7)
             RETURNING id, org_id, email, role, token_hash, invited_by_user_id,
                       expires_at, accepted_at, revoked_at, created_at",
            &[
                &input.id,
                &ctx.org_id(),
                &input.email,
                &role_str,
                &input.token_hash,
                &input.invited_by_user_id,
                &input.expires_at,
            ],
        )
        .await?;
    map_invite(&row)
}

/// Lists every invite for the tenant (open and terminal) so the admin UI can
/// show history; routes must never echo `token_hash` back to a client.
pub async fn list_invites(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
) -> Result<Vec<OrgInvite>, DbError> {
    let rows = txn
        .query(
            "SELECT id, org_id, email, role, token_hash, invited_by_user_id,
                    expires_at, accepted_at, revoked_at, created_at
             FROM org_invites
             WHERE org_id = $1
             ORDER BY created_at DESC",
            &[&ctx.org_id()],
        )
        .await?;
    rows.iter().map(map_invite).collect()
}

pub async fn get_invite(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    invite_id: Uuid,
) -> Result<OrgInvite, DbError> {
    let row = txn
        .query_opt(
            "SELECT id, org_id, email, role, token_hash, invited_by_user_id,
                    expires_at, accepted_at, revoked_at, created_at
             FROM org_invites
             WHERE org_id = $1 AND id = $2",
            &[&ctx.org_id(), &invite_id],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_invite(&row)
}

/// Looks up an invite by token hash, scoped to `ctx.org_id()` (set as the
/// `app.org_id` GUC by the caller's `with_org_txn`/`with_org_txn_typed`).
///
/// The plaintext invite token embeds its org id (`mhinv1.<org_id>.<secret>`,
/// mirroring `auth::session`'s refresh-token shape) precisely so accept-invite
/// — which runs before any membership/permission exists for the caller — can
/// set the correct GUC before querying an RLS-protected table without ever
/// concatenating an unvalidated org id into SQL.
pub async fn find_invite_by_token_hash(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    token_hash: &str,
) -> Result<Option<OrgInvite>, DbError> {
    let row = txn
        .query_opt(
            "SELECT id, org_id, email, role, token_hash, invited_by_user_id,
                    expires_at, accepted_at, revoked_at, created_at
             FROM org_invites
             WHERE org_id = $1 AND token_hash = $2",
            &[&ctx.org_id(), &token_hash],
        )
        .await?;
    row.as_ref().map(map_invite).transpose()
}

/// Marks an invite accepted. Only updates rows that are not already terminal
/// (`ck_org_invites__terminal_xor`), so a replayed accept on a terminal invite
/// returns `NotFound` here (caller should check terminal state first via
/// `get_invite`/`find_invite_by_token_hash` to return a precise typed error).
pub async fn mark_invite_accepted(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    invite_id: Uuid,
    accepted_at: DateTime<Utc>,
) -> Result<OrgInvite, DbError> {
    let row = txn
        .query_opt(
            "UPDATE org_invites
             SET accepted_at = $3
             WHERE org_id = $1 AND id = $2
               AND accepted_at IS NULL AND revoked_at IS NULL
             RETURNING id, org_id, email, role, token_hash, invited_by_user_id,
                       expires_at, accepted_at, revoked_at, created_at",
            &[&ctx.org_id(), &invite_id, &accepted_at],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_invite(&row)
}

/// Marks an invite revoked; same not-already-terminal guard as
/// [`mark_invite_accepted`].
pub async fn mark_invite_revoked(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    invite_id: Uuid,
    revoked_at: DateTime<Utc>,
) -> Result<OrgInvite, DbError> {
    let row = txn
        .query_opt(
            "UPDATE org_invites
             SET revoked_at = $3
             WHERE org_id = $1 AND id = $2
               AND accepted_at IS NULL AND revoked_at IS NULL
             RETURNING id, org_id, email, role, token_hash, invited_by_user_id,
                       expires_at, accepted_at, revoked_at, created_at",
            &[&ctx.org_id(), &invite_id, &revoked_at],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_invite(&row)
}

fn map_invite(row: &Row) -> Result<OrgInvite, DbError> {
    let role: String = row.get("role");
    let token_hash: String = row.get("token_hash");
    Ok(OrgInvite {
        id: row.get("id"),
        org_id: row.get("org_id"),
        email: row.get("email"),
        role: MembershipRole::parse(&role).map_err(DbError::Config)?,
        token_hash: SecretHash::new(token_hash),
        invited_by_user_id: row.get("invited_by_user_id"),
        expires_at: row.get("expires_at"),
        accepted_at: row.get("accepted_at"),
        revoked_at: row.get("revoked_at"),
        created_at: row.get("created_at"),
    })
}
