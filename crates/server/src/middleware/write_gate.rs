//! Central mutation write-gate (P1B-O03).
//!
//! Coordinates with backup consistency capture via shared/exclusive advisory
//! lock key [`BACKUP_ADVISORY_LOCK_KEY`] (same as `deploy/backup` 7303003):
//! - Backup holds an exclusive session lock for the capture window.
//! - Business API traffic takes a shared session lock for the request window
//!   (released before long-lived SSE bodies; stream producers re-check).
//! - Active `ops_fences` rows fail closed with `ops_fence_active`.
//! - DB errors while checking fail closed with `ops_fence_check_failed`.
//!
//! Health/readiness/startup, `/metrics`, and OpenAPI stay exempt for ops.
//!
//! Contract anchors (scanned by `app_mutation_write_gate_sufficient`):
//! - `WRITE_GATE_CONTRACT_ID = "markhand.write_gate.v1"`
//! - `BACKUP_ADVISORY_LOCK_KEY: i64 = 7303003`
//! - `pub async fn mutation_write_gate`

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, Request, Response, StatusCode};
use axum::middleware::Next;
use deadpool_postgres::{Object, Pool};

use crate::http::AppState;
use crate::middleware::RequestId;
use crate::services::ops_fence::{
    self, FenceError, MUTATIONS_PAUSED_CODE, OPS_FENCE_CHECK_FAILED_CODE,
};

/// Machine-verifiable contract id (scanned by backup write-gate detector).
pub const WRITE_GATE_CONTRACT_ID: &str = "markhand.write_gate.v1";

/// Must match `deploy/backup/lib/pipeline.py` `ADVISORY_LOCK_KEY`.
pub const BACKUP_ADVISORY_LOCK_KEY: i64 = 7303003;

/// Paths that must remain reachable during fenced capture (ops/diagnostics).
pub fn is_write_gate_exempt(path: &str) -> bool {
    path.starts_with("/api/v1/health/")
        || path == "/metrics"
        || path == "/api/v1/openapi.yaml"
        || !path.starts_with("/api/v1/")
}

/// Long-lived SSE responses must not hold the shared lock for the body lifetime.
pub fn is_long_lived_stream_path(path: &str) -> bool {
    path == "/api/v1/ask/stream" || path.ends_with("/events")
}

/// Central middleware: shared advisory lock + fence check for business API traffic.
pub async fn mutation_write_gate(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    let path = request.uri().path().to_string();
    if is_write_gate_exempt(&path) {
        return next.run(request).await;
    }
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|id| id.0.clone())
        .unwrap_or_else(|| "missing-middleware-request-id".into());

    let stream = is_long_lived_stream_path(&path);
    match begin_request_write_gate(state.pool(), stream).await {
        Ok(None) => next.run(request).await,
        Ok(Some(mut guard)) => {
            let response = next.run(request).await;
            guard.release().await;
            response
        }
        Err(error) => fence_error_response(&request_id, &error),
    }
}

/// Acquire shared backup lock + verify fence. For stream paths, unlock before return
/// so the SSE body does not pin the lock; producers must re-check.
async fn begin_request_write_gate(
    pool: &Pool,
    release_before_handler: bool,
) -> Result<Option<SharedLockGuard>, FenceError> {
    let client = pool.get().await.map_err(|_| FenceError::Database)?;
    let locked: bool = client
        .query_one(
            "SELECT pg_try_advisory_lock_shared($1)",
            &[&BACKUP_ADVISORY_LOCK_KEY],
        )
        .await
        .map_err(|_| FenceError::Database)?
        .get(0);
    if !locked {
        // Exclusive backup lock held — treat as fenced capture in progress.
        return Err(FenceError::Active);
    }
    let fence = match ops_fence::any_blocking_fence_active_on(&client).await {
        Ok(active) => active,
        Err(error) => {
            let _ = unlock_shared(&client).await;
            return Err(error);
        }
    };
    if fence {
        let _ = unlock_shared(&client).await;
        return Err(FenceError::Active);
    }
    if release_before_handler {
        let _ = unlock_shared(&client).await;
        return Ok(None);
    }
    Ok(Some(SharedLockGuard {
        client: Some(client),
    }))
}

/// Background loops / stream producers: fail closed without holding past the check.
pub async fn ensure_background_mutations_allowed(pool: &Pool) -> Result<(), FenceError> {
    let client = pool.get().await.map_err(|_| FenceError::Database)?;
    let locked: bool = client
        .query_one(
            "SELECT pg_try_advisory_lock_shared($1)",
            &[&BACKUP_ADVISORY_LOCK_KEY],
        )
        .await
        .map_err(|_| FenceError::Database)?
        .get(0);
    if !locked {
        return Err(FenceError::Active);
    }
    let result = ops_fence::ensure_mutations_allowed_on(&client).await;
    let _ = unlock_shared(&client).await;
    result
}

struct SharedLockGuard {
    client: Option<Object>,
}

impl SharedLockGuard {
    async fn release(&mut self) {
        if let Some(client) = self.client.take() {
            let _ = unlock_shared(&client).await;
        }
    }
}

impl Drop for SharedLockGuard {
    fn drop(&mut self) {
        if let Some(client) = self.client.take() {
            // Fallback if caller forgot release(); unlock on a detached task.
            tokio::spawn(async move {
                let _ = unlock_shared(&client).await;
            });
        }
    }
}

async fn unlock_shared(client: &Object) -> Result<(), FenceError> {
    client
        .execute(
            "SELECT pg_advisory_unlock_shared($1)",
            &[&BACKUP_ADVISORY_LOCK_KEY],
        )
        .await
        .map_err(|_| FenceError::Database)?;
    Ok(())
}

fn fence_error_response(request_id: &str, error: &FenceError) -> Response<Body> {
    let code = ops_fence::mutation_pause_code(error);
    let message = match error {
        FenceError::Active => "Mutations paused while an ops fence is active",
        _ => "Unable to verify ops fence state",
    };
    let mut response = Response::new(Body::from(
        serde_json::json!({
            "code": code,
            "message": message,
            "requestId": request_id,
        })
        .to_string(),
    ));
    *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    let _ = (MUTATIONS_PAUSED_CODE, OPS_FENCE_CHECK_FAILED_CODE);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_constants_are_stable() {
        assert_eq!(WRITE_GATE_CONTRACT_ID, "markhand.write_gate.v1");
        assert_eq!(BACKUP_ADVISORY_LOCK_KEY, 7303003);
    }

    #[test]
    fn exempts_ops_and_openapi_only() {
        assert!(is_write_gate_exempt("/api/v1/health/live"));
        assert!(is_write_gate_exempt("/api/v1/health/ready"));
        assert!(is_write_gate_exempt("/api/v1/health/start"));
        assert!(is_write_gate_exempt("/metrics"));
        assert!(is_write_gate_exempt("/api/v1/openapi.yaml"));
        assert!(!is_write_gate_exempt("/api/v1/auth/login"));
        assert!(!is_write_gate_exempt("/api/v1/collections"));
        assert!(!is_write_gate_exempt("/api/v1/ask"));
        assert!(!is_write_gate_exempt("/api/v1/ask/stream"));
        assert!(!is_write_gate_exempt("/api/v1/search"));
    }

    #[test]
    fn stream_paths_release_lock_before_body() {
        assert!(is_long_lived_stream_path("/api/v1/ask/stream"));
        assert!(is_long_lived_stream_path(
            "/api/v1/jobs/00000000-0000-0000-0000-000000000001/events"
        ));
        assert!(!is_long_lived_stream_path("/api/v1/ask"));
        assert!(!is_long_lived_stream_path("/api/v1/collections"));
    }

    #[test]
    fn pause_codes_distinguish_active_vs_check_failed() {
        assert_eq!(
            ops_fence::mutation_pause_code(&FenceError::Active),
            "ops_fence_active"
        );
        assert_eq!(
            ops_fence::mutation_pause_code(&FenceError::Database),
            "ops_fence_check_failed"
        );
    }
}
