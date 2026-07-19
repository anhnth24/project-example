//! Opaque rotating refresh tokens with family-wide revoke-on-reuse.

use chrono::{Duration, Utc};
use deadpool_postgres::Object;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio_postgres::Transaction;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::jwt::{JwtError, JwtKeys};
use crate::auth::password::{self, PasswordError};
use crate::auth::permissions::{resolve_org_context_in_txn, ResolveError};
use crate::config::{Argon2Config, AuthConfig, SecretString};
use crate::db::error::DbError;
use crate::db::pool::{apply_org_context, with_org_txn};
use crate::telemetry::redacted_json_value;

const REFRESH_PREFIX: &str = "mh1";
const REFRESH_SECRET_BYTES: usize = 32;

/// Issued access + refresh pair (secrets never appear in Debug).
#[derive(Clone, PartialEq, Eq)]
pub struct TokenPair {
    pub access_token: SecretString,
    pub refresh_token: SecretString,
    pub expires_in: u64,
    pub family_id: Uuid,
    pub org_id: Uuid,
    pub user_id: Uuid,
}

impl std::fmt::Debug for TokenPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenPair")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("expires_in", &self.expires_in)
            .field("family_id", &self.family_id)
            .field("org_id", &self.org_id)
            .field("user_id", &self.user_id)
            .finish()
    }
}

/// Auth/session errors (safe for clients; no secrets).
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SessionError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("user is disabled")]
    UserDisabled,
    #[error("org membership missing")]
    MembershipMissing,
    #[error("refresh token is invalid")]
    InvalidRefresh,
    #[error("refresh token reuse detected; family revoked")]
    RefreshReuse,
    #[error("authentication is not configured")]
    NotConfigured,
    #[error("database error")]
    Database,
    #[error("token error")]
    Token,
    #[error("password hashing failed")]
    Password,
}

impl From<DbError> for SessionError {
    fn from(_: DbError) -> Self {
        Self::Database
    }
}

impl From<JwtError> for SessionError {
    fn from(value: JwtError) -> Self {
        match value {
            JwtError::NotConfigured => Self::NotConfigured,
            _ => Self::Token,
        }
    }
}

impl From<PasswordError> for SessionError {
    fn from(_: PasswordError) -> Self {
        Self::Password
    }
}

impl From<ResolveError> for SessionError {
    fn from(value: ResolveError) -> Self {
        match value {
            ResolveError::UserDisabled => Self::UserDisabled,
            ResolveError::MembershipMissing => Self::MembershipMissing,
            ResolveError::PermissionDenied
            | ResolveError::CollectionDenied
            | ResolveError::InvalidContext
            | ResolveError::Database => Self::Database,
        }
    }
}

/// Hashes an opaque refresh token for storage (SHA-256 hex).
pub fn hash_refresh_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    hex::encode(digest)
}

/// Parses `mh1.<org_id>.<secret>` without logging the secret.
pub fn parse_refresh_token(token: &str) -> Result<(Uuid, &str), SessionError> {
    let mut parts = token.splitn(3, '.');
    let prefix = parts.next().ok_or(SessionError::InvalidRefresh)?;
    let org_raw = parts.next().ok_or(SessionError::InvalidRefresh)?;
    let secret = parts.next().ok_or(SessionError::InvalidRefresh)?;
    if prefix != REFRESH_PREFIX || secret.is_empty() || secret.len() < 16 {
        return Err(SessionError::InvalidRefresh);
    }
    let org_id = Uuid::parse_str(org_raw).map_err(|_| SessionError::InvalidRefresh)?;
    Ok((org_id, secret))
}

fn mint_refresh_token(org_id: Uuid) -> SecretString {
    let mut bytes = [0u8; REFRESH_SECRET_BYTES];
    rand::fill(&mut bytes[..]);
    let secret = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes);
    SecretString::new(format!("{REFRESH_PREFIX}.{org_id}.{secret}"))
}

/// Append-only audit fields (metadata must never contain secrets).
pub struct AuditEvent<'a> {
    pub org_id: Uuid,
    pub actor_user_id: Option<Uuid>,
    pub action: &'a str,
    pub resource_type: &'a str,
    pub resource_id: Option<&'a str>,
    pub outcome: &'a str,
    pub request_id: &'a str,
    pub metadata: serde_json::Value,
}

/// Append-only audit row (metadata must never contain secrets).
pub async fn write_audit(txn: &Transaction<'_>, event: AuditEvent<'_>) -> Result<(), DbError> {
    let metadata = redacted_json_value(event.metadata);
    // Defense-in-depth: refuse metadata / request ids that embed raw secrets.
    let rendered = metadata.to_string();
    for fragment in [
        "\"password\":",
        "\"refreshToken\":",
        "\"refresh_token\":",
        "\"accessToken\":",
        "\"access_token\":",
        "Bearer ",
        "mh1.",
    ] {
        if rendered.contains(fragment) {
            return Err(DbError::Config("audit_metadata_contains_secret".into()));
        }
    }
    if event.request_id.contains("mh1.")
        || event.request_id.contains("Bearer ")
        || event.request_id.starts_with("eyJ")
        || event.request_id.len() > 64
    {
        return Err(DbError::Config("audit_request_id_contains_secret".into()));
    }
    txn.execute(
        "INSERT INTO audit_log (
            org_id, actor_user_id, action, resource_type, resource_id, outcome, metadata, request_id
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        &[
            &event.org_id,
            &event.actor_user_id,
            &event.action,
            &event.resource_type,
            &event.resource_id,
            &event.outcome,
            &metadata,
            &event.request_id,
        ],
    )
    .await?;
    Ok(())
}

/// Password login: verify Argon2id, mint access+refresh, rehash if params changed.
pub async fn login_with_password(
    pool: &deadpool_postgres::Pool,
    auth: &AuthConfig,
    keys: &JwtKeys,
    email: &str,
    password: &str,
    request_id: &str,
) -> Result<TokenPair, SessionError> {
    let email = email.trim().to_ascii_lowercase();
    if email.is_empty() || password.is_empty() || password.len() > 1024 || email.len() > 320 {
        return Err(SessionError::InvalidCredentials);
    }

    let mut client = pool.get().await.map_err(|_| SessionError::Database)?;
    let user_row = client
        .query_opt(
            "SELECT id, password_hash, disabled_at FROM users WHERE email = $1",
            &[&email],
        )
        .await
        .map_err(|_| SessionError::Database)?;

    let Some(user_row) = user_row else {
        // Constant-ish work so unknown emails are not a free timing oracle.
        burn_password_verify_time(password, &auth.argon2);
        // Unknown user: if a single org exists, audit against it without secrets.
        let count: i64 = client
            .query_one("SELECT count(*)::bigint FROM orgs", &[])
            .await
            .map(|row| row.get(0))
            .unwrap_or(0);
        if count == 1 {
            if let Ok(row) = client
                .query_one("SELECT id FROM orgs ORDER BY created_at LIMIT 1", &[])
                .await
            {
                let org_id: Uuid = row.get(0);
                let _ = audit_on_client(
                    &mut client,
                    AuditEvent {
                        org_id,
                        actor_user_id: None,
                        action: "auth.login",
                        resource_type: "session",
                        resource_id: None,
                        outcome: "deny",
                        request_id,
                        metadata: serde_json::json!({ "reason": "unknown_user" }),
                    },
                )
                .await;
            }
        }
        return Err(SessionError::InvalidCredentials);
    };

    let user_id: Uuid = user_row.get(0);
    let password_hash: Option<String> = user_row.get(1);
    let disabled_at: Option<chrono::DateTime<Utc>> = user_row.get(2);

    // Disabled accounts: still burn verify time, audit distinctly, return generic error.
    if disabled_at.is_some() {
        if let Some(ref hash) = password_hash {
            let _ = password::verify_password(password, hash);
        } else {
            burn_password_verify_time(password, &auth.argon2);
        }
        if let Some(org_id) = find_user_org(&mut client, user_id).await? {
            let _ = audit_on_client(
                &mut client,
                AuditEvent {
                    org_id,
                    actor_user_id: Some(user_id),
                    action: "auth.login",
                    resource_type: "session",
                    resource_id: None,
                    outcome: "deny",
                    request_id,
                    metadata: serde_json::json!({ "reason": "user_disabled" }),
                },
            )
            .await;
        }
        return Err(SessionError::InvalidCredentials);
    }

    let Some(password_hash) = password_hash else {
        burn_password_verify_time(password, &auth.argon2);
        return Err(SessionError::InvalidCredentials);
    };
    if password::verify_password(password, &password_hash).is_err() {
        if let Some(org_id) = find_user_org(&mut client, user_id).await? {
            let _ = audit_on_client(
                &mut client,
                AuditEvent {
                    org_id,
                    actor_user_id: Some(user_id),
                    action: "auth.login",
                    resource_type: "session",
                    resource_id: None,
                    outcome: "deny",
                    request_id,
                    metadata: serde_json::json!({ "reason": "bad_password" }),
                },
            )
            .await;
        }
        return Err(SessionError::InvalidCredentials);
    }

    let Some(org_id) = find_user_org(&mut client, user_id).await? else {
        // Membership missing: audit against any org the caller might have used is
        // impossible without an org id; fail closed without writing tenant audit.
        return Err(SessionError::MembershipMissing);
    };

    // Rehash-on-login when Argon2 params changed.
    if password::needs_rehash(&password_hash, &auth.argon2)? {
        let new_hash = password::hash_password(password, &auth.argon2)?;
        client
            .execute(
                "UPDATE users SET password_hash = $1, updated_at = now() WHERE id = $2",
                &[&new_hash.expose(), &user_id],
            )
            .await
            .map_err(|_| SessionError::Database)?;
    }

    // Current-state authorization before minting tokens.
    let _ctx = resolve_org_context_in_txn(pool, org_id, user_id).await?;

    let family_id = Uuid::new_v4();
    let pair = issue_new_family(
        pool,
        keys,
        auth,
        NewFamilyParams {
            org_id,
            user_id,
            family_id,
            request_id,
            action: "auth.login",
        },
    )
    .await?;
    Ok(pair)
}

/// Dummy Argon2id verify using the configured parameters so timing matches real checks.
fn burn_password_verify_time(password: &str, params: &Argon2Config) {
    use std::sync::Mutex;
    static CACHE: Mutex<Option<(u32, u32, u32, String)>> = Mutex::new(None);
    let hash = {
        let mut guard = CACHE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let key = (params.memory_kib, params.time_cost, params.parallelism);
        match guard.as_ref() {
            Some((m, t, p, hash)) if (*m, *t, *p) == key => hash.clone(),
            _ => {
                let hash = password::hash_password("markhand-timing-oracle-pad", params)
                    .expect("dummy argon2 hash")
                    .expose()
                    .to_string();
                *guard = Some((key.0, key.1, key.2, hash.clone()));
                hash
            }
        }
    };
    let _ = password::verify_password(password, &hash);
}

async fn find_user_org(client: &mut Object, user_id: Uuid) -> Result<Option<Uuid>, SessionError> {
    let orgs = client
        .query("SELECT id FROM orgs ORDER BY created_at", &[])
        .await
        .map_err(|_| SessionError::Database)?;
    for row in orgs {
        let org_id: Uuid = row.get(0);
        // SET LOCAL only survives inside an explicit transaction.
        let txn = client
            .transaction()
            .await
            .map_err(|_| SessionError::Database)?;
        set_org_guc_txn(&txn, org_id).await?;
        let membership = txn
            .query_opt(
                "SELECT 1 FROM org_memberships WHERE org_id = $1 AND user_id = $2",
                &[&org_id, &user_id],
            )
            .await
            .map_err(|_| SessionError::Database)?;
        let found = membership.is_some();
        txn.commit().await.map_err(|_| SessionError::Database)?;
        if found {
            return Ok(Some(org_id));
        }
    }
    Ok(None)
}

async fn audit_on_client(client: &mut Object, event: AuditEvent<'_>) -> Result<(), SessionError> {
    let org_id = event.org_id;
    let txn = client
        .transaction()
        .await
        .map_err(|_| SessionError::Database)?;
    set_org_guc_txn(&txn, org_id).await?;
    write_audit(&txn, event).await?;
    txn.commit().await.map_err(|_| SessionError::Database)?;
    Ok(())
}

async fn set_org_guc_txn(txn: &Transaction<'_>, org_id: Uuid) -> Result<(), DbError> {
    let org = org_id.to_string();
    txn.execute("SELECT set_config('app.org_id', $1, true)", &[&org])
        .await?;
    Ok(())
}

struct NewFamilyParams<'a> {
    org_id: Uuid,
    user_id: Uuid,
    family_id: Uuid,
    request_id: &'a str,
    action: &'a str,
}

async fn issue_new_family(
    pool: &deadpool_postgres::Pool,
    keys: &JwtKeys,
    auth: &AuthConfig,
    params: NewFamilyParams<'_>,
) -> Result<TokenPair, SessionError> {
    let NewFamilyParams {
        org_id,
        user_id,
        family_id,
        request_id,
        action,
    } = params;
    let refresh = mint_refresh_token(org_id);
    let token_hash = hash_refresh_token(refresh.expose());
    let refresh_id = Uuid::new_v4();
    let expires_at = Utc::now() + Duration::seconds(auth.refresh_token_ttl_secs as i64);
    let access = keys.sign_access_token(user_id, org_id, family_id)?;
    let provisional = OrgContext::try_new(org_id, user_id, [] as [&str; 0], [])
        .map_err(|_| SessionError::Database)?;

    with_org_txn(pool, &provisional, {
        let request_id = request_id.to_string();
        let action = action.to_string();
        move |txn| {
            Box::pin(async move {
                // Serialize with revoke_all: user lock first, then the new family lock.
                lock_user_sessions(txn, user_id)
                    .await
                    .map_err(|_| DbError::Config("user_lock_failed".into()))?;
                lock_refresh_family(txn, family_id)
                    .await
                    .map_err(|_| DbError::Config("family_lock_failed".into()))?;
                txn.execute(
                    "INSERT INTO refresh_tokens (
                        id, org_id, user_id, family_id, token_hash, expires_at
                     ) VALUES ($1, $2, $3, $4, $5, $6)",
                    &[
                        &refresh_id,
                        &org_id,
                        &user_id,
                        &family_id,
                        &token_hash,
                        &expires_at,
                    ],
                )
                .await?;
                write_audit(
                    txn,
                    AuditEvent {
                        org_id,
                        actor_user_id: Some(user_id),
                        action: &action,
                        resource_type: "session",
                        resource_id: Some(&family_id.to_string()),
                        outcome: "success",
                        request_id: &request_id,
                        metadata: serde_json::json!({
                            "familyId": family_id.to_string(),
                            "refreshTokenId": refresh_id.to_string()
                        }),
                    },
                )
                .await?;
                Ok(())
            })
        }
    })
    .await?;

    Ok(TokenPair {
        access_token: access,
        refresh_token: refresh,
        expires_in: auth.access_token_ttl_secs,
        family_id,
        org_id,
        user_id,
    })
}

/// Rotates a refresh token. Reuse of an already-rotated token revokes the whole family.
pub async fn refresh_session(
    pool: &deadpool_postgres::Pool,
    auth: &AuthConfig,
    keys: &JwtKeys,
    refresh_token: &str,
    request_id: &str,
) -> Result<TokenPair, SessionError> {
    if refresh_token.len() > 512 {
        return Err(SessionError::InvalidRefresh);
    }
    let (org_id, _) = parse_refresh_token(refresh_token)?;
    let presented_hash = hash_refresh_token(refresh_token);

    let mut client = pool.get().await.map_err(|_| SessionError::Database)?;
    let txn = client
        .transaction()
        .await
        .map_err(|_| SessionError::Database)?;
    set_org_guc_txn(&txn, org_id).await?;

    // Unlocked lookup only to discover family_id (and reject unknown tokens).
    let row = txn
        .query_opt(
            "SELECT id, user_id, family_id
             FROM refresh_tokens
             WHERE token_hash = $1",
            &[&presented_hash],
        )
        .await
        .map_err(|_| SessionError::Database)?;

    let Some(row) = row else {
        write_audit(
            &txn,
            AuditEvent {
                org_id,
                actor_user_id: None,
                action: "auth.refresh",
                resource_type: "session",
                resource_id: None,
                outcome: "deny",
                request_id,
                metadata: serde_json::json!({ "reason": "unknown_token" }),
            },
        )
        .await?;
        txn.commit().await.map_err(|_| SessionError::Database)?;
        return Err(SessionError::InvalidRefresh);
    };

    let token_id: Uuid = row.get(0);
    let user_id: Uuid = row.get(1);
    let family_id: Uuid = row.get(2);

    // FAMILY-FIRST: serialize every refresh/reuse on the family before touching rows.
    // Prevents deadlock (different token-row lock order) and partial-commit races where
    // reuse revocation is aborted while a successor rotation commits.
    lock_refresh_family(&txn, family_id).await?;

    // Authoritative read under the family lock.
    let row = txn
        .query_one(
            "SELECT expires_at, revoked_at, replaced_by_id
             FROM refresh_tokens
             WHERE id = $1 AND org_id = $2
             FOR UPDATE",
            &[&token_id, &org_id],
        )
        .await
        .map_err(|_| SessionError::Database)?;
    let expires_at: chrono::DateTime<Utc> = row.get(0);
    let revoked_at: Option<chrono::DateTime<Utc>> = row.get(1);
    let replaced_by_id: Option<Uuid> = row.get(2);

    // Reuse detection: already rotated or revoked → revoke whole family.
    if revoked_at.is_some() || replaced_by_id.is_some() {
        revoke_family_in_txn(&txn, org_id, family_id).await?;
        write_audit(
            &txn,
            AuditEvent {
                org_id,
                actor_user_id: Some(user_id),
                action: "auth.refresh.reuse",
                resource_type: "session",
                resource_id: Some(&family_id.to_string()),
                outcome: "deny",
                request_id,
                metadata: serde_json::json!({
                    "reason": "refresh_reuse",
                    "familyId": family_id.to_string(),
                    "tokenId": token_id.to_string()
                }),
            },
        )
        .await?;
        txn.commit().await.map_err(|_| SessionError::Database)?;
        return Err(SessionError::RefreshReuse);
    }

    if expires_at <= Utc::now() {
        txn.execute(
            "UPDATE refresh_tokens SET revoked_at = now() WHERE id = $1 AND org_id = $2",
            &[&token_id, &org_id],
        )
        .await
        .map_err(|_| SessionError::Database)?;
        write_audit(
            &txn,
            AuditEvent {
                org_id,
                actor_user_id: Some(user_id),
                action: "auth.refresh",
                resource_type: "session",
                resource_id: Some(&family_id.to_string()),
                outcome: "deny",
                request_id,
                metadata: serde_json::json!({ "reason": "expired" }),
            },
        )
        .await?;
        txn.commit().await.map_err(|_| SessionError::Database)?;
        return Err(SessionError::InvalidRefresh);
    }

    // Current-state checks (disabled / membership).
    let user_row = txn
        .query_opt("SELECT disabled_at FROM users WHERE id = $1", &[&user_id])
        .await
        .map_err(|_| SessionError::Database)?;
    let disabled_at: Option<chrono::DateTime<Utc>> =
        user_row.ok_or(SessionError::InvalidRefresh)?.get(0);
    if disabled_at.is_some() {
        revoke_family_in_txn(&txn, org_id, family_id).await?;
        write_audit(
            &txn,
            AuditEvent {
                org_id,
                actor_user_id: Some(user_id),
                action: "auth.refresh",
                resource_type: "session",
                resource_id: Some(&family_id.to_string()),
                outcome: "deny",
                request_id,
                metadata: serde_json::json!({ "reason": "user_disabled" }),
            },
        )
        .await?;
        txn.commit().await.map_err(|_| SessionError::Database)?;
        return Err(SessionError::UserDisabled);
    }
    let membership = txn
        .query_opt(
            "SELECT 1 FROM org_memberships WHERE org_id = $1 AND user_id = $2",
            &[&org_id, &user_id],
        )
        .await
        .map_err(|_| SessionError::Database)?;
    if membership.is_none() {
        revoke_family_in_txn(&txn, org_id, family_id).await?;
        write_audit(
            &txn,
            AuditEvent {
                org_id,
                actor_user_id: Some(user_id),
                action: "auth.refresh",
                resource_type: "session",
                resource_id: Some(&family_id.to_string()),
                outcome: "deny",
                request_id,
                metadata: serde_json::json!({ "reason": "membership_missing" }),
            },
        )
        .await?;
        txn.commit().await.map_err(|_| SessionError::Database)?;
        return Err(SessionError::MembershipMissing);
    }

    let new_refresh = mint_refresh_token(org_id);
    let new_hash = hash_refresh_token(new_refresh.expose());
    let new_id = Uuid::new_v4();
    let new_expires = Utc::now() + Duration::seconds(auth.refresh_token_ttl_secs as i64);

    txn.execute(
        "INSERT INTO refresh_tokens (
            id, org_id, user_id, family_id, token_hash, expires_at
         ) VALUES ($1, $2, $3, $4, $5, $6)",
        &[
            &new_id,
            &org_id,
            &user_id,
            &family_id,
            &new_hash,
            &new_expires,
        ],
    )
    .await
    .map_err(|_| SessionError::Database)?;

    let updated = txn
        .execute(
            "UPDATE refresh_tokens
             SET revoked_at = now(), replaced_by_id = $1
             WHERE id = $2
               AND org_id = $3
               AND revoked_at IS NULL
               AND replaced_by_id IS NULL",
            &[&new_id, &token_id, &org_id],
        )
        .await
        .map_err(|_| SessionError::Database)?;

    if updated != 1 {
        // Lost a race after family lock should be rare; treat as reuse.
        revoke_family_in_txn(&txn, org_id, family_id).await?;
        write_audit(
            &txn,
            AuditEvent {
                org_id,
                actor_user_id: Some(user_id),
                action: "auth.refresh.reuse",
                resource_type: "session",
                resource_id: Some(&family_id.to_string()),
                outcome: "deny",
                request_id,
                metadata: serde_json::json!({ "reason": "refresh_race" }),
            },
        )
        .await?;
        txn.commit().await.map_err(|_| SessionError::Database)?;
        return Err(SessionError::RefreshReuse);
    }

    write_audit(
        &txn,
        AuditEvent {
            org_id,
            actor_user_id: Some(user_id),
            action: "auth.refresh",
            resource_type: "session",
            resource_id: Some(&family_id.to_string()),
            outcome: "success",
            request_id,
            metadata: serde_json::json!({
                "familyId": family_id.to_string(),
                "refreshTokenId": new_id.to_string(),
                "replacedTokenId": token_id.to_string()
            }),
        },
    )
    .await?;
    txn.commit().await.map_err(|_| SessionError::Database)?;

    let access = keys.sign_access_token(user_id, org_id, family_id)?;
    Ok(TokenPair {
        access_token: access,
        refresh_token: new_refresh,
        expires_in: auth.access_token_ttl_secs,
        family_id,
        org_id,
        user_id,
    })
}

/// Transaction-scoped advisory lock that serializes all refresh activity for a family.
///
/// MUST be acquired before any `FOR UPDATE` on individual refresh_tokens rows.
pub async fn lock_refresh_family(
    txn: &Transaction<'_>,
    family_id: Uuid,
) -> Result<(), SessionError> {
    let family = family_id.to_string();
    txn.execute(
        "SELECT pg_advisory_xact_lock(hashtext($1::text))",
        &[&family],
    )
    .await
    .map_err(|_| SessionError::Database)?;
    Ok(())
}

/// User-level advisory lock shared by new-family issuance and revoke-all.
///
/// Lock order (global): user lock first, then per-family locks sorted by family_id.
/// `refresh_session` only takes its single family lock (compatible — it does not create families).
pub async fn lock_user_sessions(txn: &Transaction<'_>, user_id: Uuid) -> Result<(), SessionError> {
    let key = format!("user:{user_id}");
    txn.execute("SELECT pg_advisory_xact_lock(hashtext($1::text))", &[&key])
        .await
        .map_err(|_| SessionError::Database)?;
    Ok(())
}

/// Holds a family advisory lock in a background transaction until [`Self::release`].
///
/// Used by integration tests to force real contention: refresh paths that take the
/// same family-first lock will block until this guard is released.
pub struct FamilyLockHold {
    release_tx: Option<tokio::sync::oneshot::Sender<()>>,
    done_rx: Option<tokio::sync::oneshot::Receiver<Result<(), SessionError>>>,
}

impl FamilyLockHold {
    /// Commits the holding transaction, releasing the advisory lock.
    pub async fn release(mut self) -> Result<(), SessionError> {
        if let Some(tx) = self.release_tx.take() {
            let _ = tx.send(());
        }
        match self.done_rx.take() {
            Some(rx) => rx.await.map_err(|_| SessionError::Database)?,
            None => Ok(()),
        }
    }
}

/// Acquires the family advisory lock and keeps it until [`FamilyLockHold::release`].
pub async fn acquire_family_lock_for_test(
    pool: &deadpool_postgres::Pool,
    org_id: Uuid,
    family_id: Uuid,
) -> Result<FamilyLockHold, SessionError> {
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<(), SessionError>>();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<Result<(), SessionError>>();
    let pool = pool.clone();

    tokio::spawn(async move {
        let fail = |ready: tokio::sync::oneshot::Sender<Result<(), SessionError>>,
                    done: tokio::sync::oneshot::Sender<Result<(), SessionError>>,
                    error: SessionError| {
            let _ = ready.send(Err(error));
            let _ = done.send(Err(error));
        };

        let mut client = match pool.get().await {
            Ok(client) => client,
            Err(_) => {
                fail(ready_tx, done_tx, SessionError::Database);
                return;
            }
        };
        let txn = match client.transaction().await {
            Ok(txn) => txn,
            Err(_) => {
                fail(ready_tx, done_tx, SessionError::Database);
                return;
            }
        };
        if set_org_guc_txn(&txn, org_id).await.is_err() {
            fail(ready_tx, done_tx, SessionError::Database);
            return;
        }
        if lock_refresh_family(&txn, family_id).await.is_err() {
            fail(ready_tx, done_tx, SessionError::Database);
            return;
        }
        let _ = ready_tx.send(Ok(()));
        let _ = release_rx.await;
        let outcome = txn
            .commit()
            .await
            .map(|_| ())
            .map_err(|_| SessionError::Database);
        let _ = done_tx.send(outcome);
    });

    ready_rx.await.map_err(|_| SessionError::Database)??;
    Ok(FamilyLockHold {
        release_tx: Some(release_tx),
        done_rx: Some(done_rx),
    })
}

async fn revoke_family_in_txn(
    txn: &Transaction<'_>,
    org_id: Uuid,
    family_id: Uuid,
) -> Result<(), DbError> {
    txn.execute(
        "UPDATE refresh_tokens
         SET revoked_at = COALESCE(revoked_at, now())
         WHERE org_id = $1 AND family_id = $2 AND revoked_at IS NULL",
        &[&org_id, &family_id],
    )
    .await?;
    Ok(())
}

/// Logout: revoke the presented refresh token's family.
pub async fn logout_session(
    pool: &deadpool_postgres::Pool,
    refresh_token: &str,
    request_id: &str,
) -> Result<(), SessionError> {
    let (org_id, _) = parse_refresh_token(refresh_token)?;
    let presented_hash = hash_refresh_token(refresh_token);
    let mut client = pool.get().await.map_err(|_| SessionError::Database)?;
    let txn = client
        .transaction()
        .await
        .map_err(|_| SessionError::Database)?;
    set_org_guc_txn(&txn, org_id).await?;

    // Unlocked lookup to discover family, then family-first advisory lock.
    let row = txn
        .query_opt(
            "SELECT id, user_id, family_id FROM refresh_tokens WHERE token_hash = $1",
            &[&presented_hash],
        )
        .await
        .map_err(|_| SessionError::Database)?;

    if let Some(row) = row {
        let user_id: Uuid = row.get(1);
        let family_id: Uuid = row.get(2);
        lock_refresh_family(&txn, family_id).await?;
        revoke_family_in_txn(&txn, org_id, family_id).await?;
        write_audit(
            &txn,
            AuditEvent {
                org_id,
                actor_user_id: Some(user_id),
                action: "auth.logout",
                resource_type: "session",
                resource_id: Some(&family_id.to_string()),
                outcome: "success",
                request_id,
                metadata: serde_json::json!({ "familyId": family_id.to_string() }),
            },
        )
        .await?;
    } else {
        write_audit(
            &txn,
            AuditEvent {
                org_id,
                actor_user_id: None,
                action: "auth.logout",
                resource_type: "session",
                resource_id: None,
                outcome: "deny",
                request_id,
                metadata: serde_json::json!({ "reason": "unknown_token" }),
            },
        )
        .await?;
    }
    txn.commit().await.map_err(|_| SessionError::Database)?;
    Ok(())
}

/// Revokes every refresh-token family for a user (disable / password-reset).
///
/// Takes the user-level advisory lock (shared with login/new-family issuance), then
/// per-family locks in sorted order, then revokes — so a concurrent refresh cannot
/// leave a successor active after revocation.
pub async fn revoke_all_user_families(
    pool: &deadpool_postgres::Pool,
    org_id: Uuid,
    user_id: Uuid,
    request_id: &str,
    reason: &str,
) -> Result<(), SessionError> {
    let provisional = OrgContext::try_new(org_id, user_id, [] as [&str; 0], [])
        .map_err(|_| SessionError::Database)?;
    let reason = reason.to_string();
    let request_id = request_id.to_string();
    with_org_txn(pool, &provisional, move |txn| {
        Box::pin(async move {
            // 1) User lock first — blocks concurrent login/new-family for this user.
            lock_user_sessions(txn, user_id)
                .await
                .map_err(|_| DbError::Config("user_lock_failed".into()))?;

            // 2) Discover families, lock each in deterministic order, then revoke.
            let family_rows = txn
                .query(
                    "SELECT DISTINCT family_id
                     FROM refresh_tokens
                     WHERE org_id = $1 AND user_id = $2
                     ORDER BY family_id",
                    &[&org_id, &user_id],
                )
                .await?;
            let mut family_ids: Vec<Uuid> = family_rows.iter().map(|row| row.get(0)).collect();
            family_ids.sort_unstable();
            family_ids.dedup();
            for family_id in &family_ids {
                lock_refresh_family(txn, *family_id)
                    .await
                    .map_err(|_| DbError::Config("family_lock_failed".into()))?;
            }

            txn.execute(
                "UPDATE refresh_tokens
                 SET revoked_at = COALESCE(revoked_at, now())
                 WHERE org_id = $1 AND user_id = $2 AND revoked_at IS NULL",
                &[&org_id, &user_id],
            )
            .await?;

            // Re-scan under locks for any family that appeared after the first SELECT
            // (should be empty once user-lock serializes login; defense in depth).
            let late_rows = txn
                .query(
                    "SELECT DISTINCT family_id
                     FROM refresh_tokens
                     WHERE org_id = $1 AND user_id = $2 AND revoked_at IS NULL
                     ORDER BY family_id",
                    &[&org_id, &user_id],
                )
                .await?;
            for row in late_rows {
                let family_id: Uuid = row.get(0);
                lock_refresh_family(txn, family_id)
                    .await
                    .map_err(|_| DbError::Config("family_lock_failed".into()))?;
                revoke_family_in_txn(txn, org_id, family_id).await?;
            }

            write_audit(
                txn,
                AuditEvent {
                    org_id,
                    actor_user_id: Some(user_id),
                    action: "auth.revoke_all",
                    resource_type: "session",
                    resource_id: Some(&user_id.to_string()),
                    outcome: "success",
                    request_id: &request_id,
                    metadata: serde_json::json!({ "reason": reason }),
                },
            )
            .await?;
            Ok(())
        })
    })
    .await?;
    Ok(())
}

/// Loads display profile fields for `/me` (users table has no RLS).
pub async fn load_user_profile(
    pool: &deadpool_postgres::Pool,
    user_id: Uuid,
) -> Result<(String, String), SessionError> {
    let client = pool.get().await.map_err(|_| SessionError::Database)?;
    let row = client
        .query_opt(
            "SELECT email, display_name FROM users WHERE id = $1",
            &[&user_id],
        )
        .await
        .map_err(|_| SessionError::Database)?
        .ok_or(SessionError::InvalidCredentials)?;
    Ok((row.get(0), row.get(1)))
}

/// Sets a user's password hash (test/bootstrap helper).
pub async fn set_password_hash(
    pool: &deadpool_postgres::Pool,
    user_id: Uuid,
    password: &str,
    params: &Argon2Config,
) -> Result<(), SessionError> {
    let hash = password::hash_password(password, params)?;
    let client = pool.get().await.map_err(|_| SessionError::Database)?;
    client
        .execute(
            "UPDATE users SET password_hash = $1, updated_at = now() WHERE id = $2",
            &[&hash.expose(), &user_id],
        )
        .await
        .map_err(|_| SessionError::Database)?;
    Ok(())
}

/// Applies org context GUCs on an existing transaction (re-export helper).
pub async fn bind_org_context(txn: &Transaction<'_>, ctx: &OrgContext) -> Result<(), DbError> {
    apply_org_context(txn, ctx).await
}
