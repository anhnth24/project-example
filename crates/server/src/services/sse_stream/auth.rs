//! Per-event stream auth: membership, session family, collection ACL, cited pins.

use std::collections::BTreeSet;

use chrono::Utc;
use deadpool_postgres::Pool;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::jwt::AccessClaims;
use crate::auth::permissions::{require_permission, resolve_org_context_in_txn};
use crate::auth::session;
use crate::db::models::DocumentState;
use crate::db::pool::with_org_txn;
use crate::db::sse_streams::{load_cited_document_pins, load_cited_version_pins, StreamAuthScope};
use crate::services::deletion::document_reads_suppressed;
use crate::services::qa::stream::AuthProbeDecision;
use crate::services::retrieval::{PERMISSION_QA_HISTORY, PERMISSION_QA_QUERY};

/// Revalidate cited document/version pins against current membership/ACL/state.
///
/// Missing / tombstoned / `deleted_at` → Deleted. Not indexed, unpublished,
/// wrong lineage, or collection ACL → Deny. Empty pins → Allow.
pub async fn probe_cited_pins(
    pool: &Pool,
    ctx: &OrgContext,
    scope: &StreamAuthScope,
) -> AuthProbeDecision {
    if scope.cited_document_ids.is_empty() && scope.cited_version_ids.is_empty() {
        return AuthProbeDecision::Allow;
    }
    let ctx = ctx.clone();
    let scope = scope.clone();
    match with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                if !scope.cited_document_ids.is_empty() {
                    let rows =
                        load_cited_document_pins(txn, &ctx, &scope.cited_document_ids).await?;
                    if rows.len() != scope.cited_document_ids.len() {
                        return Ok(AuthProbeDecision::Deleted);
                    }
                    for row in &rows {
                        let Ok(state) = DocumentState::parse(&row.state) else {
                            return Ok(AuthProbeDecision::Deny);
                        };
                        if document_reads_suppressed(state, row.deleted_at.is_some()) {
                            return Ok(AuthProbeDecision::Deleted);
                        }
                        if state != DocumentState::Indexed {
                            return Ok(AuthProbeDecision::Deny);
                        }
                        if !ctx.allows_collection(row.collection_id) {
                            return Ok(AuthProbeDecision::Deny);
                        }
                    }
                }
                if !scope.cited_version_ids.is_empty() {
                    let rows = load_cited_version_pins(txn, &ctx, &scope.cited_version_ids).await?;
                    if rows.len() != scope.cited_version_ids.len() {
                        return Ok(AuthProbeDecision::Deleted);
                    }
                    let cited_docs: BTreeSet<Uuid> =
                        scope.cited_document_ids.iter().copied().collect();
                    for row in &rows {
                        let Ok(state) = DocumentState::parse(&row.document_state) else {
                            return Ok(AuthProbeDecision::Deny);
                        };
                        if document_reads_suppressed(state, row.deleted_at.is_some()) {
                            return Ok(AuthProbeDecision::Deleted);
                        }
                        if state != DocumentState::Indexed {
                            return Ok(AuthProbeDecision::Deny);
                        }
                        if row.publication_state != "published" {
                            return Ok(AuthProbeDecision::Deny);
                        }
                        if !cited_docs.is_empty() && !cited_docs.contains(&row.document_id) {
                            return Ok(AuthProbeDecision::Deny);
                        }
                        if !ctx.allows_collection(row.collection_id) {
                            return Ok(AuthProbeDecision::Deny);
                        }
                    }
                }
                Ok(AuthProbeDecision::Allow)
            })
        }
    })
    .await
    {
        Ok(decision) => decision,
        Err(_) => AuthProbeDecision::Deny,
    }
}

/// Fresh auth + session-family + permission/collection/cited-pin probe.
///
/// Takes a pool (not HTTP `AppState`) so routes stay free of storage wiring.
pub fn make_auth_probe(
    pool: Pool,
    claims: AccessClaims,
    scope: StreamAuthScope,
) -> impl FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = AuthProbeDecision> + Send>>
       + Send
       + 'static {
    move || {
        let pool = pool.clone();
        let claims = claims.clone();
        let scope = scope.clone();
        Box::pin(async move {
            let now = Utc::now().timestamp();
            if claims.exp <= now {
                return AuthProbeDecision::Deny;
            }
            let Ok(user_id) = Uuid::parse_str(&claims.sub) else {
                return AuthProbeDecision::Deny;
            };
            let Ok(org_id) = Uuid::parse_str(&claims.org_id) else {
                return AuthProbeDecision::Deny;
            };
            let Ok(family_id) = Uuid::parse_str(&claims.sid) else {
                return AuthProbeDecision::Deny;
            };
            let Ok(ctx) = resolve_org_context_in_txn(&pool, org_id, user_id).await else {
                return AuthProbeDecision::Deny;
            };
            if require_permission(&ctx, PERMISSION_QA_QUERY).is_err() {
                return AuthProbeDecision::Deny;
            }
            if scope.requires_history && require_permission(&ctx, PERMISSION_QA_HISTORY).is_err() {
                return AuthProbeDecision::Deny;
            }
            if scope
                .collection_ids
                .iter()
                .any(|id| !ctx.allows_collection(*id))
            {
                return AuthProbeDecision::Deny;
            }
            match session::is_refresh_family_active(&pool, org_id, family_id).await {
                Ok(true) => {}
                Ok(false) | Err(_) => return AuthProbeDecision::Deny,
            }
            probe_cited_pins(&pool, &ctx, &scope).await
        })
    }
}
