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
use chrono::{DateTime, Utc};
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
pub struct UploadQuotaReservation {
    pub storage: QuotaReservationOutcome,
    pub document: QuotaReservationOutcome,
}

impl UploadQuotaReservation {
    pub fn storage_headers(&self) -> QuotaSnapshot {
        self.storage.quota
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadQuotaSettlement {
    pub storage: QuotaSettlement,
    pub document: QuotaSettlement,
    pub storage_quota: QuotaSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaSettlement {
    Reserved(QuotaReservation),
    Finalized(QuotaReservation),
    AlreadyFinalized(QuotaReservation),
    Refunded(QuotaReservation),
    AlreadyRefunded(QuotaReservation),
    Expired(QuotaReservation),
    RefundedCannotFinalize(QuotaReservation),
    FinalizedCannotRefund(QuotaReservation),
}

impl QuotaSettlement {
    pub const fn reservation(&self) -> &QuotaReservation {
        match self {
            Self::Reserved(reservation)
            | Self::Finalized(reservation)
            | Self::AlreadyFinalized(reservation)
            | Self::Refunded(reservation)
            | Self::AlreadyRefunded(reservation)
            | Self::Expired(reservation)
            | Self::RefundedCannotFinalize(reservation)
            | Self::FinalizedCannotRefund(reservation) => reservation,
        }
    }

    pub const fn is_finalize_success(&self) -> bool {
        matches!(self, Self::Finalized(_) | Self::AlreadyFinalized(_))
    }

    pub const fn is_refund_success(&self) -> bool {
        matches!(
            self,
            Self::Refunded(_) | Self::AlreadyRefunded(_) | Self::Expired(_)
        )
    }
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
    #[error("quota reservation key conflicts with a different or terminal reservation")]
    ReservationConflict,
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
            Self::ReservationConflict => "quota_reservation_conflict",
            Self::RefundedCannotFinalize => "quota_refunded_cannot_finalize",
            Self::ExpiredCannotFinalize => "quota_expired_cannot_finalize",
            Self::FinalizedCannotRefund => "quota_finalized_cannot_refund",
            Self::Database(_) => "quota_database_error",
        }
    }

    pub const fn status_code(&self) -> StatusCode {
        match self {
            Self::QuotaExceeded(_) => StatusCode::TOO_MANY_REQUESTS,
            Self::ReservationConflict => StatusCode::CONFLICT,
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
        Self::Database(error)
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
    let result = reserve_inner(
        db_pool,
        ctx,
        reservation_key,
        resource_kind,
        amount,
        ttl,
        job_id,
    )
    .await;
    match &result {
        Ok(outcome) if outcome.created => {
            crate::telemetry::record_quota("reserve", resource_kind.as_str())
        }
        Ok(_) => {}
        Err(QuotaError::QuotaExceeded(_)) => {
            persist_quota_deny_audit(db_pool, ctx, resource_kind).await;
            crate::telemetry::record_quota("deny", resource_kind.as_str())
        }
        Err(_) => crate::telemetry::record_quota("error", resource_kind.as_str()),
    }
    result
}

async fn reserve_inner(
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
                let observed_at = quota::fresh_clock_timestamp(txn).await?;
                if quota::find_by_key(txn, &ctx, &reservation_key)
                    .await?
                    .is_some()
                {
                    let Some(active) = quota::find_active_matching_by_key(
                        txn,
                        &ctx,
                        &reservation_key,
                        resource_kind,
                        amount,
                        job_id,
                        observed_at,
                    )
                    .await?
                    else {
                        return Err(QuotaError::ReservationConflict);
                    };
                    let snapshot = current_snapshot(txn, &ctx, resource_kind, observed_at).await?;
                    return Ok(QuotaReservationOutcome {
                        reservation: active,
                        quota: snapshot,
                        created: false,
                    });
                }

                let usage = quota::usage(txn, &ctx, resource_kind, observed_at)
                    .await
                    .map_err(map_quota_config_error)?;
                let remaining = remaining(usage.limit, usage.committed, usage.active_reserved)?;
                if amount > remaining {
                    // Do not audit inside this txn: returning Err rolls it back.
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
                    quota::ReservationInsert {
                        reservation_key: &reservation_key,
                        kind: resource_kind,
                        amount,
                        ttl_secs,
                        job_id,
                        observed_at,
                    },
                )
                .await?
                else {
                    quota::find_by_key(txn, &ctx, &reservation_key)
                        .await?
                        .ok_or(QuotaError::ReservationNotFound)?;
                    let Some(active) = quota::find_active_matching_by_key(
                        txn,
                        &ctx,
                        &reservation_key,
                        resource_kind,
                        amount,
                        job_id,
                        observed_at,
                    )
                    .await?
                    else {
                        return Err(QuotaError::ReservationConflict);
                    };
                    let snapshot = current_snapshot(txn, &ctx, resource_kind, observed_at).await?;
                    return Ok(QuotaReservationOutcome {
                        reservation: active,
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
) -> Result<QuotaSettlement, QuotaError> {
    validate_reservation_key(reservation_key)?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let reservation_key = reservation_key.to_string();
        move |txn| {
            Box::pin(async move {
                let kind = quota::kind_by_key(txn, &ctx, &reservation_key)
                    .await
                    .map_err(map_reservation_error)?;
                quota::lock_admission(txn, &ctx, kind).await?;
                let observed_at = quota::fresh_clock_timestamp(txn).await?;
                if let Some(finalized) =
                    quota::finalize_reserved_by_key(txn, &ctx, &reservation_key, observed_at)
                        .await?
                {
                    if finalized.resource_kind != kind {
                        return Err(QuotaError::ReservationResourceMismatch);
                    }
                    if finalized.resource_kind.counter_key().is_some() {
                        let period =
                            quota::current_period(txn, finalized.resource_kind, observed_at)
                                .await?;
                        let current = quota::lock_committed_counter(
                            txn,
                            &ctx,
                            finalized.resource_kind,
                            period,
                        )
                        .await?
                        .unwrap_or(0);
                        let value = current
                            .checked_add(finalized.amount)
                            .ok_or(QuotaError::ArithmeticOverflow)?;
                        quota::upsert_counter_value(
                            txn,
                            &ctx,
                            finalized.resource_kind,
                            period,
                            value,
                        )
                        .await?;
                    }
                    return Ok(QuotaSettlement::Finalized(finalized));
                }

                if let Some(expired) =
                    quota::expire_reserved_by_key_if_due(txn, &ctx, &reservation_key, observed_at)
                        .await?
                {
                    return Ok(QuotaSettlement::Expired(expired));
                }

                let reservation = quota::get_by_key_for_update(txn, &ctx, &reservation_key)
                    .await
                    .map_err(map_reservation_error)?;
                match reservation.status {
                    ReservationStatus::Finalized => {
                        Ok(QuotaSettlement::AlreadyFinalized(reservation))
                    }
                    ReservationStatus::Refunded => {
                        Ok(QuotaSettlement::RefundedCannotFinalize(reservation))
                    }
                    ReservationStatus::Expired => Ok(QuotaSettlement::Expired(reservation)),
                    ReservationStatus::Reserved => Err(QuotaError::ReservationConflict),
                }
            })
        }
    })
    .await
}

pub async fn refund(
    db_pool: &Pool,
    ctx: &OrgContext,
    reservation_key: &str,
) -> Result<QuotaSettlement, QuotaError> {
    let result = refund_inner(db_pool, ctx, reservation_key).await;
    match &result {
        Ok(settlement) => crate::telemetry::record_quota(
            "refund",
            settlement.reservation().resource_kind.as_str(),
        ),
        Err(_) => crate::telemetry::record_quota("error", "unknown"),
    }
    result
}

async fn refund_inner(
    db_pool: &Pool,
    ctx: &OrgContext,
    reservation_key: &str,
) -> Result<QuotaSettlement, QuotaError> {
    validate_reservation_key(reservation_key)?;
    pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let reservation_key = reservation_key.to_string();
        move |txn| {
            Box::pin(async move {
                let kind = quota::kind_by_key(txn, &ctx, &reservation_key)
                    .await
                    .map_err(map_reservation_error)?;
                quota::lock_admission(txn, &ctx, kind).await?;
                let observed_at = quota::fresh_clock_timestamp(txn).await?;
                if let Some(refunded) =
                    quota::refund_reserved_by_key(txn, &ctx, &reservation_key, observed_at).await?
                {
                    return Ok(QuotaSettlement::Refunded(refunded));
                }
                if let Some(expired) =
                    quota::expire_reserved_by_key_if_due(txn, &ctx, &reservation_key, observed_at)
                        .await?
                {
                    return Ok(QuotaSettlement::Expired(expired));
                }
                let reservation = quota::get_by_key_for_update(txn, &ctx, &reservation_key)
                    .await
                    .map_err(map_reservation_error)?;
                match reservation.status {
                    ReservationStatus::Refunded => {
                        Ok(QuotaSettlement::AlreadyRefunded(reservation))
                    }
                    ReservationStatus::Expired => Ok(QuotaSettlement::Expired(reservation)),
                    ReservationStatus::Finalized => {
                        Ok(QuotaSettlement::FinalizedCannotRefund(reservation))
                    }
                    ReservationStatus::Reserved => Err(QuotaError::ReservationConflict),
                }
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
            Box::pin(async move {
                let observed_at = quota::fresh_clock_timestamp(txn).await?;
                quota::expire_reserved_batch(txn, &ctx, batch_size, observed_at).await
            })
        }
    })
    .await
    .map_err(Into::into)
}

pub async fn sweep_expired_all_orgs(db_pool: &Pool, batch_size: u32) -> Result<u64, QuotaError> {
    let org_ids = quota::list_org_ids_for_sweep(db_pool).await?;
    let mut expired = 0_u64;
    for org_id in org_ids {
        let ctx = OrgContext::try_new(org_id, SWEEP_USER_ID, [] as [&str; 0], [])
            .map_err(|error| QuotaError::Database(DbError::Config(error.to_string())))?;
        expired = expired
            .checked_add(expire_reserved(db_pool, &ctx, batch_size).await?)
            .ok_or(QuotaError::ArithmeticOverflow)?;
    }
    Ok(expired)
}

const SWEEP_USER_ID: Uuid = Uuid::from_u128(0x00000000000040008000000000000001);

pub async fn reserve_upload(
    db_pool: &Pool,
    ctx: &OrgContext,
    reservation_key: &str,
    size_bytes: u64,
    ttl: Duration,
) -> Result<UploadQuotaReservation, QuotaError> {
    validate_reservation_key(reservation_key)?;
    let storage_amount = checked_amount(size_bytes)?;
    let ttl_secs = checked_ttl_secs(ttl)?;
    let storage_key = format!("upload.storage.{reservation_key}");
    let document_key = format!("upload.documents.{reservation_key}");
    let result = pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let storage_key = storage_key.clone();
        let document_key = document_key.clone();
        move |txn| {
            Box::pin(async move {
                lock_resource_kinds(
                    txn,
                    &ctx,
                    &[ResourceKind::Documents, ResourceKind::StorageBytes],
                )
                .await?;
                let observed_at = quota::fresh_clock_timestamp(txn).await?;
                let document = reserve_spec_in_txn(
                    txn,
                    &ctx,
                    &document_key,
                    ResourceKind::Documents,
                    1,
                    ttl_secs,
                    observed_at,
                )
                .await?;
                let storage = reserve_spec_in_txn(
                    txn,
                    &ctx,
                    &storage_key,
                    ResourceKind::StorageBytes,
                    storage_amount,
                    ttl_secs,
                    observed_at,
                )
                .await?;
                Ok(UploadQuotaReservation { storage, document })
            })
        }
    })
    .await;
    match &result {
        Ok(outcome) => {
            if outcome.document.created {
                crate::telemetry::record_quota("reserve", ResourceKind::Documents.as_str());
            }
            if outcome.storage.created {
                crate::telemetry::record_quota("reserve", ResourceKind::StorageBytes.as_str());
            }
        }
        Err(QuotaError::QuotaExceeded(denial)) => {
            persist_quota_deny_audit(db_pool, ctx, denial.resource_kind).await;
            crate::telemetry::record_quota("deny", denial.resource_kind.as_str());
        }
        Err(_) => crate::telemetry::record_quota("error", "unknown"),
    }
    result
}

async fn persist_quota_deny_audit(db_pool: &Pool, ctx: &OrgContext, resource_kind: ResourceKind) {
    let request_id = crate::services::audit::request_id_from_correlation();
    let _ = crate::services::audit::write_deny_durable(
        db_pool,
        ctx.org_id(),
        Some(ctx.user_id()),
        crate::services::audit::AuditAction::QuotaDeny,
        crate::services::audit::AuditResource::Quota,
        None,
        &request_id,
        serde_json::json!({
            "reason": "quota_exceeded",
            "resource_kind": resource_kind.as_str(),
        }),
    )
    .await;
}

pub async fn finalize_upload(
    db_pool: &Pool,
    ctx: &OrgContext,
    reservation_key: &str,
) -> Result<UploadQuotaSettlement, QuotaError> {
    validate_reservation_key(reservation_key)?;
    let storage_key = format!("upload.storage.{reservation_key}");
    let document_key = format!("upload.documents.{reservation_key}");
    let settlement = pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let storage_key = storage_key.clone();
        let document_key = document_key.clone();
        move |txn| {
            Box::pin(async move {
                lock_resource_kinds(
                    txn,
                    &ctx,
                    &[ResourceKind::Documents, ResourceKind::StorageBytes],
                )
                .await?;
                let observed_at = quota::fresh_clock_timestamp(txn).await?;
                let document_preview = preview_locked(
                    txn,
                    &ctx,
                    &document_key,
                    ResourceKind::Documents,
                    observed_at,
                )
                .await?;
                let storage_preview = preview_locked(
                    txn,
                    &ctx,
                    &storage_key,
                    ResourceKind::StorageBytes,
                    observed_at,
                )
                .await?;
                let (document, storage) = match (&document_preview, &storage_preview) {
                    (QuotaSettlement::Reserved(_), QuotaSettlement::Reserved(_)) => {
                        let document = finalize_locked(
                            txn,
                            &ctx,
                            &document_key,
                            ResourceKind::Documents,
                            observed_at,
                        )
                        .await?;
                        let storage = finalize_locked(
                            txn,
                            &ctx,
                            &storage_key,
                            ResourceKind::StorageBytes,
                            observed_at,
                        )
                        .await?;
                        (document, storage)
                    }
                    (
                        QuotaSettlement::AlreadyFinalized(_),
                        QuotaSettlement::AlreadyFinalized(_),
                    ) => (document_preview, storage_preview),
                    _ => (document_preview, storage_preview),
                };
                let storage_quota =
                    current_snapshot(txn, &ctx, ResourceKind::StorageBytes, observed_at).await?;
                Ok::<UploadQuotaSettlement, QuotaError>(UploadQuotaSettlement {
                    storage,
                    document,
                    storage_quota,
                })
            })
        }
    })
    .await?;
    if settlement.storage.is_finalize_success() && settlement.document.is_finalize_success() {
        Ok(settlement)
    } else {
        Err(
            finalize_settlement_error(&settlement.storage).unwrap_or_else(|| {
                finalize_settlement_error(&settlement.document)
                    .unwrap_or(QuotaError::ReservationConflict)
            }),
        )
    }
}

pub async fn refund_upload(
    db_pool: &Pool,
    ctx: &OrgContext,
    reservation_key: &str,
) -> Result<UploadQuotaSettlement, QuotaError> {
    validate_reservation_key(reservation_key)?;
    let storage_key = format!("upload.storage.{reservation_key}");
    let document_key = format!("upload.documents.{reservation_key}");
    let settlement = pool::with_org_txn_typed(db_pool, ctx, {
        let ctx = ctx.clone();
        let storage_key = storage_key.clone();
        let document_key = document_key.clone();
        move |txn| {
            Box::pin(async move {
                lock_resource_kinds(
                    txn,
                    &ctx,
                    &[ResourceKind::Documents, ResourceKind::StorageBytes],
                )
                .await?;
                let observed_at = quota::fresh_clock_timestamp(txn).await?;
                let document_preview = preview_locked(
                    txn,
                    &ctx,
                    &document_key,
                    ResourceKind::Documents,
                    observed_at,
                )
                .await?;
                let storage_preview = preview_locked(
                    txn,
                    &ctx,
                    &storage_key,
                    ResourceKind::StorageBytes,
                    observed_at,
                )
                .await?;
                let (document, storage) =
                    if matches!(&document_preview, QuotaSettlement::AlreadyFinalized(_))
                        || matches!(&storage_preview, QuotaSettlement::AlreadyFinalized(_))
                    {
                        (document_preview, storage_preview)
                    } else {
                        let document = if matches!(&document_preview, QuotaSettlement::Reserved(_))
                        {
                            refund_locked(
                                txn,
                                &ctx,
                                &document_key,
                                ResourceKind::Documents,
                                observed_at,
                            )
                            .await?
                        } else {
                            document_preview
                        };
                        let storage = if matches!(&storage_preview, QuotaSettlement::Reserved(_)) {
                            refund_locked(
                                txn,
                                &ctx,
                                &storage_key,
                                ResourceKind::StorageBytes,
                                observed_at,
                            )
                            .await?
                        } else {
                            storage_preview
                        };
                        (document, storage)
                    };
                let storage_quota =
                    current_snapshot(txn, &ctx, ResourceKind::StorageBytes, observed_at).await?;
                Ok::<UploadQuotaSettlement, QuotaError>(UploadQuotaSettlement {
                    storage,
                    document,
                    storage_quota,
                })
            })
        }
    })
    .await?;
    if settlement.storage.is_refund_success() && settlement.document.is_refund_success() {
        Ok(settlement)
    } else {
        Err(
            refund_settlement_error(&settlement.storage).unwrap_or_else(|| {
                refund_settlement_error(&settlement.document)
                    .unwrap_or(QuotaError::ReservationConflict)
            }),
        )
    }
}

async fn lock_resource_kinds(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    kinds: &[ResourceKind],
) -> Result<(), QuotaError> {
    let mut ordered = kinds.to_vec();
    ordered.sort_by_key(|kind| kind.as_str());
    ordered.dedup_by_key(|kind| kind.as_str());
    for kind in ordered {
        quota::lock_admission(txn, ctx, kind).await?;
    }
    Ok(())
}

async fn reserve_spec_in_txn(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    reservation_key: &str,
    resource_kind: ResourceKind,
    amount: i64,
    ttl_secs: i64,
    observed_at: DateTime<Utc>,
) -> Result<QuotaReservationOutcome, QuotaError> {
    validate_reservation_key(reservation_key)?;
    if amount <= 0 {
        return Err(QuotaError::InvalidAmount);
    }
    if quota::find_by_key(txn, ctx, reservation_key)
        .await?
        .is_some()
    {
        let Some(active) = quota::find_active_matching_by_key(
            txn,
            ctx,
            reservation_key,
            resource_kind,
            amount,
            None,
            observed_at,
        )
        .await?
        else {
            return Err(QuotaError::ReservationConflict);
        };
        let snapshot = current_snapshot(txn, ctx, resource_kind, observed_at).await?;
        return Ok(QuotaReservationOutcome {
            reservation: active,
            quota: snapshot,
            created: false,
        });
    }

    let usage = quota::usage(txn, ctx, resource_kind, observed_at)
        .await
        .map_err(map_quota_config_error)?;
    let available = remaining(usage.limit, usage.committed, usage.active_reserved)?;
    if amount > available {
        // Caller must persist deny audit after the enclosing txn rolls back.
        return Err(QuotaError::QuotaExceeded(QuotaDenial {
            resource_kind,
            limit: usage.limit,
            committed: usage.committed,
            active_reserved: usage.active_reserved,
            requested: amount,
            remaining: available,
            retry_after_secs: retry_after_for(resource_kind),
        }));
    }

    let Some(inserted) = quota::insert_reserved(
        txn,
        ctx,
        quota::ReservationInsert {
            reservation_key,
            kind: resource_kind,
            amount,
            ttl_secs,
            job_id: None,
            observed_at,
        },
    )
    .await?
    else {
        let Some(active) = quota::find_active_matching_by_key(
            txn,
            ctx,
            reservation_key,
            resource_kind,
            amount,
            None,
            observed_at,
        )
        .await?
        else {
            return Err(QuotaError::ReservationConflict);
        };
        let snapshot = current_snapshot(txn, ctx, resource_kind, observed_at).await?;
        return Ok(QuotaReservationOutcome {
            reservation: active,
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
            remaining: available
                .checked_sub(amount)
                .ok_or(QuotaError::ArithmeticOverflow)?,
        },
        created: true,
    })
}

async fn finalize_locked(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    reservation_key: &str,
    expected_kind: ResourceKind,
    observed_at: DateTime<Utc>,
) -> Result<QuotaSettlement, QuotaError> {
    let kind = quota::kind_by_key(txn, ctx, reservation_key)
        .await
        .map_err(map_reservation_error)?;
    if kind != expected_kind {
        return Err(QuotaError::ReservationResourceMismatch);
    }
    if let Some(finalized) =
        quota::finalize_reserved_by_key(txn, ctx, reservation_key, observed_at).await?
    {
        add_committed_counter(txn, ctx, &finalized, observed_at).await?;
        return Ok(QuotaSettlement::Finalized(finalized));
    }
    if let Some(expired) =
        quota::expire_reserved_by_key_if_due(txn, ctx, reservation_key, observed_at).await?
    {
        return Ok(QuotaSettlement::Expired(expired));
    }
    let reservation = quota::get_by_key_for_update(txn, ctx, reservation_key)
        .await
        .map_err(map_reservation_error)?;
    match reservation.status {
        ReservationStatus::Finalized => Ok(QuotaSettlement::AlreadyFinalized(reservation)),
        ReservationStatus::Refunded => Ok(QuotaSettlement::RefundedCannotFinalize(reservation)),
        ReservationStatus::Expired => Ok(QuotaSettlement::Expired(reservation)),
        ReservationStatus::Reserved => Err(QuotaError::ReservationConflict),
    }
}

async fn refund_locked(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    reservation_key: &str,
    expected_kind: ResourceKind,
    observed_at: DateTime<Utc>,
) -> Result<QuotaSettlement, QuotaError> {
    let kind = quota::kind_by_key(txn, ctx, reservation_key)
        .await
        .map_err(map_reservation_error)?;
    if kind != expected_kind {
        return Err(QuotaError::ReservationResourceMismatch);
    }
    if let Some(refunded) =
        quota::refund_reserved_by_key(txn, ctx, reservation_key, observed_at).await?
    {
        return Ok(QuotaSettlement::Refunded(refunded));
    }
    if let Some(expired) =
        quota::expire_reserved_by_key_if_due(txn, ctx, reservation_key, observed_at).await?
    {
        return Ok(QuotaSettlement::Expired(expired));
    }
    let reservation = quota::get_by_key_for_update(txn, ctx, reservation_key)
        .await
        .map_err(map_reservation_error)?;
    match reservation.status {
        ReservationStatus::Refunded => Ok(QuotaSettlement::AlreadyRefunded(reservation)),
        ReservationStatus::Expired => Ok(QuotaSettlement::Expired(reservation)),
        ReservationStatus::Finalized => Ok(QuotaSettlement::FinalizedCannotRefund(reservation)),
        ReservationStatus::Reserved => Err(QuotaError::ReservationConflict),
    }
}

async fn preview_locked(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    reservation_key: &str,
    expected_kind: ResourceKind,
    observed_at: DateTime<Utc>,
) -> Result<QuotaSettlement, QuotaError> {
    let kind = quota::kind_by_key(txn, ctx, reservation_key)
        .await
        .map_err(map_reservation_error)?;
    if kind != expected_kind {
        return Err(QuotaError::ReservationResourceMismatch);
    }
    if let Some(expired) =
        quota::expire_reserved_by_key_if_due(txn, ctx, reservation_key, observed_at).await?
    {
        return Ok(QuotaSettlement::Expired(expired));
    }
    let reservation = quota::get_by_key_for_update(txn, ctx, reservation_key)
        .await
        .map_err(map_reservation_error)?;
    match reservation.status {
        ReservationStatus::Reserved => Ok(QuotaSettlement::Reserved(reservation)),
        ReservationStatus::Finalized => Ok(QuotaSettlement::AlreadyFinalized(reservation)),
        ReservationStatus::Refunded => Ok(QuotaSettlement::AlreadyRefunded(reservation)),
        ReservationStatus::Expired => Ok(QuotaSettlement::Expired(reservation)),
    }
}

async fn add_committed_counter(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    reservation: &QuotaReservation,
    observed_at: DateTime<Utc>,
) -> Result<(), QuotaError> {
    if reservation.resource_kind.counter_key().is_some() {
        let period = quota::current_period(txn, reservation.resource_kind, observed_at).await?;
        let current = quota::lock_committed_counter(txn, ctx, reservation.resource_kind, period)
            .await?
            .unwrap_or(0);
        let value = current
            .checked_add(reservation.amount)
            .ok_or(QuotaError::ArithmeticOverflow)?;
        quota::upsert_counter_value(txn, ctx, reservation.resource_kind, period, value).await?;
    }
    Ok(())
}

async fn current_snapshot(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    resource_kind: ResourceKind,
    observed_at: DateTime<Utc>,
) -> Result<QuotaSnapshot, QuotaError> {
    let usage = quota::usage(txn, ctx, resource_kind, observed_at)
        .await
        .map_err(map_quota_config_error)?;
    Ok(QuotaSnapshot {
        resource_kind,
        limit: usage.limit,
        committed: usage.committed,
        active_reserved: usage.active_reserved,
        remaining: remaining(usage.limit, usage.committed, usage.active_reserved)?,
    })
}

fn finalize_settlement_error(settlement: &QuotaSettlement) -> Option<QuotaError> {
    match settlement {
        QuotaSettlement::Finalized(_) | QuotaSettlement::AlreadyFinalized(_) => None,
        QuotaSettlement::Reserved(_) => None,
        QuotaSettlement::Expired(_) => Some(QuotaError::ExpiredCannotFinalize),
        QuotaSettlement::Refunded(_)
        | QuotaSettlement::AlreadyRefunded(_)
        | QuotaSettlement::RefundedCannotFinalize(_) => Some(QuotaError::RefundedCannotFinalize),
        QuotaSettlement::FinalizedCannotRefund(_) => Some(QuotaError::ReservationConflict),
    }
}

fn refund_settlement_error(settlement: &QuotaSettlement) -> Option<QuotaError> {
    match settlement {
        QuotaSettlement::Refunded(_)
        | QuotaSettlement::AlreadyRefunded(_)
        | QuotaSettlement::Expired(_) => None,
        QuotaSettlement::Reserved(_) => None,
        QuotaSettlement::Finalized(_)
        | QuotaSettlement::AlreadyFinalized(_)
        | QuotaSettlement::FinalizedCannotRefund(_) => Some(QuotaError::FinalizedCannotRefund),
        QuotaSettlement::RefundedCannotFinalize(_) => Some(QuotaError::ReservationConflict),
    }
}

fn map_quota_config_error(error: DbError) -> QuotaError {
    match error {
        DbError::NotFound => QuotaError::NotConfigured,
        other => QuotaError::Database(other),
    }
}

fn map_reservation_error(error: DbError) -> QuotaError {
    match error {
        DbError::NotFound => QuotaError::ReservationNotFound,
        other => QuotaError::Database(other),
    }
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
