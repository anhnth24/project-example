//! Tenant-scoped typed claim persistence and conflict-candidate lookup.
//!
//! Claims use deterministic UUIDs supplied by the extraction service. Replaying
//! an index batch therefore converges without duplicating evidence.

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use tokio_postgres::Transaction;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;

#[derive(Debug, Clone)]
pub struct NewClaim<'a> {
    pub id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub chunk_id: Uuid,
    pub claim_key: &'a str,
    pub subject: &'a str,
    pub predicate: &'a str,
    pub value_type: &'a str,
    pub value_number: Option<Decimal>,
    pub value_text: Option<&'a str>,
    pub value_boolean: Option<bool>,
    pub value_date: Option<NaiveDate>,
    pub value_money: Option<Decimal>,
    pub unit: Option<&'a str>,
    pub scope: &'a str,
    pub effective_from: DateTime<Utc>,
    pub effective_to: Option<DateTime<Utc>>,
    pub citation_quote: &'a str,
    pub citation_span_start: i32,
    pub citation_span_end: i32,
}

/// Inserts a deterministic claim identity exactly once.
pub async fn insert_if_absent(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: &NewClaim<'_>,
) -> Result<Uuid, DbError> {
    let inserted = txn
        .query_opt(
            "INSERT INTO claims (
                id, org_id, document_id, version_id, chunk_id, claim_key, subject,
                predicate, value_type, value_number, value_text, value_boolean,
                value_date, value_money, unit, scope, effective_from, effective_to,
                citation_quote, citation_span_start, citation_span_end
             ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                $15, $16, $17, $18, $19, $20, $21
             )
             ON CONFLICT (id) DO NOTHING
             RETURNING id",
            &[
                &input.id,
                &ctx.org_id(),
                &input.document_id,
                &input.version_id,
                &input.chunk_id,
                &input.claim_key,
                &input.subject,
                &input.predicate,
                &input.value_type,
                &input.value_number,
                &input.value_text,
                &input.value_boolean,
                &input.value_date,
                &input.value_money,
                &input.unit,
                &input.scope,
                &input.effective_from,
                &input.effective_to,
                &input.citation_quote,
                &input.citation_span_start,
                &input.citation_span_end,
            ],
        )
        .await?;
    if let Some(row) = inserted {
        return Ok(row.get("id"));
    }

    txn.query_one(
        "SELECT id
         FROM claims
         WHERE org_id = $1 AND id = $2",
        &[&ctx.org_id(), &input.id],
    )
    .await
    .map(|row| row.get("id"))
    .map_err(Into::into)
}

/// Returns existing, incompatible claims with the same deterministic comparison
/// dimensions and an overlapping effective interval.
pub async fn find_conflict_candidates(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: &NewClaim<'_>,
) -> Result<Vec<Uuid>, DbError> {
    let rows = txn
        .query(
            "SELECT id
             FROM claims
             WHERE org_id = $1
               AND id <> $2
               AND claim_key = $3
               AND subject = $4
               AND predicate = $5
               AND scope = $6
               AND value_type = $7
               AND (effective_to IS NULL OR effective_to > $8)
               AND ($9::timestamptz IS NULL OR effective_from < $9)
               AND (
                   value_number IS DISTINCT FROM $10
                   OR value_text IS DISTINCT FROM $11
                   OR value_boolean IS DISTINCT FROM $12
                   OR value_date IS DISTINCT FROM $13
                   OR value_money IS DISTINCT FROM $14
                   OR unit IS DISTINCT FROM $15
               )
             ORDER BY id",
            &[
                &ctx.org_id(),
                &input.id,
                &input.claim_key,
                &input.subject,
                &input.predicate,
                &input.scope,
                &input.value_type,
                &input.effective_from,
                &input.effective_to,
                &input.value_number,
                &input.value_text,
                &input.value_boolean,
                &input.value_date,
                &input.value_money,
                &input.unit,
            ],
        )
        .await?;
    Ok(rows.iter().map(|row| row.get("id")).collect())
}
