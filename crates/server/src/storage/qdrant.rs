//! Qdrant REST adapter with mandatory org/collection filters (ADR 0009).
//!
//! Point IDs are bound to `(org_id, collection_id, chunk_identity)` so collisions
//! across orgs **or** collections are impossible by construction. Every
//! read/search/delete uses server-side org (+ authorized collection) filters.
//! Collection names are always [`CollectionName`] (never raw strings).
//!
//! Destructive collection drops use [`QdrantAdminClient`], which requires a
//! distinct operator admin API key and is **not** re-exported from `storage`
//! (tenant request paths must not obtain it).

use std::collections::BTreeSet;
use std::fmt;

use fileconv_knowledge::identity::IndexSignature;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::config::SecretString;
use crate::services::index_signature::{
    collection_name_for_digest, collection_name_for_signature, validate_signature_digest,
    CollectionName,
};
use crate::storage::error::StorageError;
use crate::storage::url_safety::normalize_service_url;

const POINT_ID_DOMAIN: &[u8] = b"markhand-qdrant-point-v2";

/// Mandatory tenant + authorized-collection scope for every vector operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VectorScope {
    pub org_id: Uuid,
    pub collection_ids: BTreeSet<Uuid>,
}

impl VectorScope {
    pub fn new(org_id: Uuid, collection_ids: impl IntoIterator<Item = Uuid>) -> Self {
        Self {
            org_id,
            collection_ids: collection_ids.into_iter().collect(),
        }
    }

    /// Fail closed when org is nil or the authorized collection set is empty.
    pub fn validate(&self) -> Result<(), StorageError> {
        if self.org_id.is_nil() || self.collection_ids.is_empty() {
            return Err(StorageError::MissingScope);
        }
        if self.collection_ids.iter().any(Uuid::is_nil) {
            return Err(StorageError::MissingScope);
        }
        Ok(())
    }
}

/// Identity + lifecycle markers stored on every Qdrant point payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkPointPayload {
    pub org_id: Uuid,
    pub collection_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub chunk_id: String,
    pub ordinal: u64,
    pub is_current: bool,
    pub is_effective: bool,
    pub index_generation: u32,
}

impl ChunkPointPayload {
    fn to_json(&self) -> Value {
        json!({
            "org_id": self.org_id.to_string(),
            "collection_id": self.collection_id.to_string(),
            "document_id": self.document_id.to_string(),
            "version_id": self.version_id.to_string(),
            "chunk_id": self.chunk_id,
            "ordinal": self.ordinal,
            "is_current": self.is_current,
            "is_effective": self.is_effective,
            "index_generation": self.index_generation,
        })
    }

    fn from_json(value: &Value) -> Result<Self, StorageError> {
        let obj = value.as_object().ok_or(StorageError::Backend)?;
        let get_uuid = |key: &str| -> Result<Uuid, StorageError> {
            let raw = obj
                .get(key)
                .and_then(Value::as_str)
                .ok_or(StorageError::Backend)?;
            Uuid::parse_str(raw).map_err(|_| StorageError::Backend)
        };
        Ok(Self {
            org_id: get_uuid("org_id")?,
            collection_id: get_uuid("collection_id")?,
            document_id: get_uuid("document_id")?,
            version_id: get_uuid("version_id")?,
            chunk_id: obj
                .get("chunk_id")
                .and_then(Value::as_str)
                .ok_or(StorageError::Backend)?
                .to_string(),
            ordinal: obj
                .get("ordinal")
                .and_then(Value::as_u64)
                .ok_or(StorageError::Backend)?,
            is_current: obj
                .get("is_current")
                .and_then(Value::as_bool)
                .ok_or(StorageError::Backend)?,
            is_effective: obj
                .get("is_effective")
                .and_then(Value::as_bool)
                .ok_or(StorageError::Backend)?,
            index_generation: {
                let raw = obj
                    .get("index_generation")
                    .and_then(Value::as_u64)
                    .ok_or(StorageError::Backend)?;
                u32::try_from(raw).map_err(|_| StorageError::PreconditionFailed)?
            },
        })
    }
}

/// Point to upsert into a generation-scoped collection.
#[derive(Debug, Clone)]
pub struct UpsertPoint {
    pub chunk_identity: String,
    pub vector: Vec<f32>,
    pub payload: ChunkPointPayload,
}

/// Search hit returned under a mandatory tenant filter.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub point_id: Uuid,
    pub score: f32,
    pub payload: ChunkPointPayload,
}

/// Fail-closed Qdrant client using the HTTP REST API (no gRPC dependency).
#[derive(Clone)]
pub struct QdrantClient {
    base_url: String,
    api_key: Option<SecretString>,
    http: Client,
}

impl fmt::Debug for QdrantClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QdrantClient")
            .field("base_url", &"[REDACTED_ENDPOINT]")
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

impl QdrantClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self, StorageError> {
        Self::with_api_key(base_url, None)
    }

    pub fn with_api_key(
        base_url: impl Into<String>,
        api_key: Option<SecretString>,
    ) -> Result<Self, StorageError> {
        let base_url = normalize_service_url(base_url.into())?;
        if let Some(key) = api_key.as_ref() {
            if key.expose().is_empty() {
                return Err(StorageError::ConfigMissingCredentials);
            }
        }
        let http = Client::builder()
            .use_rustls_tls()
            .build()
            .map_err(|_| StorageError::ConfigInvalid)?;
        Ok(Self {
            base_url,
            api_key,
            http,
        })
    }

    /// Ensure the versioned collection for `signature` exists (size + distance).
    pub async fn ensure_collection_for_signature(
        &self,
        signature: &IndexSignature<'_>,
    ) -> Result<CollectionName, StorageError> {
        let name = collection_name_for_signature(signature)?;
        self.ensure_collection(&name, signature.dimensions, signature.normalized)
            .await?;
        Ok(name)
    }

    /// Ensure a collection named from a known digest exists.
    pub async fn ensure_collection_for_digest(
        &self,
        digest: &str,
        dimensions: usize,
        normalized: bool,
    ) -> Result<CollectionName, StorageError> {
        validate_signature_digest(digest)?;
        let name = collection_name_for_digest(digest)?;
        self.ensure_collection(&name, dimensions, normalized)
            .await?;
        Ok(name)
    }

    async fn ensure_collection(
        &self,
        name: &CollectionName,
        dimensions: usize,
        normalized: bool,
    ) -> Result<(), StorageError> {
        if dimensions == 0 {
            return Err(StorageError::PreconditionFailed);
        }
        let distance = if normalized { "Cosine" } else { "Dot" };
        if let Some((existing_size, existing_distance)) = self.get_collection_params(name).await? {
            if existing_size != dimensions || !distance_matches(&existing_distance, distance) {
                return Err(StorageError::CollectionMismatch);
            }
            return Ok(());
        }
        let url = format!("{}/collections/{}", self.base_url, name.as_str());
        let body = json!({
            "vectors": {
                "size": dimensions,
                "distance": distance
            }
        });
        let response = self
            .authed(self.http.put(&url))
            .json(&body)
            .send()
            .await
            .map_err(|_| StorageError::Transport)?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let text = response.text().await.unwrap_or_default();
        if status.as_u16() == 409
            || text.contains("already exists")
            || text.contains("AlreadyExists")
        {
            let Some((existing_size, existing_distance)) = self.get_collection_params(name).await?
            else {
                return Err(StorageError::Backend);
            };
            if existing_size != dimensions || !distance_matches(&existing_distance, distance) {
                return Err(StorageError::CollectionMismatch);
            }
            return Ok(());
        }
        Err(StorageError::Backend)
    }

    async fn get_collection_params(
        &self,
        name: &CollectionName,
    ) -> Result<Option<(usize, String)>, StorageError> {
        let url = format!("{}/collections/{}", self.base_url, name.as_str());
        let response = self
            .authed(self.http.get(&url))
            .send()
            .await
            .map_err(|_| StorageError::Transport)?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(StorageError::Backend);
        }
        let body: Value = response.json().await.map_err(|_| StorageError::Backend)?;
        let vectors = body
            .pointer("/result/config/params/vectors")
            .ok_or(StorageError::Backend)?;
        let (size, distance) = if let Some(size) = vectors.get("size").and_then(Value::as_u64) {
            let distance = vectors
                .get("distance")
                .and_then(Value::as_str)
                .ok_or(StorageError::Backend)?;
            (size as usize, distance.to_string())
        } else if let Some(obj) = vectors.as_object() {
            let first = obj.values().next().ok_or(StorageError::Backend)?;
            let size = first
                .get("size")
                .and_then(Value::as_u64)
                .ok_or(StorageError::Backend)? as usize;
            let distance = first
                .get("distance")
                .and_then(Value::as_str)
                .ok_or(StorageError::Backend)?
                .to_string();
            (size, distance)
        } else {
            return Err(StorageError::Backend);
        };
        Ok(Some((size, distance)))
    }

    /// Upsert deterministic points bound to `(org, collection, chunk)`.
    pub async fn upsert_points(
        &self,
        collection_name: &CollectionName,
        scope: &VectorScope,
        points: &[UpsertPoint],
    ) -> Result<Vec<Uuid>, StorageError> {
        scope.validate()?;
        if points.is_empty() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::with_capacity(points.len());
        let mut body_points = Vec::with_capacity(points.len());
        for point in points {
            self.enforce_payload_scope(scope, &point.payload)?;
            if point.payload.chunk_id != point.chunk_identity {
                return Err(StorageError::PreconditionFailed);
            }
            let point_id = point_id_from_org_collection_and_chunk(
                scope.org_id,
                point.payload.collection_id,
                &point.chunk_identity,
            )?;
            ids.push(point_id);
            body_points.push(json!({
                "id": point_id.to_string(),
                "vector": point.vector,
                "payload": point.payload.to_json(),
            }));
        }
        self.assert_owned_or_absent(collection_name, scope, &ids)
            .await?;

        let url = format!(
            "{}/collections/{}/points?wait=true",
            self.base_url,
            collection_name.as_str()
        );
        let response = self
            .authed(self.http.put(&url))
            .json(&json!({ "points": body_points }))
            .send()
            .await
            .map_err(|_| StorageError::Transport)?;
        if !response.status().is_success() {
            return Err(StorageError::Backend);
        }
        Ok(ids)
    }

    /// Search with a mandatory org + authorized collection filter.
    pub async fn search(
        &self,
        collection_name: &CollectionName,
        scope: &VectorScope,
        vector: &[f32],
        limit: usize,
    ) -> Result<Vec<SearchHit>, StorageError> {
        scope.validate()?;
        if limit == 0 {
            return Err(StorageError::PreconditionFailed);
        }
        let url = format!(
            "{}/collections/{}/points/query",
            self.base_url,
            collection_name.as_str()
        );
        let body = json!({
            "query": vector,
            "filter": mandatory_filter(scope),
            "limit": limit,
            "with_payload": true,
        });
        let response = self
            .authed(self.http.post(&url))
            .json(&body)
            .send()
            .await
            .map_err(|_| StorageError::Transport)?;
        if response.status().as_u16() == 404 || !response.status().is_success() {
            return self
                .search_legacy(collection_name, scope, vector, limit)
                .await;
        }
        let payload: Value = response.json().await.map_err(|_| StorageError::Backend)?;
        let hits = parse_search_hits(&payload)?;
        enforce_hits_in_scope(scope, &hits)?;
        Ok(hits)
    }

    async fn search_legacy(
        &self,
        collection_name: &CollectionName,
        scope: &VectorScope,
        vector: &[f32],
        limit: usize,
    ) -> Result<Vec<SearchHit>, StorageError> {
        let url = format!(
            "{}/collections/{}/points/search",
            self.base_url,
            collection_name.as_str()
        );
        let body = json!({
            "vector": vector,
            "filter": mandatory_filter(scope),
            "limit": limit,
            "with_payload": true,
        });
        let response = self
            .authed(self.http.post(&url))
            .json(&body)
            .send()
            .await
            .map_err(|_| StorageError::Transport)?;
        if !response.status().is_success() {
            return Err(StorageError::Backend);
        }
        let payload: Value = response.json().await.map_err(|_| StorageError::Backend)?;
        let hits = parse_search_hits(&payload)?;
        enforce_hits_in_scope(scope, &hits)?;
        Ok(hits)
    }

    /// Delete points matching the mandatory org/collection filter (and optional extra).
    pub async fn delete_by_scope(
        &self,
        collection_name: &CollectionName,
        scope: &VectorScope,
        extra_must: &[Value],
    ) -> Result<(), StorageError> {
        scope.validate()?;
        let mut must = mandatory_filter_must(scope);
        must.extend(extra_must.iter().cloned());
        let url = format!(
            "{}/collections/{}/points/delete?wait=true",
            self.base_url,
            collection_name.as_str()
        );
        let response = self
            .authed(self.http.post(&url))
            .json(&json!({ "filter": { "must": must } }))
            .send()
            .await
            .map_err(|_| StorageError::Transport)?;
        if !response.status().is_success() {
            return Err(StorageError::Backend);
        }
        Ok(())
    }

    /// Fetch points by id using a **server-side** org + collection + has_id filter.
    pub async fn get_points(
        &self,
        collection_name: &CollectionName,
        scope: &VectorScope,
        point_ids: &[Uuid],
    ) -> Result<Vec<(Uuid, ChunkPointPayload)>, StorageError> {
        scope.validate()?;
        if point_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut must = mandatory_filter_must(scope);
        let ids: Vec<Value> = point_ids
            .iter()
            .map(|id| Value::String(id.to_string()))
            .collect();
        must.push(json!({ "has_id": ids }));
        let url = format!(
            "{}/collections/{}/points/scroll",
            self.base_url,
            collection_name.as_str()
        );
        let response = self
            .authed(self.http.post(&url))
            .json(&json!({
                "filter": { "must": must },
                "limit": point_ids.len(),
                "with_payload": true,
                "with_vector": false,
            }))
            .send()
            .await
            .map_err(|_| StorageError::Transport)?;
        if !response.status().is_success() {
            return Err(StorageError::Backend);
        }
        let body: Value = response.json().await.map_err(|_| StorageError::Backend)?;
        let points = body
            .pointer("/result/points")
            .and_then(Value::as_array)
            .ok_or(StorageError::Backend)?;
        let mut out = Vec::new();
        for point in points {
            let id = parse_point_id(point.get("id").ok_or(StorageError::Backend)?)?;
            let payload =
                ChunkPointPayload::from_json(point.get("payload").ok_or(StorageError::Backend)?)?;
            if payload.org_id != scope.org_id
                || !scope.collection_ids.contains(&payload.collection_id)
            {
                return Err(StorageError::OwnershipConflict);
            }
            out.push((id, payload));
        }
        Ok(out)
    }

    async fn assert_owned_or_absent(
        &self,
        collection_name: &CollectionName,
        scope: &VectorScope,
        point_ids: &[Uuid],
    ) -> Result<(), StorageError> {
        let existing = self.get_points(collection_name, scope, point_ids).await?;
        for (_, payload) in existing {
            if payload.org_id != scope.org_id
                || !scope.collection_ids.contains(&payload.collection_id)
            {
                return Err(StorageError::OwnershipConflict);
            }
        }
        Ok(())
    }

    fn enforce_payload_scope(
        &self,
        scope: &VectorScope,
        payload: &ChunkPointPayload,
    ) -> Result<(), StorageError> {
        if payload.org_id != scope.org_id {
            return Err(StorageError::MissingScope);
        }
        if !scope.collection_ids.contains(&payload.collection_id) {
            return Err(StorageError::MissingScope);
        }
        Ok(())
    }

    fn authed(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(key) = self.api_key.as_ref() {
            builder.header("api-key", key.expose())
        } else {
            builder
        }
    }
}

/// Operator credential for destructive Qdrant collection lifecycle.
///
/// Distinct from the tenant `api-key`. Never constructed from request auth.
#[derive(Clone)]
pub struct QdrantAdminApiKey(SecretString);

impl fmt::Debug for QdrantAdminApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("QdrantAdminApiKey([REDACTED])")
    }
}

impl QdrantAdminApiKey {
    /// Build from `MARKHAND_QDRANT_ADMIN_API_KEY` (must be non-empty).
    pub fn new(key: SecretString) -> Result<Self, StorageError> {
        if key.expose().is_empty() {
            return Err(StorageError::ConfigMissingCredentials);
        }
        Ok(Self(key))
    }
}

/// Operator-only Qdrant admin client (collection drop).
///
/// **Boundary:** not re-exported from `crate::storage`. Tenant HTTP/worker paths
/// must not import or construct this type. Requires [`QdrantAdminApiKey`] —
/// never reuses the tenant client's credentials.
#[derive(Clone)]
pub struct QdrantAdminClient {
    base_url: String,
    admin_api_key: SecretString,
    http: Client,
}

impl fmt::Debug for QdrantAdminClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QdrantAdminClient")
            .field("base_url", &"[REDACTED_ENDPOINT]")
            .field("admin_api_key", &"[REDACTED]")
            .finish()
    }
}

impl QdrantAdminClient {
    /// Construct with a distinct operator admin API key (not the tenant key).
    pub fn new(
        base_url: impl Into<String>,
        admin_key: QdrantAdminApiKey,
    ) -> Result<Self, StorageError> {
        let base_url = normalize_service_url(base_url.into())?;
        let http = Client::builder()
            .use_rustls_tls()
            .build()
            .map_err(|_| StorageError::ConfigInvalid)?;
        Ok(Self {
            base_url,
            admin_api_key: admin_key.0,
            http,
        })
    }

    /// Drop an entire Qdrant collection. Operator-only.
    pub async fn delete_collection(
        &self,
        collection_name: &CollectionName,
    ) -> Result<(), StorageError> {
        let url = format!("{}/collections/{}", self.base_url, collection_name.as_str());
        let response = self
            .http
            .delete(&url)
            .header("api-key", self.admin_api_key.expose())
            .send()
            .await
            .map_err(|_| StorageError::Transport)?;
        if response.status().is_success() || response.status().as_u16() == 404 {
            return Ok(());
        }
        Err(StorageError::Backend)
    }
}

/// Deterministic UUIDv8 from `(org_id, collection_id, chunk_identity)`.
pub fn point_id_from_org_collection_and_chunk(
    org_id: Uuid,
    collection_id: Uuid,
    chunk_identity_hex: &str,
) -> Result<Uuid, StorageError> {
    if org_id.is_nil() || collection_id.is_nil() {
        return Err(StorageError::MissingScope);
    }
    if chunk_identity_hex.len() != 64
        || !chunk_identity_hex.bytes().all(|byte| {
            // Canonical lowercase hex only — casing must never fork point ids.
            matches!(byte, b'0'..=b'9' | b'a'..=b'f')
        })
    {
        return Err(StorageError::PreconditionFailed);
    }
    let mut hasher = Sha256::new();
    hasher.update(POINT_ID_DOMAIN);
    hasher.update(org_id.as_bytes());
    hasher.update(collection_id.as_bytes());
    hasher.update(chunk_identity_hex.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Ok(Uuid::new_v8(bytes))
}

fn distance_matches(existing: &str, expected: &str) -> bool {
    existing.eq_ignore_ascii_case(expected)
}

fn mandatory_filter(scope: &VectorScope) -> Value {
    json!({ "must": mandatory_filter_must(scope) })
}

fn mandatory_filter_must(scope: &VectorScope) -> Vec<Value> {
    let collection_values: Vec<String> = scope.collection_ids.iter().map(Uuid::to_string).collect();
    vec![
        json!({
            "key": "org_id",
            "match": { "value": scope.org_id.to_string() }
        }),
        json!({
            "key": "collection_id",
            "match": { "any": collection_values }
        }),
    ]
}

fn enforce_hits_in_scope(scope: &VectorScope, hits: &[SearchHit]) -> Result<(), StorageError> {
    for hit in hits {
        if hit.payload.org_id != scope.org_id
            || !scope.collection_ids.contains(&hit.payload.collection_id)
        {
            return Err(StorageError::OwnershipConflict);
        }
    }
    Ok(())
}

fn parse_search_hits(body: &Value) -> Result<Vec<SearchHit>, StorageError> {
    let points = body
        .pointer("/result/points")
        .and_then(Value::as_array)
        .or_else(|| body.pointer("/result").and_then(Value::as_array))
        .ok_or(StorageError::Backend)?;
    let mut hits = Vec::with_capacity(points.len());
    for point in points {
        let point_id = parse_point_id(point.get("id").ok_or(StorageError::Backend)?)?;
        let score = point.get("score").and_then(Value::as_f64).unwrap_or(0.0) as f32;
        let payload =
            ChunkPointPayload::from_json(point.get("payload").ok_or(StorageError::Backend)?)?;
        hits.push(SearchHit {
            point_id,
            score,
            payload,
        });
    }
    Ok(hits)
}

fn parse_point_id(value: &Value) -> Result<Uuid, StorageError> {
    match value {
        Value::String(text) => Uuid::parse_str(text).map_err(|_| StorageError::Backend),
        Value::Number(_) => Err(StorageError::Backend),
        _ => Err(StorageError::Backend),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_scope_is_rejected() {
        let empty_org = VectorScope::new(Uuid::nil(), [Uuid::new_v4()]);
        assert!(matches!(
            empty_org.validate(),
            Err(StorageError::MissingScope)
        ));
        let empty_collections = VectorScope::new(Uuid::new_v4(), []);
        assert!(matches!(
            empty_collections.validate(),
            Err(StorageError::MissingScope)
        ));
    }

    #[test]
    fn point_id_binds_org_and_collection() {
        let identity = "d54db7b6de20b51a416670927eeab346256c9b891732965e51586fac333c1835";
        let org = Uuid::new_v4();
        let col_a = Uuid::new_v4();
        let col_b = Uuid::new_v4();
        let a1 = point_id_from_org_collection_and_chunk(org, col_a, identity).unwrap();
        let a2 = point_id_from_org_collection_and_chunk(org, col_a, identity).unwrap();
        let b = point_id_from_org_collection_and_chunk(org, col_b, identity).unwrap();
        let other_org =
            point_id_from_org_collection_and_chunk(Uuid::new_v4(), col_a, identity).unwrap();
        assert_eq!(a1, a2);
        assert_ne!(a1, b);
        assert_ne!(a1, other_org);
        assert_eq!(a1.get_version(), Some(uuid::Version::Custom));
    }

    #[test]
    fn point_id_rejects_uppercase_chunk_digest() {
        let org = Uuid::new_v4();
        let col = Uuid::new_v4();
        let upper = "D54DB7B6DE20B51A416670927EEAB346256C9B891732965E51586FAC333C1835";
        assert!(matches!(
            point_id_from_org_collection_and_chunk(org, col, upper),
            Err(StorageError::PreconditionFailed)
        ));
        let mixed =
            "d54db7b6de20b51a416670927eeab346256c9b891732965e51586fac333c1835".to_ascii_uppercase();
        assert!(matches!(
            point_id_from_org_collection_and_chunk(org, col, &mixed),
            Err(StorageError::PreconditionFailed)
        ));
    }

    #[test]
    fn debug_redacts_endpoint_and_api_key() {
        let client = QdrantClient::with_api_key(
            "http://127.0.0.1:6333",
            Some(SecretString::new("super-secret-qdrant-key")),
        )
        .unwrap();
        let debug = format!("{client:?}");
        assert!(!debug.contains("127.0.0.1"));
        assert!(!debug.contains("super-secret"));
        assert!(debug.contains("[REDACTED"));
    }
}
