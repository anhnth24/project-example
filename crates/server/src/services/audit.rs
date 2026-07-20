//! Safe append-only audit helpers built on the durable auth/session writer.

use serde_json::Value;

use crate::auth::context::OrgContext;
use crate::auth::session::{write_audit, AuditEvent};
use crate::db::error::DbError;
use crate::db::pool::with_org_txn;
use crate::telemetry::redacted_json_value;

pub struct SafeAuditEvent {
    pub action: &'static str,
    pub resource_type: &'static str,
    pub resource_id: Option<String>,
    pub outcome: &'static str,
    pub request_id: String,
    pub metadata: Value,
}

pub async fn record_audit_event(
    pool: &deadpool_postgres::Pool,
    ctx: &OrgContext,
    event: SafeAuditEvent,
) -> Result<(), DbError> {
    let txn_ctx = ctx.clone();
    let event_ctx = ctx.clone();
    let request_id = event.request_id;
    let resource_id = event.resource_id.filter(|value| !value.trim().is_empty());
    let metadata = redacted_json_value(event.metadata);
    with_org_txn(pool, &txn_ctx, move |txn| {
        let ctx = event_ctx.clone();
        let request_id = request_id.clone();
        let resource_id = resource_id.clone();
        let metadata = metadata.clone();
        let action = event.action;
        let resource_type = event.resource_type;
        let outcome = event.outcome;
        Box::pin(async move {
            write_audit(
                txn,
                AuditEvent {
                    org_id: ctx.org_id(),
                    actor_user_id: Some(ctx.user_id()),
                    action,
                    resource_type,
                    resource_id: resource_id.as_deref(),
                    outcome,
                    request_id: &request_id,
                    metadata,
                },
            )
            .await
        })
    })
    .await
}
