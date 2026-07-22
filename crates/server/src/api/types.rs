//! Shared wire types for collection / document / job REST responses.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::pagination::PageInfo;

/// Versioned SSE envelope. Sequence is monotonic per stream and supports reconnect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SseEnvelope {
    pub version: u16,
    pub sequence: u64,
    pub event: String,
    pub request_id: String,
    pub data: serde_json::Value,
}

/// Generic list envelope used by `/api/v1` collection/document/job routes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResponse<T> {
    pub items: Vec<T>,
    pub page_info: PageInfo,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionResponse {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub visibility: String,
    pub owner_user_id: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentResponse {
    pub id: Uuid,
    pub collection_id: Uuid,
    pub title: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_version_id: Option<Uuid>,
    pub created_by_user_id: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentVersionResponse {
    pub id: Uuid,
    pub document_id: Uuid,
    pub version_number: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_version_id: Option<Uuid>,
    pub publication_state: String,
    pub is_current: bool,
    pub content_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byte_size: Option<i64>,
    pub effective_from: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_to: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_summary: Option<String>,
    pub created_by_user_id: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionDiffResponse {
    pub document_id: Uuid,
    pub left_version_id: Uuid,
    pub right_version_id: Uuid,
    pub left_version_number: i32,
    pub right_version_number: i32,
    pub content_sha256_changed: bool,
    pub publication_state_changed: bool,
    pub current_flag_changed: bool,
    pub change_summary_changed: bool,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobResponse {
    pub id: Uuid,
    pub job_type: String,
    pub status: String,
    pub attempts: i32,
    pub max_attempts: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_id: Option<Uuid>,
    pub available_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReindexResponse {
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub job_id: Uuid,
    pub created: bool,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictResponse {
    pub id: Uuid,
    pub status: String,
    pub severity: String,
    pub conflict_type: String,
    pub claim_a_id: Uuid,
    pub claim_b_id: Uuid,
    pub first_detected_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_detected_version_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution_note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution_version_a_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution_version_b_id: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictEvidenceResponse {
    pub id: Uuid,
    pub conflict_id: Uuid,
    pub claim_id: Uuid,
    pub evidence_role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub citation_quote: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
