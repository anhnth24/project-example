//! Fail-closed readiness probes (P1B-R06 review).
//!
//! Routes must not mention storage product names; this service owns dependency
//! checks and propagates typed probe failures.

use std::time::Duration;

use deadpool_postgres::Pool;
use thiserror::Error;
use tokio::time::timeout;

use crate::config::ServerConfig;
use crate::database;
use crate::services::index_signature;
use crate::services::ops_fence;
use crate::storage::minio::MinioClient;
use crate::storage::qdrant::QdrantClient;

const DEPENDENCY_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReadinessProbeError {
    #[error("postgresql unavailable")]
    Database,
    #[error("vector store unavailable")]
    VectorStore,
    #[error("object store unavailable")]
    ObjectStore,
    #[error("index signature invalid")]
    IndexSignature,
    #[error("no active index generation")]
    ActiveGeneration,
    #[error("reconciliation or restore fence active")]
    ReconcileFence,
    #[error("object store credentials missing")]
    ObjectStoreCredentials,
    #[error("vector store credentials missing")]
    VectorStoreCredentials,
}

impl ReadinessProbeError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Database => "ready_database",
            Self::VectorStore => "ready_vector_store",
            Self::ObjectStore => "ready_object_store",
            Self::IndexSignature => "ready_index_signature",
            Self::ActiveGeneration => "ready_active_generation",
            Self::ReconcileFence => "ready_reconcile_fence",
            Self::ObjectStoreCredentials => "ready_object_store_credentials",
            Self::VectorStoreCredentials => "ready_vector_store_credentials",
        }
    }
}

pub struct ReadinessDeps<'a> {
    pub config: &'a ServerConfig,
    pub database_url: &'a str,
    pub pool: &'a Pool,
    pub http: &'a reqwest::Client,
    pub vector_base_url: &'a str,
    pub vector_client: Option<&'a QdrantClient>,
    pub object_client: Option<&'a MinioClient>,
    pub object_health_url: &'a str,
}

/// Full fail-closed readiness. Any probe error fails the check.
pub async fn check_ready(deps: ReadinessDeps<'_>) -> Result<(), ReadinessProbeError> {
    timeout(
        DEPENDENCY_TIMEOUT,
        database::check_connection(deps.database_url),
    )
    .await
    .map_err(|_| ReadinessProbeError::Database)?
    .map_err(|_| ReadinessProbeError::Database)?;

    if deps.vector_client.is_none() {
        return Err(ReadinessProbeError::VectorStoreCredentials);
    }
    let vector = deps
        .http
        .get(format!(
            "{}/healthz",
            deps.vector_base_url.trim_end_matches('/')
        ))
        .send();
    let vector = timeout(DEPENDENCY_TIMEOUT, vector)
        .await
        .map_err(|_| ReadinessProbeError::VectorStore)?
        .map_err(|_| ReadinessProbeError::VectorStore)?;
    if !vector.status().is_success() {
        return Err(ReadinessProbeError::VectorStore);
    }
    deps.vector_client
        .expect("checked above")
        .collections_probe()
        .await
        .map_err(|_| ReadinessProbeError::VectorStore)?;

    let object = deps
        .object_client
        .ok_or(ReadinessProbeError::ObjectStoreCredentials)?;
    let object_health = deps.http.get(deps.object_health_url).send();
    let object_health = timeout(DEPENDENCY_TIMEOUT, object_health)
        .await
        .map_err(|_| ReadinessProbeError::ObjectStore)?
        .map_err(|_| ReadinessProbeError::ObjectStore)?;
    if !object_health.status().is_success() {
        return Err(ReadinessProbeError::ObjectStore);
    }
    object
        .bucket_probe()
        .await
        .map_err(|_| ReadinessProbeError::ObjectStore)?;

    match deps.config.index_signature() {
        Some(signature) => {
            index_signature::validate_signature_digest(signature)
                .map_err(|_| ReadinessProbeError::IndexSignature)?;
            // When any generation rows exist, at least one active must match.
            if !active_generation_consistent(deps.pool, signature).await? {
                return Err(ReadinessProbeError::ActiveGeneration);
            }
            // Configured signature must map to an existing vector collection.
            deps.vector_client
                .expect("checked above")
                .collection_probe_for_digest(signature)
                .await
                .map_err(|_| ReadinessProbeError::ActiveGeneration)?;
        }
        None if deps.config.profile() == crate::config::Profile::Prod => {
            return Err(ReadinessProbeError::IndexSignature);
        }
        None => {}
    }

    if ops_fence::any_blocking_fence_active(deps.pool)
        .await
        .map_err(|_| ReadinessProbeError::Database)?
        || ops_fence::any_org_reconcile_running(deps.pool)
            .await
            .map_err(|_| ReadinessProbeError::Database)?
    {
        return Err(ReadinessProbeError::ReconcileFence);
    }
    Ok(())
}

async fn active_generation_consistent(
    pool: &Pool,
    signature: &str,
) -> Result<bool, ReadinessProbeError> {
    // Cross-org check via SECURITY DEFINER aggregate (FORCE RLS safe).
    let client = pool
        .get()
        .await
        .map_err(|_| ReadinessProbeError::Database)?;
    let ok: bool = client
        .query_one(
            "SELECT markhand_index_generation_consistent($1)",
            &[&signature],
        )
        .await
        .map_err(|_| ReadinessProbeError::Database)?
        .get(0);
    Ok(ok)
}
