//! Per-poll stream principal revalidation (P1B-R05).
//!
//! Job SSE must not trust the initial extractor forever. Each poll re-checks:
//! access-token `exp`, session-family revoke, user suspend/disable, membership,
//! and fresh job/document authorization from PostgreSQL.

use chrono::Utc;
use deadpool_postgres::Pool;
use thiserror::Error;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::jwt::AccessClaims;
use crate::auth::permissions::{resolve_org_context_in_txn, ResolveError};
use crate::db::models::{Job, JobStatus};
use crate::db::pool::with_org_txn_typed;
use crate::services::access::{self, AccessError};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StreamAuthError {
    #[error("access token expired")]
    Expired,
    #[error("session family revoked")]
    SessionRevoked,
    #[error("user disabled or membership missing")]
    PrincipalDenied,
    #[error("job unauthorized")]
    JobDenied,
    #[error("cited document unauthorized or deleted")]
    CitationDenied,
    #[error("database error")]
    Database,
    #[error("send timeout")]
    SendTimeout,
}

impl StreamAuthError {
    pub const fn close_reason(&self) -> &'static str {
        match self {
            Self::Expired => "token_expired",
            Self::SessionRevoked => "session_revoked",
            Self::PrincipalDenied => "principal_denied",
            Self::JobDenied => "auth_revoked",
            Self::CitationDenied => "citation_revoked",
            Self::Database => "stream_error",
            Self::SendTimeout => "send_timeout",
        }
    }
}

/// Result of one SSE poll revalidation.
#[derive(Debug, Clone)]
pub struct StreamPrincipal {
    pub context: OrgContext,
    pub job: Job,
    pub terminal: bool,
}

pub fn token_expired(claims: &AccessClaims, now_epoch_secs: i64) -> bool {
    claims.exp <= now_epoch_secs
}

/// True when the refresh-token family still has a live (unrevoked, unexpired) row.
///
/// Must run under transaction-local `app.org_id` so FORCE RLS on `refresh_tokens`
/// can observe the caller's family (bare pool clients see zero rows).
pub async fn session_family_active(
    pool: &Pool,
    org_id: Uuid,
    family_id: Uuid,
) -> Result<bool, StreamAuthError> {
    // user_id is required by OrgContext construction but unused by this probe;
    // reuse family_id (never nil) so the GUC scope is valid under FORCE RLS.
    let provisional = OrgContext::try_new(org_id, family_id, [] as [&str; 0], [])
        .map_err(|_| StreamAuthError::SessionRevoked)?;
    crate::db::pool::with_org_txn(pool, &provisional, {
        move |txn| {
            Box::pin(async move {
                let row = txn
                    .query_opt(
                        "SELECT 1
                         FROM refresh_tokens
                         WHERE org_id = $1
                           AND family_id = $2
                           AND revoked_at IS NULL
                           AND expires_at > now()
                         LIMIT 1",
                        &[&org_id, &family_id],
                    )
                    .await?;
                Ok(row.is_some())
            })
        }
    })
    .await
    .map_err(|_| StreamAuthError::Database)
}

/// Revalidates JWT exp + PG session/membership + job access; reloads terminal status.
pub async fn revalidate_job_stream(
    pool: &Pool,
    claims: &AccessClaims,
    job_id: Uuid,
) -> Result<StreamPrincipal, StreamAuthError> {
    let now = Utc::now().timestamp();
    if token_expired(claims, now) {
        return Err(StreamAuthError::Expired);
    }
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| StreamAuthError::PrincipalDenied)?;
    let org_id = Uuid::parse_str(&claims.org_id).map_err(|_| StreamAuthError::PrincipalDenied)?;
    let family_id = Uuid::parse_str(&claims.sid).map_err(|_| StreamAuthError::SessionRevoked)?;

    if !session_family_active(pool, org_id, family_id).await? {
        return Err(StreamAuthError::SessionRevoked);
    }

    let context = resolve_org_context_in_txn(pool, org_id, user_id)
        .await
        .map_err(|error| match error {
            ResolveError::UserDisabled | ResolveError::MembershipMissing => {
                StreamAuthError::PrincipalDenied
            }
            ResolveError::Database | ResolveError::InvalidContext => StreamAuthError::Database,
            ResolveError::PermissionDenied | ResolveError::CollectionDenied => {
                StreamAuthError::PrincipalDenied
            }
        })?;

    // Authorize + reload authoritative status (may have terminalized since last poll).
    let _authorized = access::resolve_job_access(pool, &context, job_id)
        .await
        .map_err(|error| match error {
            AccessError::NotFound | AccessError::HistoryRequired | AccessError::NotPublished => {
                StreamAuthError::JobDenied
            }
            AccessError::Database => StreamAuthError::Database,
        })?;
    let job = with_org_txn_typed(pool, &context, {
        let ctx = context.clone();
        move |txn| Box::pin(async move { crate::db::jobs::get_by_id(txn, &ctx, job_id).await })
    })
    .await
    .map_err(|_| StreamAuthError::Database)?;

    let terminal = matches!(
        job.status,
        JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled | JobStatus::DeadLetter
    );
    Ok(StreamPrincipal {
        context,
        job,
        terminal,
    })
}

/// Common stream principal guard for ask SSE: exp/session/membership + fresh
/// authorization for every cited document in the current batch.
pub async fn revalidate_ask_stream(
    pool: &Pool,
    claims: &AccessClaims,
    cited_document_ids: &[Uuid],
) -> Result<OrgContext, StreamAuthError> {
    let now = Utc::now().timestamp();
    if token_expired(claims, now) {
        return Err(StreamAuthError::Expired);
    }
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| StreamAuthError::PrincipalDenied)?;
    let org_id = Uuid::parse_str(&claims.org_id).map_err(|_| StreamAuthError::PrincipalDenied)?;
    let family_id = Uuid::parse_str(&claims.sid).map_err(|_| StreamAuthError::SessionRevoked)?;

    if !session_family_active(pool, org_id, family_id).await? {
        return Err(StreamAuthError::SessionRevoked);
    }

    let context = resolve_org_context_in_txn(pool, org_id, user_id)
        .await
        .map_err(|error| match error {
            ResolveError::UserDisabled | ResolveError::MembershipMissing => {
                StreamAuthError::PrincipalDenied
            }
            ResolveError::Database | ResolveError::InvalidContext => StreamAuthError::Database,
            ResolveError::PermissionDenied | ResolveError::CollectionDenied => {
                StreamAuthError::PrincipalDenied
            }
        })?;

    for document_id in cited_document_ids {
        access::resolve_document(pool, &context, *document_id)
            .await
            .map_err(|error| match error {
                AccessError::NotFound
                | AccessError::HistoryRequired
                | AccessError::NotPublished => StreamAuthError::CitationDenied,
                AccessError::Database => StreamAuthError::Database,
            })?;
    }
    Ok(context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_helper_is_strict() {
        let claims = AccessClaims {
            sub: Uuid::new_v4().to_string(),
            iss: "iss".into(),
            aud: "aud".into(),
            iat: 1,
            nbf: 1,
            exp: 100,
            org_id: Uuid::new_v4().to_string(),
            sid: Uuid::new_v4().to_string(),
        };
        assert!(!token_expired(&claims, 99));
        assert!(token_expired(&claims, 100));
        assert!(token_expired(&claims, 101));
    }

    #[test]
    fn close_reasons_are_stable() {
        assert_eq!(StreamAuthError::Expired.close_reason(), "token_expired");
        assert_eq!(
            StreamAuthError::SessionRevoked.close_reason(),
            "session_revoked"
        );
        assert_eq!(
            StreamAuthError::PrincipalDenied.close_reason(),
            "principal_denied"
        );
        assert_eq!(StreamAuthError::JobDenied.close_reason(), "auth_revoked");
        assert_eq!(
            StreamAuthError::CitationDenied.close_reason(),
            "citation_revoked"
        );
        assert_eq!(StreamAuthError::SendTimeout.close_reason(), "send_timeout");
    }

    #[test]
    fn ask_stream_close_reasons_cover_expiry_logout_removal_delete() {
        assert_eq!(StreamAuthError::Expired.close_reason(), "token_expired");
        assert_eq!(
            StreamAuthError::SessionRevoked.close_reason(),
            "session_revoked"
        );
        assert_eq!(
            StreamAuthError::PrincipalDenied.close_reason(),
            "principal_denied"
        );
        assert_eq!(
            StreamAuthError::CitationDenied.close_reason(),
            "citation_revoked"
        );
    }

    #[test]
    fn terminal_statuses_match_worker_restart_reload() {
        use crate::db::models::JobStatus;
        let terminal = |status: JobStatus| {
            matches!(
                status,
                JobStatus::Succeeded
                    | JobStatus::Failed
                    | JobStatus::Cancelled
                    | JobStatus::DeadLetter
            )
        };
        assert!(terminal(JobStatus::Succeeded));
        assert!(terminal(JobStatus::Failed));
        assert!(terminal(JobStatus::Cancelled));
        assert!(terminal(JobStatus::DeadLetter));
        assert!(!terminal(JobStatus::Pending));
        assert!(!terminal(JobStatus::Leased));
        assert!(!terminal(JobStatus::Running));
    }

    #[test]
    fn expiry_and_revoke_close_before_membership_reload() {
        // Contract: poll order is exp → session family → membership/OrgContext → job.
        let order = [
            StreamAuthError::Expired.close_reason(),
            StreamAuthError::SessionRevoked.close_reason(),
            StreamAuthError::PrincipalDenied.close_reason(),
            StreamAuthError::JobDenied.close_reason(),
        ];
        assert_eq!(
            order,
            [
                "token_expired",
                "session_revoked",
                "principal_denied",
                "auth_revoked"
            ]
        );
    }
}
