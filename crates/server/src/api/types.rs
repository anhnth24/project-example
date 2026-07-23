//! Shared response DTOs for Phase 1B REST routes.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::api::PageInfo;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionDto {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub visibility: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentDto {
    pub id: Uuid,
    pub collection_id: Uuid,
    pub title: String,
    pub state: String,
    pub current_version_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentVersionDto {
    pub id: Uuid,
    pub document_id: Uuid,
    pub version_number: i32,
    pub is_current: bool,
    /// Original uploaded/source object SHA-256.
    pub source_content_sha256: String,
    pub effective_from: DateTime<Utc>,
    pub effective_to: Option<DateTime<Utc>>,
    pub change_summary: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobDto {
    pub id: Uuid,
    pub job_type: String,
    pub status: String,
    pub attempts: i32,
    pub document_id: Option<Uuid>,
    pub version_id: Option<Uuid>,
    /// Server-minted request correlation id from the job payload (O01), when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page: PageInfo,
}
