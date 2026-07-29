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
    /// P2-18 — nullable: absent/`null` for a collection with no project.
    pub project_id: Option<Uuid>,
    /// Joined display name for `project_id` (nullable together with it) so
    /// the Library nav can group by project without a second round trip —
    /// same "never show the raw id" convention `MembershipDto`'s
    /// email/display_name join already follows.
    pub project_name: Option<String>,
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

/// Document Graph MVP (P2-17) — one node per caller-visible document.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNodeDto {
    pub id: Uuid,
    pub title: String,
    pub collection_id: Uuid,
    pub collection_name: String,
    /// `documents.state` (uploaded/converting/.../indexed/failed).
    pub status: String,
    pub degree: i64,
}

/// P2-17 graph edge. `kind` is `conflict` | `co_citation` | `similarity`;
/// `weight` is `0..1` (`conflict`/`co_citation` via
/// `services::graph::saturating_weight`, `similarity` — opt-in on a
/// configured vector index + embedder — via
/// `services::graph::normalize_similarity_weight`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdgeDto {
    pub source: Uuid,
    pub target: Uuid,
    pub kind: String,
    pub weight: f64,
}

/// P2-17 community: one connected component of the (pruned) graph.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphCommunityDto {
    pub id: String,
    pub label: String,
    pub node_ids: Vec<Uuid>,
    pub size: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphResponseDto {
    pub nodes: Vec<GraphNodeDto>,
    pub edges: Vec<GraphEdgeDto>,
    pub communities: Vec<GraphCommunityDto>,
    pub request_id: String,
}

/// 1C-11 audit log read entry. Field names follow the existing telemetry
/// envelope vocabulary (`occurred_at`/`actor_id`/`target_type`/`target_id`,
/// see `telemetry::AuditEvent`) rather than the raw `audit_log` column names.
/// `metadata` is passed through verbatim — it was already reduced to the
/// per-action scalar allowlist at write time
/// (`services::audit::sanitize_for_action`), so no extra filtering happens
/// here; this endpoint must never add fields beyond that allowlist.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntryDto {
    pub id: Uuid,
    pub seq: i64,
    pub actor_id: Option<Uuid>,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub outcome: String,
    pub metadata: serde_json::Value,
    pub request_id: Option<String>,
    pub occurred_at: DateTime<Utc>,
}
