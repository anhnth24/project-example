//! Atomic quota admission, settlement, and HTTP error/header contract.
//!
//! Admission is serialized per `(org_id, resource_kind)` by
//! `pg_advisory_xact_lock(hashtext('quota:' || org_id || ':' || resource_kind))`
//! inside the F04 org transaction helper (`pool::with_org_txn_typed`, same RLS
//! semantics as `with_org_txn`). The client never supplies committed counters
//! or reservation amounts directly: route/services derive amounts from
//! validated server-side facts such as measured upload bytes.

use std::time::Duration;

use axum::http::{header::RETRY_AFTER, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use deadpool_postgres::Pool;
use thiserror::Error;
use uuid::Uuid;

use crate::api::ApiError;
use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::{QuotaReservation, ReservationStatus, ResourceKind};
use crate::db::{pool, quota};

pub const DEFAULT_RESERVATION_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaSnapshot {
    pub resource_kind: ResourceKind,
    pub limit: i64,
    pub committed: i64,
    pub active_reserved: i64,
    pub remaining: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaReservationOutcome {
    pub reservation: QuotaReservation,
    pub quota: QuotaSnapshot,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaDenial {
    pub resource_kind: ResourceKind,
    pub limit: i64,
    pub committed: i64,
    pub active_reserved: i64,
    pub requested: i64,
    pub remaining: i64,
    pub retry_after_secs: Option<u64>,
}

#[derive(Debug, Error)]
pub enum QuotaError {
    #[error("quota is not configured for this organization")]
    NotConfigured,
    #[error("quota reservation key is invalid")]
    InvalidReservationKey,
    #[error("quota amount is invalid")]
    InvalidAmount,
    #[error("quota arithmetic overflow")]
    ArithmeticOverflow,
    #[error("quota exceeded")]
    QuotaExceeded(QuotaDenial),
    #[error("quota reservation not found")]
    ReservationNotFound,
    #[error("quota reservation resource mismatch")]
    ReservationResourceMismatch,
    #[error("quota reservation was refunded and cannot be finalized")]
    RefundedCannotFinalize,
    #[error("quota reservation expired and cannot be finalized")]
    ExpiredCannotFinalize,
    #[error("quota reservation was finalized and cannot be refunded")]
    FinalizedCannotRefund,
    #[error("database error")]
    Database(DbError),
}

impl QuotaError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotConfigured => "quota_not_configured",
            Self::InvalidReservationKey => "quota_invalid_key",
            Self::InvalidAmount => "quota_invalid_amount",
            Self::ArithmeticOverflow => "quota_arithmetic_overflow",
            Self::QuotaExceeded(_) => "quota_exceeded",
            Self::ReservationNotFound => "quota_reservation_not_found",
            Self::ReservationResourceMismatch => "quota_reservation_resource_mismatch",
            Self::RefundedCannotFinalize => "quota_refunded_cannot_finalize",
            Self::ExpiredCannotFinalize => "quota_expired_cannot_finalize",
            Self::FinalizedCannotRefund => "quota_finalized_cannot_refund",
            Self::Database(_) => "quota_database_error",
        }
    }

    pub const fn status_code(&self) -> StatusCode {
        match self {
            Self::QuotaExceeded(_) => StatusCode::TOO_MANY_REQUESTS,
            Self::NotConfigured
            | Self::InvalidReservationKey
            | Self::InvalidAmount
            | Self::ArithmeticOverflow
            | Self::ReservationNotFound
            | Self::ReservationResourceMismatch
            | Self::RefundedCannotFinalize
            | Self::ExpiredCannotFinalize
            | Self::FinalizedCannotRefund
            | Self::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub const fn user_message(&self) -> &'static str {
        match self {
            Self::QuotaExceeded(_) => "Quota exceeded",
            Self::NotConfigured => "Quota is not configured",
            _ => "Quota admission failed",
        }
    }
}

impl From<DbError> for QuotaError {
    fn from(error: DbError) -> Self {
        match error {
            DbError::NotFound => Self::NotConfigured,
            other => Self::Database(other),
        }
    }
}

impl IntoResponse for QuotaError {
    fn into_response(self) -> Response {
        self.into_response_with_request_id(&Uuid::new_v4().to_string())
    }
}

impl QuotaError {
    pub fn into_response_with_request_id(self, request_id: &str) -> Response {
        let status = self.status_code();
        let details = match &self {
            Self::QuotaExceeded(denial) => Some(serde_json::json!({
                "resourceKind": denial.resource_kind.as_str(),
                "limit": denial.limit,
                "used": denial.committed,
                "activeReserved": denial.active_reserved,
                "remaining": denial.remaining,
                "requested": denial.requested,
            })),
            _ => None,
        };
        let mut response = (
            status,
            Json(ApiError {
                code: self.code().into(),
                message: self.user_message().into(),
                request_id: request_id.to_string(),
                details,
            }),
        )
            .into_response();
        if let Self::QuotaExceeded(denial) = &self {
            apply_quota_headers(
                response.headers_mut(),
                &QuotaSnapshot {
                    resource_kind: denial.resource_kind,
                    limit: denial.limit,
                    committed: denial.committed,
                    active_reserved: denial.active_reserved,
                    remaining: denial.remaining,
                },
            );
            if let Some(seconds) = denial.retry_after_secs {
                if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
                    response.headers_mut().insert(RETRY_AFTER, value);
                }
            }
        }
        response
    }
}

pub fn apply_quota_headers(headers: &mut axum::http::HeaderMap, snapshot: &QuotaSnapshot) {
    insert_header(headers, "x-quota-resource", snapshot.resource_kind.as_str());
    insert_header(headers, "x-quota-limit", &snapshot.limit.to_string());
    insert_header(headers, "x-quota-used", &snapshot.committed.to_string());
    insert_header(
        headers,
        "x-quota-reserved",
        &snapshot.active_reserved.to_string(),
    );
    insert_header(
        headers,
        "x-quota-remaining",
        &snapshot.remaining.to_string(),
    );
}

fn insert_header(headers: &mut axum::http::HeaderMap, name: &'static str, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(HeaderName::from_static(name), value);
    }
}

pub async fn reserve(
    db_pool: &Pool,
    ctx: &OrgContext,
    reservation_key: &str,
    resource_kind: ResourceKind,
    amount: u64,
    ttl: Duration,
    job_id: Option<Uuid>,
) -> Result<QuotaReservationOutcome, QuotaError> {
    validate_reservation_key(reservation_key)?;
    let amount = checked_amount(amount)?;
    let ttl_secs = checked_ttl_secs(ttl)?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let reservation_key = reservation_key.to_string();
        move |txn| {
            Box::pin(async move {
                quota::lock_admission(txn, &ctx, resource_kind).await?;
                if let Some(existing) = quota::find_by_key(txn, &ctx, &reservation_key).await? {
                    let snapshot = current_snapshot(txn, &ctx, resource_kind).await?;
                    return Ok(QuotaReservationOutcome {
                        reservation: existing,
                        quota: snapshot,
                        created: false,
                    });
                }

                let usage = quota::usage(txn, &ctx, resource_kind).await?;
                let remaining = remaining(usage.limit, usage.committed, usage.active_reserved)?;
                if amount > remaining {
                    return Err(QuotaError::QuotaExceeded(QuotaDenial {
                        resource_kind,
                        limit: usage.limit,
                        committed: usage.committed,
                        active_reserved: usage.active_reserved,
                        requested: amount,
                        remaining,
                        retry_after_secs: retry_after_for(resource_kind),
                    }));
                }

                let Some(inserted) = quota::insert_reserved(
                    txn,
                    &ctx,
                    &reservation_key,
                    resource_kind,
                    amount,
                    ttl_secs,
                    job_id,
                )
                .await?
                else {
                    let existing = quota::find_by_key(txn, &ctx, &reservation_key)
                        .await?
                        .ok_or(QuotaError::ReservationNotFound)?;
                    let snapshot = current_snapshot(txn, &ctx, resource_kind).await?;
                    return Ok(QuotaReservationOutcome {
                        reservation: existing,
                        quota: snapshot,
                        created: false,
                    });
                };

                let active_reserved = usage
                    .active_reserved
                    .checked_add(amount)
                    .ok_or(QuotaError::ArithmeticOverflow)?;
                Ok(QuotaReservationOutcome {
                    reservation: inserted,
                    quota: QuotaSnapshot {
                        resource_kind,
                        limit: usage.limit,
                        committed: usage.committed,
                        active_reserved,
                        remaining: remaining
                            .checked_sub(amount)
                            .ok_or(QuotaError::ArithmeticOverflow)?,
                    },
                    created: true,
                })
            })
        }
    })
    .await
}

pub async fn finalize(
    db_pool: &Pool,
    ctx: &OrgContext,
    reservation_key: &str,
) -> Result<QuotaReservation, QuotaError> {
    validate_reservation_key(reservation_key)?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let reservation_key = reservation_key.to_string();
        move |txn| {
            Box::pin(async move {
                let reservation = quota::get_by_key_for_update(txn, &ctx, &reservation_key).await?;
                quota::lock_admission(txn, &ctx, reservation.resource_kind).await?;
                match reservation.status {
                    ReservationStatus::Finalized => return Ok(reservation),
                    ReservationStatus::Refunded => return Err(QuotaError::RefundedCannotFinalize),
                    ReservationStatus::Expired => return Err(QuotaError::ExpiredCannotFinalize),
                    ReservationStatus::Reserved => {}
                }
                if reservation.expires_at <= Utc::now() {
                    quota::set_status(txn, &ctx, reservation.id, ReservationStatus::Expired)
                        .await?;
                    return Err(QuotaError::ExpiredCannotFinalize);
                }

                let finalized =
                    quota::set_status(txn, &ctx, reservation.id, ReservationStatus::Finalized)
                        .await?;
                if finalized.resource_kind.counter_key().is_some() {
                    let period = quota::current_period(txn, finalized.resource_kind).await?;
                    let current =
                        quota::lock_committed_counter(txn, &ctx, finalized.resource_kind, period)
                            .await?
                            .unwrap_or(0);
                    let value = current
                        .checked_add(finalized.amount)
                        .ok_or(QuotaError::ArithmeticOverflow)?;
                    quota::upsert_counter_value(txn, &ctx, finalized.resource_kind, period, value)
                        .await?;
                }
                Ok(finalized)
            })
        }
    })
    .await
}

pub async fn refund(
    db_pool: &Pool,
    ctx: &OrgContext,
    reservation_key: &str,
) -> Result<QuotaReservation, QuotaError> {
    validate_reservation_key(reservation_key)?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let reservation_key = reservation_key.to_string();
        move |txn| {
            Box::pin(async move {
                let reservation = quota::get_by_key_for_update(txn, &ctx, &reservation_key).await?;
                quota::lock_admission(txn, &ctx, reservation.resource_kind).await?;
                match reservation.status {
                    ReservationStatus::Refunded | ReservationStatus::Expired => {
                        return Ok(reservation);
                    }
                    ReservationStatus::Finalized => return Err(QuotaError::FinalizedCannotRefund),
                    ReservationStatus::Reserved => {}
                }
                if reservation.expires_at <= Utc::now() {
                    return quota::set_status(
                        txn,
                        &ctx,
                        reservation.id,
                        ReservationStatus::Expired,
                    )
                    .await
                    .map_err(Into::into);
                }
                quota::set_status(txn, &ctx, reservation.id, ReservationStatus::Refunded)
                    .await
                    .map_err(Into::into)
            })
        }
    })
    .await
}

pub async fn expire_reserved(
    db_pool: &Pool,
    ctx: &OrgContext,
    batch_size: u32,
) -> Result<u64, QuotaError> {
    let batch_size = i64::from(batch_size.max(1));
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move { quota::expire_reserved_batch(txn, &ctx, batch_size).await })
        }
    })
    .await
    .map_err(Into::into)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadQuotaReservation {
    pub storage: QuotaReservationOutcome,
    pub document: Option<QuotaReservationOutcome>,
}

impl UploadQuotaReservation {
    pub fn storage_headers(&self) -> QuotaSnapshot {
        self.storage.quota
    }
}

pub async fn reserve_upload(
    db_pool: &Pool,
    ctx: &OrgContext,
    reservation_key: &str,
    size_bytes: u64,
    ttl: Duration,
) -> Result<UploadQuotaReservation, QuotaError> {
    let storage_key = format!("upload.storage.{reservation_key}");
    let document_key = format!("upload.documents.{reservation_key}");
    let storage = reserve(
        db_pool,
        ctx,
        &storage_key,
        ResourceKind::StorageBytes,
        size_bytes,
        ttl,
        None,
    )
    .await?;
    match reserve(
        db_pool,
        ctx,
        &document_key,
        ResourceKind::Documents,
        1,
        ttl,
        None,
    )
    .await
    {
        Ok(document) => Ok(UploadQuotaReservation {
            storage,
            document: Some(document),
        }),
        Err(error) => {
            let _ = refund(db_pool, ctx, &storage_key).await;
            Err(error)
        }
    }
}

pub async fn finalize_upload(
    db_pool: &Pool,
    ctx: &OrgContext,
    reservation_key: &str,
) -> Result<(), QuotaError> {
    let storage_key = format!("upload.storage.{reservation_key}");
    let document_key = format!("upload.documents.{reservation_key}");
    finalize(db_pool, ctx, &document_key).await?;
    finalize(db_pool, ctx, &storage_key).await?;
    Ok(())
}

pub async fn refund_upload(
    db_pool: &Pool,
    ctx: &OrgContext,
    reservation_key: &str,
) -> Result<(), QuotaError> {
    let storage_key = format!("upload.storage.{reservation_key}");
    let document_key = format!("upload.documents.{reservation_key}");
    let document = refund(db_pool, ctx, &document_key).await;
    let storage = refund(db_pool, ctx, &storage_key).await;
    document?;
    storage?;
    Ok(())
}

async fn current_snapshot(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    resource_kind: ResourceKind,
) -> Result<QuotaSnapshot, QuotaError> {
    let usage = quota::usage(txn, ctx, resource_kind).await?;
    Ok(QuotaSnapshot {
        resource_kind,
        limit: usage.limit,
        committed: usage.committed,
        active_reserved: usage.active_reserved,
        remaining: remaining(usage.limit, usage.committed, usage.active_reserved)?,
    })
}

fn remaining(limit: i64, committed: i64, active_reserved: i64) -> Result<i64, QuotaError> {
    if limit < 0 || committed < 0 || active_reserved < 0 {
        return Err(QuotaError::ArithmeticOverflow);
    }
    let used = committed
        .checked_add(active_reserved)
        .ok_or(QuotaError::ArithmeticOverflow)?;
    if used >= limit {
        Ok(0)
    } else {
        limit
            .checked_sub(used)
            .ok_or(QuotaError::ArithmeticOverflow)
    }
}

fn checked_amount(amount: u64) -> Result<i64, QuotaError> {
    if amount == 0 {
        return Err(QuotaError::InvalidAmount);
    }
    i64::try_from(amount).map_err(|_| QuotaError::InvalidAmount)
}

fn checked_ttl_secs(ttl: Duration) -> Result<i64, QuotaError> {
    let secs = ttl.as_secs();
    if secs == 0 {
        return Err(QuotaError::InvalidAmount);
    }
    i64::try_from(secs).map_err(|_| QuotaError::ArithmeticOverflow)
}

fn validate_reservation_key(key: &str) -> Result<(), QuotaError> {
    if key.is_empty() || key.len() > 160 || key.chars().any(char::is_control) {
        return Err(QuotaError::InvalidReservationKey);
    }
    Ok(())
}

const fn retry_after_for(kind: ResourceKind) -> Option<u64> {
    match kind {
        ResourceKind::ConcurrentJobs => Some(30),
        ResourceKind::StorageBytes | ResourceKind::Documents | ResourceKind::Tokens => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_amount_rejects_zero_and_overflow() {
        assert!(matches!(checked_amount(0), Err(QuotaError::InvalidAmount)));
        assert!(matches!(
            checked_amount(i64::MAX as u64 + 1),
            Err(QuotaError::InvalidAmount)
        ));
    }

    #[test]
    fn remaining_is_checked_and_saturates_when_over_limit() {
        assert_eq!(remaining(10, 3, 4).unwrap(), 3);
        assert_eq!(remaining(10, 12, 0).unwrap(), 0);
        assert!(matches!(
            remaining(i64::MAX, i64::MAX, 1),
            Err(QuotaError::ArithmeticOverflow)
        ));
    }
}
