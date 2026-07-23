//! Typed mirrors of the Markhand Web PostgreSQL schema (P1B-F03).
//!
//! Column coverage is intentionally complete so drift is reviewable. Repository
//! modules in later issues own query mapping. Secret-bearing fields never appear
//! in Debug.
//!
//! Timestamps use `chrono::DateTime<Utc>` (tokio-postgres `with-chrono-0_4`).
//! Money/number claim values use `rust_decimal::Decimal` (exact `numeric`).

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

/// Redacted string for password/token hashes so Debug cannot leak secrets.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretHash(String);

impl SecretHash {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Org {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<SecretHash>,
    pub disabled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipRole {
    Owner,
    Admin,
    Editor,
    Viewer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgMembership {
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub role: MembershipRole,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permission {
    pub id: Uuid,
    pub code: String,
    pub description: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Role {
    pub id: Uuid,
    pub org_id: Uuid,
    pub code: String,
    pub name: String,
    pub is_system: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RolePermission {
    pub org_id: Uuid,
    pub role_id: Uuid,
    pub permission_id: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupMembership {
    pub org_id: Uuid,
    pub group_id: Uuid,
    pub user_id: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshToken {
    pub id: Uuid,
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub family_id: Uuid,
    pub token_hash: SecretHash,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub replaced_by_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgInvite {
    pub id: Uuid,
    pub org_id: Uuid,
    pub email: String,
    pub role: MembershipRole,
    pub token_hash: SecretHash,
    pub invited_by_user_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub accepted_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionVisibility {
    Private,
    Org,
    Groups,
}

impl CollectionVisibility {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Org => "org",
            Self::Groups => "groups",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "private" => Ok(Self::Private),
            "org" => Ok(Self::Org),
            "groups" => Ok(Self::Groups),
            other => Err(format!("unknown collection visibility: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub owner_user_id: Uuid,
    pub visibility: CollectionVisibility,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessLevel {
    Read,
    Write,
    Admin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionUserAccess {
    pub id: Uuid,
    pub org_id: Uuid,
    pub collection_id: Uuid,
    pub user_id: Uuid,
    pub access_level: AccessLevel,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionGroupAccess {
    pub id: Uuid,
    pub org_id: Uuid,
    pub collection_id: Uuid,
    pub group_id: Uuid,
    pub access_level: AccessLevel,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionRoleAccess {
    pub id: Uuid,
    pub org_id: Uuid,
    pub collection_id: Uuid,
    pub role_id: Uuid,
    pub access_level: AccessLevel,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentState {
    Uploaded,
    Converting,
    Converted,
    Indexing,
    Indexed,
    Failed,
    Tombstoned,
    Purged,
}

impl DocumentState {
    /// PostgreSQL `documents.state` text value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Uploaded => "uploaded",
            Self::Converting => "converting",
            Self::Converted => "converted",
            Self::Indexing => "indexing",
            Self::Indexed => "indexed",
            Self::Failed => "failed",
            Self::Tombstoned => "tombstoned",
            Self::Purged => "purged",
        }
    }

    /// Parses a DB state string; unknown values are rejected.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "uploaded" => Ok(Self::Uploaded),
            "converting" => Ok(Self::Converting),
            "converted" => Ok(Self::Converted),
            "indexing" => Ok(Self::Indexing),
            "indexed" => Ok(Self::Indexed),
            "failed" => Ok(Self::Failed),
            "tombstoned" => Ok(Self::Tombstoned),
            "purged" => Ok(Self::Purged),
            other => Err(format!("unknown document state: {other}")),
        }
    }
}

impl std::fmt::Display for DocumentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub id: Uuid,
    pub org_id: Uuid,
    pub collection_id: Uuid,
    pub title: String,
    pub state: DocumentState,
    pub current_version_id: Option<Uuid>,
    pub created_by_user_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationState {
    Draft,
    Published,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentVersion {
    pub id: Uuid,
    pub org_id: Uuid,
    pub document_id: Uuid,
    pub version_number: i32,
    pub parent_version_id: Option<Uuid>,
    pub publication_state: PublicationState,
    pub is_current: bool,
    pub content_sha256: String,
    pub original_object_key: String,
    pub markdown_object_key: Option<String>,
    pub source_filename: Option<String>,
    pub source_content_type: Option<String>,
    pub byte_size: Option<i64>,
    pub effective_from: DateTime<Utc>,
    pub effective_to: Option<DateTime<Utc>>,
    pub change_summary: Option<String>,
    pub created_by_user_id: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Markdown,
    Preview,
    Thumbnail,
    ExtractedText,
    Other,
}

impl ArtifactKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Preview => "preview",
            Self::Thumbnail => "thumbnail",
            Self::ExtractedText => "extracted_text",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "markdown" => Ok(Self::Markdown),
            "preview" => Ok(Self::Preview),
            "thumbnail" => Ok(Self::Thumbnail),
            "extracted_text" => Ok(Self::ExtractedText),
            "other" => Ok(Self::Other),
            other => Err(format!("unknown artifact kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedArtifact {
    pub id: Uuid,
    pub org_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub artifact_kind: ArtifactKind,
    pub object_key: String,
    pub content_sha256: String,
    pub content_type: Option<String>,
    pub byte_size: Option<i64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    pub id: Uuid,
    pub org_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub ordinal: i32,
    pub heading_path: Vec<String>,
    pub body: String,
    pub body_text_version: String,
    pub chunk_identity_sha256: String,
    pub index_metadata_id: Uuid,
    pub index_signature: String,
    pub page: Option<i32>,
    pub slide: Option<i32>,
    pub sheet: Option<String>,
    pub span_start: Option<i32>,
    pub span_end: Option<i32>,
    /// PostgreSQL `tsvector` is opaque at the Rust boundary; stored as text when selected.
    pub tsv: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimValueType {
    Number,
    Enum,
    Date,
    Boolean,
    Text,
    Money,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    pub id: Uuid,
    pub org_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub chunk_id: Option<Uuid>,
    pub claim_key: String,
    pub subject: String,
    pub predicate: String,
    pub value_type: ClaimValueType,
    pub value_number: Option<Decimal>,
    pub value_text: Option<String>,
    pub value_boolean: Option<bool>,
    pub value_date: Option<NaiveDate>,
    pub value_money: Option<Decimal>,
    pub unit: Option<String>,
    pub scope: String,
    pub effective_from: DateTime<Utc>,
    pub effective_to: Option<DateTime<Utc>>,
    pub citation_quote: Option<String>,
    pub citation_span_start: Option<i32>,
    pub citation_span_end: Option<i32>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictStatus {
    Open,
    Resolved,
    AcceptedException,
    FalsePositive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictSeverity {
    Info,
    Warning,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictType {
    Numeric,
    Enum,
    Date,
    Limit,
    MustVsMustNot,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub id: Uuid,
    pub org_id: Uuid,
    pub status: ConflictStatus,
    pub severity: ConflictSeverity,
    pub conflict_type: ConflictType,
    pub claim_a_id: Uuid,
    pub claim_b_id: Uuid,
    pub first_detected_at: DateTime<Utc>,
    pub first_detected_version_id: Option<Uuid>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolution_note: Option<String>,
    pub resolution_version_a_id: Option<Uuid>,
    pub resolution_version_b_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceRole {
    Left,
    Right,
    ResolutionLeft,
    ResolutionRight,
    Supporting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictEvidence {
    pub id: Uuid,
    pub org_id: Uuid,
    pub conflict_id: Uuid,
    pub claim_id: Uuid,
    pub evidence_role: EvidenceRole,
    pub citation_quote: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobType {
    Convert,
    Index,
    Delete,
    Reconcile,
    EmbeddingBatch,
    LifecycleRefresh,
}

impl JobType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Convert => "convert",
            Self::Index => "index",
            Self::Delete => "delete",
            Self::Reconcile => "reconcile",
            Self::EmbeddingBatch => "embedding_batch",
            Self::LifecycleRefresh => "lifecycle_refresh",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "convert" => Ok(Self::Convert),
            "index" => Ok(Self::Index),
            "delete" => Ok(Self::Delete),
            "reconcile" => Ok(Self::Reconcile),
            "embedding_batch" => Ok(Self::EmbeddingBatch),
            "lifecycle_refresh" => Ok(Self::LifecycleRefresh),
            other => Err(format!("unknown job type: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Pending,
    Leased,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    DeadLetter,
}

impl JobStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Leased => "leased",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::DeadLetter => "dead_letter",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "pending" => Ok(Self::Pending),
            "leased" => Ok(Self::Leased),
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "dead_letter" => Ok(Self::DeadLetter),
            other => Err(format!("unknown job status: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: Uuid,
    pub org_id: Uuid,
    pub job_type: JobType,
    pub status: JobStatus,
    pub payload_version: i32,
    pub payload: JsonValue,
    pub attempts: i32,
    pub max_attempts: i32,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub heartbeat_at: Option<DateTime<Utc>>,
    pub checkpoint: Option<JsonValue>,
    pub idempotency_key: String,
    pub document_id: Option<Uuid>,
    pub version_id: Option<Uuid>,
    pub available_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutboxEvent {
    pub id: Uuid,
    pub org_id: Uuid,
    pub event_type: String,
    pub payload_version: i32,
    pub payload: JsonValue,
    pub idempotency_key: String,
    pub job_id: Option<Uuid>,
    pub published_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventLogEntry {
    pub id: Uuid,
    pub org_id: Uuid,
    pub sequence_no: i64,
    pub event_type: String,
    pub payload_version: i32,
    pub payload: JsonValue,
    pub job_id: Option<Uuid>,
    pub document_id: Option<Uuid>,
    pub version_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgQuota {
    pub org_id: Uuid,
    pub max_storage_bytes: i64,
    pub max_documents: i32,
    pub max_concurrent_jobs: i32,
    pub max_monthly_tokens: i64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageCounter {
    pub id: Uuid,
    pub org_id: Uuid,
    pub counter_key: String,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub value: i64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    StorageBytes,
    Documents,
    ConcurrentJobs,
    Tokens,
}

impl ResourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StorageBytes => "storage_bytes",
            Self::Documents => "documents",
            Self::ConcurrentJobs => "concurrent_jobs",
            Self::Tokens => "tokens",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "storage_bytes" => Ok(Self::StorageBytes),
            "documents" => Ok(Self::Documents),
            "concurrent_jobs" => Ok(Self::ConcurrentJobs),
            "tokens" => Ok(Self::Tokens),
            other => Err(format!("unknown resource kind: {other}")),
        }
    }

    pub const fn counter_key(self) -> Option<&'static str> {
        match self {
            Self::StorageBytes => Some("storage_bytes"),
            Self::Documents => Some("documents"),
            Self::Tokens => Some("tokens"),
            Self::ConcurrentJobs => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReservationStatus {
    Reserved,
    Finalized,
    Refunded,
    Expired,
}

impl ReservationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Finalized => "finalized",
            Self::Refunded => "refunded",
            Self::Expired => "expired",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "reserved" => Ok(Self::Reserved),
            "finalized" => Ok(Self::Finalized),
            "refunded" => Ok(Self::Refunded),
            "expired" => Ok(Self::Expired),
            other => Err(format!("unknown reservation status: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaReservation {
    pub id: Uuid,
    pub org_id: Uuid,
    pub reservation_key: String,
    pub resource_kind: ResourceKind,
    pub amount: i64,
    pub status: ReservationStatus,
    pub expires_at: DateTime<Utc>,
    pub job_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub settled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Success,
    Deny,
    Error,
    Intent,
}

impl AuditOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Deny => "deny",
            Self::Error => "error",
            Self::Intent => "intent",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "success" => Ok(Self::Success),
            "deny" => Ok(Self::Deny),
            "error" => Ok(Self::Error),
            "intent" => Ok(Self::Intent),
            other => Err(format!("unknown audit outcome: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub id: Uuid,
    pub org_id: Uuid,
    pub seq: i64,
    pub actor_user_id: Option<Uuid>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub outcome: AuditOutcome,
    pub metadata: JsonValue,
    pub request_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingRuntimePath {
    LocalHash,
    LocalNeural,
    GlmCloudInterim,
    VllmLocal,
    ProviderCloud,
}

impl EmbeddingRuntimePath {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalHash => "local-hash",
            Self::LocalNeural => "local-neural",
            Self::GlmCloudInterim => "glm-cloud-interim",
            Self::VllmLocal => "vllm-local",
            Self::ProviderCloud => "provider-cloud",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        // Persisted DB boundary: reuse core allowlist (empty / control / unknown).
        let parsed = fileconv_core::embedding_runtime::parse_embedding_runtime_path(value)
            .map_err(|error| error.to_string())?;
        Ok(match parsed {
            fileconv_core::embedding_runtime::EmbeddingRuntimePath::LocalHash => Self::LocalHash,
            fileconv_core::embedding_runtime::EmbeddingRuntimePath::LocalNeural => {
                Self::LocalNeural
            }
            fileconv_core::embedding_runtime::EmbeddingRuntimePath::GlmCloudInterim => {
                Self::GlmCloudInterim
            }
            fileconv_core::embedding_runtime::EmbeddingRuntimePath::VllmLocal => Self::VllmLocal,
            fileconv_core::embedding_runtime::EmbeddingRuntimePath::ProviderCloud => {
                Self::ProviderCloud
            }
        })
    }
}

/// Lifecycle state for an immutable vector generation (ADR 0011).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexGenerationState {
    Building,
    Shadow,
    Active,
    Draining,
    Retired,
}

impl IndexGenerationState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Building => "building",
            Self::Shadow => "shadow",
            Self::Active => "active",
            Self::Draining => "draining",
            Self::Retired => "retired",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "building" => Ok(Self::Building),
            "shadow" => Ok(Self::Shadow),
            "active" => Ok(Self::Active),
            "draining" => Ok(Self::Draining),
            "retired" => Ok(Self::Retired),
            other => Err(format!("unknown index generation state: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexMetadata {
    pub id: Uuid,
    pub org_id: Uuid,
    pub collection_id: Option<Uuid>,
    pub index_signature_sha256: String,
    pub identity_version: i32,
    pub chunking_version: String,
    pub body_text_version: String,
    pub query_normalization_version: String,
    pub embedding_family: String,
    pub embedding_revision: String,
    pub dimensions: i32,
    pub normalized: bool,
    pub runtime_path: EmbeddingRuntimePath,
    pub generation: i32,
    pub is_active: bool,
    pub state: IndexGenerationState,
    pub created_at: DateTime<Utc>,
}

/// Expected public columns for every modeled/business table (exact-set drift guard).
pub fn expected_table_columns() -> &'static [(&'static str, &'static [&'static str])] {
    &[
        ("orgs", &["id", "slug", "name", "created_at", "updated_at"]),
        (
            "users",
            &[
                "id",
                "email",
                "display_name",
                "disabled_at",
                "created_at",
                "updated_at",
                "password_hash",
            ],
        ),
        (
            "org_memberships",
            &["org_id", "user_id", "role", "created_at"],
        ),
        ("permissions", &["id", "code", "description", "created_at"]),
        (
            "roles",
            &[
                "id",
                "org_id",
                "code",
                "name",
                "is_system",
                "created_at",
                "updated_at",
            ],
        ),
        (
            "role_permissions",
            &["org_id", "role_id", "permission_id", "created_at"],
        ),
        (
            "groups",
            &[
                "id",
                "org_id",
                "name",
                "description",
                "created_at",
                "updated_at",
            ],
        ),
        (
            "group_memberships",
            &["org_id", "group_id", "user_id", "created_at"],
        ),
        (
            "refresh_tokens",
            &[
                "id",
                "org_id",
                "user_id",
                "family_id",
                "token_hash",
                "expires_at",
                "revoked_at",
                "replaced_by_id",
                "created_at",
            ],
        ),
        (
            "org_invites",
            &[
                "id",
                "org_id",
                "email",
                "role",
                "token_hash",
                "invited_by_user_id",
                "expires_at",
                "accepted_at",
                "revoked_at",
                "created_at",
            ],
        ),
        (
            "collections",
            &[
                "id",
                "org_id",
                "name",
                "slug",
                "description",
                "owner_user_id",
                "visibility",
                "created_at",
                "updated_at",
                "deleted_at",
            ],
        ),
        (
            "collection_user_access",
            &[
                "id",
                "org_id",
                "collection_id",
                "user_id",
                "access_level",
                "created_at",
            ],
        ),
        (
            "collection_group_access",
            &[
                "id",
                "org_id",
                "collection_id",
                "group_id",
                "access_level",
                "created_at",
            ],
        ),
        (
            "collection_role_access",
            &[
                "id",
                "org_id",
                "collection_id",
                "role_id",
                "access_level",
                "created_at",
            ],
        ),
        (
            "documents",
            &[
                "id",
                "org_id",
                "collection_id",
                "title",
                "state",
                "current_version_id",
                "created_by_user_id",
                "created_at",
                "updated_at",
                "deleted_at",
            ],
        ),
        (
            "document_versions",
            &[
                "id",
                "org_id",
                "document_id",
                "version_number",
                "parent_version_id",
                "publication_state",
                "is_current",
                "content_sha256",
                "original_object_key",
                "markdown_object_key",
                "source_filename",
                "source_content_type",
                "byte_size",
                "effective_from",
                "effective_to",
                "change_summary",
                "created_by_user_id",
                "created_at",
            ],
        ),
        (
            "derived_artifacts",
            &[
                "id",
                "org_id",
                "document_id",
                "version_id",
                "artifact_kind",
                "object_key",
                "content_sha256",
                "content_type",
                "byte_size",
                "created_at",
            ],
        ),
        (
            "index_metadata",
            &[
                "id",
                "org_id",
                "collection_id",
                "index_signature_sha256",
                "identity_version",
                "chunking_version",
                "body_text_version",
                "query_normalization_version",
                "embedding_family",
                "embedding_revision",
                "dimensions",
                "normalized",
                "runtime_path",
                "generation",
                "is_active",
                "state",
                "created_at",
            ],
        ),
        (
            "index_generation_backfills",
            &[
                "id",
                "org_id",
                "index_metadata_id",
                "document_id",
                "version_id",
                "status",
                "created_at",
                "completed_at",
            ],
        ),
        (
            "embedding_batches",
            &[
                "id",
                "org_id",
                "index_job_id",
                "job_id",
                "index_metadata_id",
                "document_id",
                "version_id",
                "start_ordinal",
                "end_ordinal",
                "input_sha256",
                "status",
                "created_at",
                "completed_at",
            ],
        ),
        (
            "chunks",
            &[
                "id",
                "org_id",
                "document_id",
                "version_id",
                "ordinal",
                "heading_path",
                "body",
                "body_text_version",
                "chunk_identity_sha256",
                "index_metadata_id",
                "index_signature",
                "page",
                "slide",
                "sheet",
                "span_start",
                "span_end",
                "tsv",
                "created_at",
            ],
        ),
        (
            "claims",
            &[
                "id",
                "org_id",
                "document_id",
                "version_id",
                "chunk_id",
                "claim_key",
                "subject",
                "predicate",
                "value_type",
                "value_number",
                "value_text",
                "value_boolean",
                "value_date",
                "value_money",
                "unit",
                "scope",
                "effective_from",
                "effective_to",
                "citation_quote",
                "citation_span_start",
                "citation_span_end",
                "created_at",
            ],
        ),
        (
            "conflicts",
            &[
                "id",
                "org_id",
                "status",
                "severity",
                "conflict_type",
                "claim_a_id",
                "claim_b_id",
                "first_detected_at",
                "first_detected_version_id",
                "resolved_at",
                "resolution_note",
                "resolution_version_a_id",
                "resolution_version_b_id",
                "created_at",
                "updated_at",
            ],
        ),
        (
            "conflict_evidence",
            &[
                "id",
                "org_id",
                "conflict_id",
                "claim_id",
                "evidence_role",
                "citation_quote",
                "created_at",
            ],
        ),
        (
            "jobs",
            &[
                "id",
                "org_id",
                "job_type",
                "status",
                "payload_version",
                "payload",
                "attempts",
                "max_attempts",
                "lease_owner",
                "lease_expires_at",
                "heartbeat_at",
                "checkpoint",
                "idempotency_key",
                "document_id",
                "version_id",
                "available_at",
                "started_at",
                "finished_at",
                "last_error",
                "created_at",
                "updated_at",
            ],
        ),
        (
            "outbox_events",
            &[
                "id",
                "org_id",
                "event_type",
                "payload_version",
                "payload",
                "idempotency_key",
                "job_id",
                "published_at",
                "created_at",
            ],
        ),
        (
            "event_log",
            &[
                "id",
                "org_id",
                "sequence_no",
                "event_type",
                "payload_version",
                "payload",
                "job_id",
                "document_id",
                "version_id",
                "created_at",
            ],
        ),
        (
            "org_quotas",
            &[
                "org_id",
                "max_storage_bytes",
                "max_documents",
                "max_concurrent_jobs",
                "max_monthly_tokens",
                "updated_at",
            ],
        ),
        (
            "usage_counters",
            &[
                "id",
                "org_id",
                "counter_key",
                "period_start",
                "period_end",
                "value",
                "updated_at",
            ],
        ),
        (
            "quota_reservations",
            &[
                "id",
                "org_id",
                "reservation_key",
                "resource_kind",
                "amount",
                "status",
                "expires_at",
                "job_id",
                "created_at",
                "settled_at",
            ],
        ),
        (
            "audit_log",
            &[
                "id",
                "org_id",
                "seq",
                "actor_user_id",
                "action",
                "resource_type",
                "resource_id",
                "outcome",
                "metadata",
                "request_id",
                "created_at",
            ],
        ),
        (
            "download_capability_redemptions",
            &["org_id", "jti", "redeemed_at", "expires_at"],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::{IndexGenerationState, SecretHash};

    #[test]
    fn secret_hash_debug_is_redacted() {
        let hash = SecretHash::new("super-secret-token-hash-value");
        assert_eq!(format!("{hash:?}"), "[REDACTED]");
        assert!(hash.expose().contains("super-secret"));
    }

    #[test]
    fn index_generation_lifecycle_labels_round_trip() {
        for (state, label) in [
            (IndexGenerationState::Building, "building"),
            (IndexGenerationState::Shadow, "shadow"),
            (IndexGenerationState::Active, "active"),
            (IndexGenerationState::Draining, "draining"),
            (IndexGenerationState::Retired, "retired"),
        ] {
            assert_eq!(state.as_str(), label);
            assert_eq!(IndexGenerationState::parse(label).unwrap(), state);
        }
        assert!(IndexGenerationState::parse("mixed").is_err());
    }
}
