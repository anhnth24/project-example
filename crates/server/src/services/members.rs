//! Membership + invite domain services (Wave 1 domain layer, P2-11 / 1C-02).
//!
//! Wave 2 builds HTTP routes (E1-E8 in
//! `plans/reports/plan-260728-0231-markhand-web-membership-admin-slice.md`)
//! on top of these functions. Nothing here talks axum/HTTP; [`MemberError`] is
//! a plain typed enum so the route layer maps it to a status code (in
//! particular `LastOwner` -> 409) without inferring intent from a string.
//!
//! Design notes that matter for Wave 2 / reviewers:
//! - **Last-owner invariant** ([`check_last_owner_invariant`]) is a pure,
//!   DB-free function over already-locked owner rows, so it has fast unit
//!   tests. The DB-touching wrapper ([`guard_last_owner`]) takes
//!   `db::members::lock_owner_rows` (`SELECT ... FOR UPDATE` on every
//!   owner-role row in the org) *inside the same transaction* as the mutation
//!   it guards, so two concurrent operations against the same org's owners
//!   serialize on those row locks. Concurrent-race coverage is Wave 2's
//!   DB-gated test (see module doc bottom).
//! - **Invite tokens** embed the org id in the plaintext
//!   (`mhinv1.<org_id>.<secret>`), exactly mirroring
//!   `auth::session`'s refresh-token shape (`mh1.<org_id>.<secret>`). This is
//!   required, not cosmetic: `org_invites` is `FORCE ROW LEVEL SECURITY`
//!   scoped by the `app.org_id` GUC, and accept-invite runs before the
//!   caller has any membership/permission in the target org, so there is no
//!   other fail-closed way to pick the right GUC before querying by hash.
//! - **Remove vs. suspend vs. downgrade** differ in what they do to
//!   `refresh_tokens`: `remove_member` hard-deletes those rows in the same
//!   transaction (required by the `refresh_tokens(org_id,user_id) REFERENCES
//!   org_memberships` FK, which has no `ON DELETE` action and therefore
//!   RESTRICTs the membership DELETE while any referencing row — even a
//!   revoked one — still exists). `suspend_member`/`change_role` keep the
//!   membership row, so the FK is untouched; Wave 2 MUST separately call
//!   `auth::session::revoke_all_user_families` after those two so an
//!   already-issued short-lived access token can't coast to expiry with
//!   stale permissions. Calling `revoke_all_user_families` before or after
//!   `remove_member` is harmless (it becomes a no-op once the rows are gone).
//!
//! DB-gated tests Wave 2 must still write (not covered here, no DB in Wave 1):
//! - Concurrent last-owner race: two txns racing to remove/downgrade/suspend
//!   two different owners of the same org — exactly one must succeed, the
//!   org must retain >= 1 active owner afterwards.
//! - Invite replay/expiry against real Postgres: accept same token twice,
//!   accept after revoke, accept after `expires_at` — all reject; a valid
//!   accept creates the membership row and the `member.invite_accept` audit
//!   row in the same commit.
//! - Cross-org denial (plan section 2, caveat C1): org A admin can't
//!   list/patch/delete org B's members via these functions (RLS should make
//!   the row simply not exist under org A's GUC — assert `NotFound`/empty,
//!   not a 403-shaped leak); an org A invite token must not accept into org B
//!   even if somehow presented with org B's GUC.
//! - `remove_member` really does clear `refresh_tokens` and a subsequent
//!   refresh with the old token is rejected.

use chrono::{DateTime, Duration, Utc};
use deadpool_postgres::Pool;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio_postgres::Transaction;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::config::SecretString;
use crate::db::error::DbError;
use crate::db::members::{self, NewInvite, OwnerRow};
use crate::db::models::{
    AuditOutcome, MembershipRole, MembershipState, OrgInvite, OrgMembership, ResourceKind,
};
use crate::db::pool::with_org_txn_typed;
use crate::db::quota;
use crate::services::audit::{self, AuditAction, AuditRecord};

const INVITE_TOKEN_PREFIX: &str = "mhinv1";
const INVITE_TOKEN_SECRET_BYTES: usize = 32;

/// Default invite lifetime (7 days); callers may pass a different TTL.
pub const DEFAULT_INVITE_TTL_SECS: i64 = 7 * 24 * 3600;

/// Typed domain errors. Wave 2 maps these to HTTP status codes; in particular
/// `LastOwner` -> 409 (plan section 4, E6/E7).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MemberError {
    #[error("membership not found")]
    NotFound,
    #[error("invite not found")]
    InviteNotFound,
    #[error("invite token is malformed")]
    InvalidToken,
    #[error("invite already accepted or revoked")]
    InviteTerminal,
    #[error("invite has expired")]
    InviteExpired,
    #[error("user is already a member of this org")]
    AlreadyMember,
    #[error("operation would leave the org with zero active owners")]
    LastOwner,
    #[error("only an active owner may invite a new owner")]
    OwnerRequiredForOwnerInvite,
    #[error("only an active owner may grant or manage an owner")]
    OwnerRequiredToManageOwner,
    #[error("database error")]
    Database,
}

/// Pure decision: does this membership operation touch the owner tier and so
/// require the *caller* to be an active owner? True when the operation grants
/// owner, or when its target is currently an owner (demote/remove/suspend/
/// reactivate of an owner). Phase 1C-02 acceptance: "admin không quản owner".
/// Kept pure so the rule is unit- and mutation-testable without a database.
pub fn operation_manages_owner(target_current_role: MembershipRole, grants_owner: bool) -> bool {
    grants_owner || target_current_role == MembershipRole::Owner
}

impl From<DbError> for MemberError {
    fn from(err: DbError) -> Self {
        // `NotFound` must not collapse into `Database` (→ 500). Every bare `?`
        // on a `members::get`/`mark_invite_accepted` here can legitimately hit
        // a row that a concurrent delete removed between a route's pre-check
        // and this transaction; the caller maps `NotFound` → 404, which is the
        // honest answer, instead of an opaque 500 indistinguishable from a real
        // outage. The invite-lookup path maps `NotFound` → `InviteNotFound`
        // explicitly *before* this blanket conversion is reached, so member
        // gets are the only thing this arm relabels.
        match err {
            DbError::NotFound => Self::NotFound,
            _ => Self::Database,
        }
    }
}

// ---------------------------------------------------------------------
// Last-owner invariant (pure, fast-unit-testable — no DB in this function)
// ---------------------------------------------------------------------

/// Pure last-owner invariant check.
///
/// `owners` is every owner-role membership row in the org, exactly as read
/// (and, in production, row-locked `FOR UPDATE`) by
/// [`db::members::lock_owner_rows`](crate::db::members::lock_owner_rows).
/// `target_user_id` is the membership being changed; `target_will_be_active_owner`
/// is what the target's row will look like *after* the operation completes:
/// `false` for remove/suspend/downgrade-away-from-owner, `true` only when the
/// target keeps or gains an active owner role. Only `MembershipState::Active`
/// owners count — a suspended owner does not keep the org "owned".
pub fn check_last_owner_invariant(
    owners: &[OwnerRow],
    target_user_id: Uuid,
    target_will_be_active_owner: bool,
) -> Result<(), MemberError> {
    let others_active = owners
        .iter()
        .filter(|owner| owner.user_id != target_user_id && owner.state == MembershipState::Active)
        .count();
    let remaining = others_active + usize::from(target_will_be_active_owner);
    if remaining == 0 {
        return Err(MemberError::LastOwner);
    }
    Ok(())
}

async fn guard_last_owner(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    target_user_id: Uuid,
    target_will_be_active_owner: bool,
) -> Result<(), MemberError> {
    let owners = members::lock_owner_rows(txn, ctx).await?;
    check_last_owner_invariant(&owners, target_user_id, target_will_be_active_owner)
}

/// Enforces "only an active owner may grant or manage an owner" in the same
/// transaction as the mutation. Without this, `guard_last_owner` alone lets any
/// `member.manage` holder `PATCH {self} {role: owner}` — it only stops the
/// *count* reaching zero, never restricts *who* may reach the owner tier — so a
/// non-owner admin could self-promote and take over the tenant. The caller row
/// is read inside the mutation txn so a concurrent demote of the caller is
/// observed.
async fn guard_owner_tier(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    target_current_role: MembershipRole,
    grants_owner: bool,
) -> Result<(), MemberError> {
    if !operation_manages_owner(target_current_role, grants_owner) {
        return Ok(());
    }
    let caller = members::get(txn, ctx, ctx.user_id()).await?;
    if caller.role != MembershipRole::Owner || caller.state != MembershipState::Active {
        return Err(MemberError::OwnerRequiredToManageOwner);
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Invite token hashing / lifecycle (pure helpers, fast-unit-testable)
// ---------------------------------------------------------------------

/// Hashes an opaque invite token for storage (SHA-256 hex) — same scheme as
/// `auth::session::hash_refresh_token`, applied independently here so this
/// module has no compile-time dependency on the session module.
fn hash_invite_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// App-side hash verification, exercised directly by unit tests. The
/// production accept-invite path uses an equality lookup in SQL
/// (`db::members::find_invite_by_token_hash`) instead of calling this, but
/// the check is the same computation either way.
pub fn verify_invite_token(plaintext: &str, stored_hash: &str) -> bool {
    hash_invite_token(plaintext) == stored_hash
}

fn mint_invite_token(org_id: Uuid) -> SecretString {
    let mut bytes = [0u8; INVITE_TOKEN_SECRET_BYTES];
    rand::fill(&mut bytes[..]);
    let secret = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes);
    SecretString::new(format!("{INVITE_TOKEN_PREFIX}.{org_id}.{secret}"))
}

/// Parses `mhinv1.<org_id>.<secret>` without logging the secret.
fn parse_invite_token(token: &str) -> Result<(Uuid, &str), MemberError> {
    let mut parts = token.splitn(3, '.');
    let prefix = parts.next().ok_or(MemberError::InvalidToken)?;
    let org_raw = parts.next().ok_or(MemberError::InvalidToken)?;
    let secret = parts.next().ok_or(MemberError::InvalidToken)?;
    if prefix != INVITE_TOKEN_PREFIX || secret.is_empty() || secret.len() < 16 {
        return Err(MemberError::InvalidToken);
    }
    let org_id = Uuid::parse_str(org_raw).map_err(|_| MemberError::InvalidToken)?;
    Ok((org_id, secret))
}

/// Pure invite-acceptability check: not expired, not already terminal
/// (accepted or revoked). Mirrors `ck_org_invites__terminal_xor`.
pub fn check_invite_acceptable(invite: &OrgInvite, now: DateTime<Utc>) -> Result<(), MemberError> {
    if invite.accepted_at.is_some() || invite.revoked_at.is_some() {
        return Err(MemberError::InviteTerminal);
    }
    if invite.expires_at <= now {
        return Err(MemberError::InviteExpired);
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Membership reads
// ---------------------------------------------------------------------

/// Lists every membership in the tenant (E1).
pub async fn list_members(
    pool: &Pool,
    ctx: &OrgContext,
) -> Result<Vec<OrgMembership>, MemberError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| Box::pin(async move { Ok(members::list(txn, &ctx).await?) })
    })
    .await
}

/// Lists every invite in the tenant, open and terminal (E2). Callers must
/// never surface `token_hash` to a client.
pub async fn list_invites(pool: &Pool, ctx: &OrgContext) -> Result<Vec<OrgInvite>, MemberError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| Box::pin(async move { Ok(members::list_invites(txn, &ctx).await?) })
    })
    .await
}

// ---------------------------------------------------------------------
// Invite create / accept / revoke (E3-E5)
// ---------------------------------------------------------------------

/// Result of creating an invite: the plaintext token is surfaced exactly
/// once here — callers (Wave 2's route) must return it in the response body
/// and MUST NOT log it or pass it to `services::audit` (the metadata
/// allowlist for `MemberInvite` does not include a token field, so passing
/// it would fail closed at `sanitize_for_action`, but don't rely on that as
/// the only guard).
pub struct CreatedInvite {
    pub invite: OrgInvite,
    pub plaintext_token: SecretString,
}

/// Creates a single-use invite (E3). Only an active owner may invite a new
/// owner (plan section 4, E3 note); any active member with the caller's
/// permission (`member.manage`, enforced by Wave 2's route guard) may invite
/// non-owner roles.
pub async fn create_invite(
    pool: &Pool,
    ctx: &OrgContext,
    request_id: &str,
    email: &str,
    role: MembershipRole,
    ttl_secs: i64,
) -> Result<CreatedInvite, MemberError> {
    let email = email.trim().to_ascii_lowercase();
    let expires_at = Utc::now() + Duration::seconds(ttl_secs.max(60));
    let invite_id = Uuid::new_v4();
    let plaintext = mint_invite_token(ctx.org_id());
    let token_hash = hash_invite_token(plaintext.expose());

    let invite = with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        let request_id = request_id.to_string();
        move |txn| {
            Box::pin(async move {
                if role == MembershipRole::Owner {
                    let caller = members::get(txn, &ctx, ctx.user_id()).await?;
                    if caller.role != MembershipRole::Owner
                        || caller.state != MembershipState::Active
                    {
                        return Err(MemberError::OwnerRequiredForOwnerInvite);
                    }
                }
                let invite = members::insert_invite(
                    txn,
                    &ctx,
                    NewInvite {
                        id: invite_id,
                        email: &email,
                        role,
                        token_hash: &token_hash,
                        invited_by_user_id: ctx.user_id(),
                        expires_at,
                    },
                )
                .await?;
                audit::record_in_txn(
                    txn,
                    &ctx,
                    AuditRecord {
                        request_id: &request_id,
                        action: AuditAction::MemberInvite.as_str(),
                        resource_type: "member",
                        resource_id: Some(&invite.id.to_string()),
                        outcome: AuditOutcome::Success,
                        metadata: serde_json::json!({
                            "invite_id": invite.id.to_string(),
                            "role": role.as_str(),
                        }),
                    },
                )
                .await?;
                Ok(invite)
            })
        }
    })
    .await?;

    Ok(CreatedInvite {
        invite,
        plaintext_token: plaintext,
    })
}

/// Accepts an invite by presenting its plaintext token (E5). Auth-only: the
/// caller only needs a valid session, not `member.manage` in the target org
/// (they have no membership there yet). Creates the membership transactionally
/// with marking the invite accepted; rejects replay/expiry/revoked tokens.
pub async fn accept_invite(
    pool: &Pool,
    plaintext_token: &str,
    accepting_user_id: Uuid,
    request_id: &str,
) -> Result<OrgMembership, MemberError> {
    let (org_id, _secret) = parse_invite_token(plaintext_token)?;
    let token_hash = hash_invite_token(plaintext_token);
    let provisional = OrgContext::try_new(org_id, accepting_user_id, [] as [&str; 0], [])
        .map_err(|_| MemberError::InvalidToken)?;

    with_org_txn_typed(pool, &provisional, {
        let ctx = provisional.clone();
        let request_id = request_id.to_string();
        move |txn| {
            Box::pin(async move {
                let invite = members::find_invite_by_token_hash(txn, &ctx, &token_hash)
                    .await?
                    .ok_or(MemberError::InviteNotFound)?;
                check_invite_acceptable(&invite, Utc::now())?;

                let inserted =
                    members::try_insert(txn, &ctx, accepting_user_id, invite.role).await?;
                let Some(membership) = inserted else {
                    return Err(MemberError::AlreadyMember);
                };
                members::mark_invite_accepted(txn, &ctx, invite.id, Utc::now()).await?;

                audit::record_in_txn(
                    txn,
                    &ctx,
                    AuditRecord {
                        request_id: &request_id,
                        action: AuditAction::MemberInviteAccept.as_str(),
                        resource_type: "member",
                        resource_id: Some(&invite.id.to_string()),
                        outcome: AuditOutcome::Success,
                        metadata: serde_json::json!({
                            "invite_id": invite.id.to_string(),
                            "role": invite.role.as_str(),
                        }),
                    },
                )
                .await?;
                Ok(membership)
            })
        }
    })
    .await
}

/// Revokes an invite that has not yet been accepted/revoked (E4).
pub async fn revoke_invite(
    pool: &Pool,
    ctx: &OrgContext,
    request_id: &str,
    invite_id: Uuid,
) -> Result<OrgInvite, MemberError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        let request_id = request_id.to_string();
        move |txn| {
            Box::pin(async move {
                let existing = members::get_invite(txn, &ctx, invite_id).await.map_err(
                    |error| match error {
                        DbError::NotFound => MemberError::InviteNotFound,
                        other => MemberError::from(other),
                    },
                )?;
                if existing.accepted_at.is_some() || existing.revoked_at.is_some() {
                    return Err(MemberError::InviteTerminal);
                }
                let invite = members::mark_invite_revoked(txn, &ctx, invite_id, Utc::now()).await?;
                audit::record_in_txn(
                    txn,
                    &ctx,
                    AuditRecord {
                        request_id: &request_id,
                        action: AuditAction::MemberInviteRevoke.as_str(),
                        resource_type: "member",
                        resource_id: Some(&invite.id.to_string()),
                        outcome: AuditOutcome::Success,
                        metadata: serde_json::json!({ "invite_id": invite.id.to_string() }),
                    },
                )
                .await?;
                Ok(invite)
            })
        }
    })
    .await
}

// ---------------------------------------------------------------------
// Role change / remove / suspend (E6-E7) — last-owner invariant applies
// ---------------------------------------------------------------------

/// Changes a member's role (E6). Runs the last-owner invariant in the same
/// transaction: if `target_user_id` currently holds an active owner role and
/// `new_role` is not `Owner`, this fails closed with `LastOwner` when they
/// are the org's only remaining active owner.
///
/// Wave 2 must call `auth::session::revoke_all_user_families` after a
/// successful downgrade so an already-issued access token cannot coast on
/// stale permissions until it naturally expires.
pub async fn change_role(
    pool: &Pool,
    ctx: &OrgContext,
    request_id: &str,
    target_user_id: Uuid,
    new_role: MembershipRole,
) -> Result<OrgMembership, MemberError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        let request_id = request_id.to_string();
        move |txn| {
            Box::pin(async move {
                let current = members::get(txn, &ctx, target_user_id).await?;
                // Owner-tier gate BEFORE the no-op short-circuit, so an admin
                // cannot probe "is this user an owner?" by PATCHing owner→owner
                // and reading 200 vs 403 — the gate answers 403 either way.
                guard_owner_tier(txn, &ctx, current.role, new_role == MembershipRole::Owner)
                    .await?;
                // A role no-op changes nothing: skip the mutation and the
                // audit row so the trail never records a misleading "owner →
                // owner" change (adversarial review finding #5).
                if new_role == current.role {
                    return Ok(current);
                }
                let will_be_active_owner =
                    new_role == MembershipRole::Owner && current.state == MembershipState::Active;
                guard_last_owner(txn, &ctx, target_user_id, will_be_active_owner).await?;

                let updated = members::update_role(txn, &ctx, target_user_id, new_role).await?;
                audit::record_in_txn(
                    txn,
                    &ctx,
                    AuditRecord {
                        request_id: &request_id,
                        action: AuditAction::MemberRoleChange.as_str(),
                        resource_type: "member",
                        resource_id: Some(&target_user_id.to_string()),
                        outcome: AuditOutcome::Success,
                        metadata: serde_json::json!({
                            "target_user_id": target_user_id.to_string(),
                            "old_role": current.role.as_str(),
                            "new_role": new_role.as_str(),
                        }),
                    },
                )
                .await?;
                Ok(updated)
            })
        }
    })
    .await
}

/// Removes a member from the org entirely (E7). Runs the last-owner
/// invariant, then hard-deletes the user's `refresh_tokens` rows in the org
/// before deleting the membership row (required by the FK — see
/// `db::members::delete` doc comment).
///
/// This already fully invalidates the user's sessions in this org, so
/// whether Wave 2 also calls `auth::session::revoke_all_user_families` before
/// or after this is a no-op either way; it is not required for correctness
/// here (unlike for `change_role` downgrade / `suspend_member`, where the
/// membership row survives and an existing access token could otherwise
/// coast until expiry).
pub async fn remove_member(
    pool: &Pool,
    ctx: &OrgContext,
    request_id: &str,
    target_user_id: Uuid,
) -> Result<(), MemberError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        let request_id = request_id.to_string();
        move |txn| {
            Box::pin(async move {
                let current = members::get(txn, &ctx, target_user_id).await?;
                // Removing an owner is managing an owner → owner-only.
                guard_owner_tier(txn, &ctx, current.role, false).await?;
                guard_last_owner(txn, &ctx, target_user_id, false).await?;

                members::purge_refresh_tokens(txn, &ctx, target_user_id).await?;
                members::delete(txn, &ctx, target_user_id).await?;

                audit::record_in_txn(
                    txn,
                    &ctx,
                    AuditRecord {
                        request_id: &request_id,
                        action: AuditAction::MemberRemove.as_str(),
                        resource_type: "member",
                        resource_id: Some(&target_user_id.to_string()),
                        outcome: AuditOutcome::Success,
                        metadata: serde_json::json!({
                            "target_user_id": target_user_id.to_string(),
                            "old_role": current.role.as_str(),
                        }),
                    },
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
}

/// Suspends a member: `state` becomes `suspended`, row/role/history kept.
/// The org-context resolver (`auth::permissions::resolve_org_context*`)
/// treats a suspended membership exactly like a missing one on the next
/// request. Runs the last-owner invariant (a suspended owner never counts as
/// active). Wave 2 must call `revoke_all_user_families` afterwards — same
/// reasoning as a role downgrade.
pub async fn suspend_member(
    pool: &Pool,
    ctx: &OrgContext,
    request_id: &str,
    target_user_id: Uuid,
) -> Result<OrgMembership, MemberError> {
    set_member_state(
        pool,
        ctx,
        request_id,
        target_user_id,
        MembershipState::Suspended,
    )
    .await
}

/// Reactivates a previously suspended member. Never reduces the active-owner
/// count, so the invariant is not (and need not be) checked here.
pub async fn reactivate_member(
    pool: &Pool,
    ctx: &OrgContext,
    request_id: &str,
    target_user_id: Uuid,
) -> Result<OrgMembership, MemberError> {
    set_member_state(
        pool,
        ctx,
        request_id,
        target_user_id,
        MembershipState::Active,
    )
    .await
}

async fn set_member_state(
    pool: &Pool,
    ctx: &OrgContext,
    request_id: &str,
    target_user_id: Uuid,
    new_state: MembershipState,
) -> Result<OrgMembership, MemberError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        let request_id = request_id.to_string();
        move |txn| {
            Box::pin(async move {
                let current = members::get(txn, &ctx, target_user_id).await?;
                // Suspending or reactivating an owner is managing an owner →
                // owner-only, checked before the no-op short-circuit for the
                // same anti-probe reason as `change_role`.
                guard_owner_tier(txn, &ctx, current.role, false).await?;
                if new_state == current.state {
                    return Ok(current);
                }
                if new_state == MembershipState::Suspended {
                    guard_last_owner(txn, &ctx, target_user_id, false).await?;
                }
                let updated = members::set_state(txn, &ctx, target_user_id, new_state).await?;
                audit::record_in_txn(
                    txn,
                    &ctx,
                    AuditRecord {
                        request_id: &request_id,
                        action: AuditAction::MemberRoleChange.as_str(),
                        resource_type: "member",
                        resource_id: Some(&target_user_id.to_string()),
                        outcome: AuditOutcome::Success,
                        metadata: serde_json::json!({
                            "target_user_id": target_user_id.to_string(),
                            "old_state": current.state.as_str(),
                            "new_state": new_state.as_str(),
                        }),
                    },
                )
                .await?;
                Ok(updated)
            })
        }
    })
    .await
}

// ---------------------------------------------------------------------
// Usage aggregation (E8) — assembled from existing db::quota read fns
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceUsage {
    pub kind: ResourceKind,
    pub limit: i64,
    pub committed: i64,
    pub reserved: i64,
    pub remaining: i64,
}

const ALL_RESOURCE_KINDS: [ResourceKind; 4] = [
    ResourceKind::StorageBytes,
    ResourceKind::Documents,
    ResourceKind::ConcurrentJobs,
    ResourceKind::Tokens,
];

/// Assembles per-`ResourceKind` {limit, committed, reserved, remaining} from
/// the existing `db::quota` read functions (E8).
pub async fn usage_overview(
    pool: &Pool,
    ctx: &OrgContext,
) -> Result<Vec<ResourceUsage>, MemberError> {
    let observed_at = Utc::now();
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let mut out = Vec::with_capacity(ALL_RESOURCE_KINDS.len());
                for kind in ALL_RESOURCE_KINDS {
                    let usage = quota::usage(txn, &ctx, kind, observed_at).await?;
                    let remaining = (usage.limit - usage.committed - usage.active_reserved).max(0);
                    out.push(ResourceUsage {
                        kind,
                        limit: usage.limit,
                        committed: usage.committed,
                        reserved: usage.active_reserved,
                        remaining,
                    });
                }
                Ok(out)
            })
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn owner(user_id: Uuid, state: MembershipState) -> OwnerRow {
        OwnerRow { user_id, state }
    }

    // --- Last-owner invariant: fast, DB-free mutation-test target ---

    #[test]
    fn two_active_owners_allow_remove_downgrade_suspend_of_one() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let owners = [
            owner(a, MembershipState::Active),
            owner(b, MembershipState::Active),
        ];

        // remove(a): a's row disappears -> target_will_be_active_owner = false.
        assert_eq!(check_last_owner_invariant(&owners, a, false), Ok(()));
        // downgrade(a) away from owner: same shape, target no longer an owner.
        assert_eq!(check_last_owner_invariant(&owners, a, false), Ok(()));
        // suspend(a): a stops counting as active regardless of role.
        assert_eq!(check_last_owner_invariant(&owners, a, false), Ok(()));
    }

    #[test]
    fn single_active_owner_blocks_remove_downgrade_suspend() {
        let sole = Uuid::new_v4();
        let owners = [owner(sole, MembershipState::Active)];

        // remove(sole)
        assert_eq!(
            check_last_owner_invariant(&owners, sole, false),
            Err(MemberError::LastOwner)
        );
        // downgrade(sole) to a non-owner role
        assert_eq!(
            check_last_owner_invariant(&owners, sole, false),
            Err(MemberError::LastOwner)
        );
        // suspend(sole)
        assert_eq!(
            check_last_owner_invariant(&owners, sole, false),
            Err(MemberError::LastOwner)
        );
    }

    // --- Owner-tier gate (adversarial finding #1: no self-promotion) ---

    #[test]
    fn operation_manages_owner_flags_owner_target_or_owner_grant() {
        use MembershipRole::*;
        // Granting owner (to anyone, whatever their current role) is owner-tier.
        assert!(operation_manages_owner(Viewer, true));
        assert!(operation_manages_owner(Admin, true));
        // Touching a current owner (demote/remove/suspend) is owner-tier even
        // when not granting owner.
        assert!(operation_manages_owner(Owner, false));
        // Managing a non-owner without granting owner is NOT owner-tier — an
        // admin may do it.
        assert!(!operation_manages_owner(Admin, false));
        assert!(!operation_manages_owner(Editor, false));
        assert!(!operation_manages_owner(Viewer, false));
    }

    #[test]
    fn a_suspended_owner_does_not_count_as_the_remaining_owner() {
        let active = Uuid::new_v4();
        let suspended = Uuid::new_v4();
        let owners = [
            owner(active, MembershipState::Active),
            owner(suspended, MembershipState::Suspended),
        ];
        // Removing the only *active* owner must fail even though a second
        // (suspended) owner row still exists.
        assert_eq!(
            check_last_owner_invariant(&owners, active, false),
            Err(MemberError::LastOwner)
        );
    }

    #[test]
    fn promoting_a_new_owner_never_trips_the_guard() {
        let sole = Uuid::new_v4();
        let promoted = Uuid::new_v4();
        let owners = [owner(sole, MembershipState::Active)];
        // Promoted user isn't in the owners list yet; will become one.
        assert_eq!(check_last_owner_invariant(&owners, promoted, true), Ok(()));
    }

    // --- Invite hashing / lifecycle: fast, DB-free ---

    #[test]
    fn invite_token_hash_verifies_and_rejects_wrong_token() {
        let hash = hash_invite_token("mhinv1.11111111-1111-1111-1111-111111111111.super-secret");
        assert!(verify_invite_token(
            "mhinv1.11111111-1111-1111-1111-111111111111.super-secret",
            &hash
        ));
        assert!(!verify_invite_token("wrong-token", &hash));
    }

    #[test]
    fn invite_token_mint_and_parse_roundtrips_org_id() {
        let org_id = Uuid::new_v4();
        let token = mint_invite_token(org_id);
        let (parsed_org, secret) = parse_invite_token(token.expose()).unwrap();
        assert_eq!(parsed_org, org_id);
        assert!(secret.len() >= 16);
    }

    #[test]
    fn invite_token_parse_rejects_malformed_input() {
        assert_eq!(
            parse_invite_token("garbage"),
            Err(MemberError::InvalidToken)
        );
        assert_eq!(
            parse_invite_token("mhinv1.not-a-uuid.secretsecretsecret"),
            Err(MemberError::InvalidToken)
        );
        assert_eq!(
            parse_invite_token(&format!("mhinv1.{}.short", Uuid::new_v4())),
            Err(MemberError::InvalidToken)
        );
    }

    fn sample_invite(
        accepted_at: Option<DateTime<Utc>>,
        revoked_at: Option<DateTime<Utc>>,
        expires_at: DateTime<Utc>,
    ) -> OrgInvite {
        OrgInvite {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            email: "invitee@example.com".into(),
            role: MembershipRole::Editor,
            token_hash: crate::db::models::SecretHash::new("deadbeef".repeat(8)),
            invited_by_user_id: Uuid::new_v4(),
            expires_at,
            accepted_at,
            revoked_at,
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        }
    }

    #[test]
    fn invite_accept_check_rejects_expired() {
        let past = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let invite = sample_invite(None, None, past);
        assert_eq!(
            check_invite_acceptable(&invite, Utc::now()),
            Err(MemberError::InviteExpired)
        );
    }

    #[test]
    fn invite_accept_check_rejects_already_accepted() {
        let future = Utc.with_ymd_and_hms(2100, 1, 1, 0, 0, 0).unwrap();
        let invite = sample_invite(Some(Utc::now()), None, future);
        assert_eq!(
            check_invite_acceptable(&invite, Utc::now()),
            Err(MemberError::InviteTerminal)
        );
    }

    #[test]
    fn invite_accept_check_rejects_revoked() {
        let future = Utc.with_ymd_and_hms(2100, 1, 1, 0, 0, 0).unwrap();
        let invite = sample_invite(None, Some(Utc::now()), future);
        assert_eq!(
            check_invite_acceptable(&invite, Utc::now()),
            Err(MemberError::InviteTerminal)
        );
    }

    #[test]
    fn invite_accept_check_accepts_valid() {
        let future = Utc.with_ymd_and_hms(2100, 1, 1, 0, 0, 0).unwrap();
        let invite = sample_invite(None, None, future);
        assert_eq!(check_invite_acceptable(&invite, Utc::now()), Ok(()));
    }
}
