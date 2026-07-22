//! Stable `/api/v1` error envelope and codes.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Canonical error body returned by `/api/v1` endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl ApiError {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        request_id: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            request_id: request_id.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

/// Shared route-layer rejection that never embeds internal dependency detail.
#[derive(Debug, Clone)]
pub struct ApiRejection {
    status: StatusCode,
    body: ApiError,
}

impl ApiRejection {
    pub fn new(
        status: StatusCode,
        code: impl Into<String>,
        message: impl Into<String>,
        request_id: impl Into<String>,
    ) -> Self {
        Self {
            status,
            body: ApiError::new(code, message, request_id),
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.body.details = Some(details);
        self
    }

    pub fn validation(message: impl Into<String>, request_id: impl Into<String>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "validation_failed",
            message,
            request_id,
        )
    }

    pub fn not_found(message: impl Into<String>, request_id: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message, request_id)
    }

    pub fn permission_denied(request_id: impl Into<String>) -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "permission_denied",
            "Permission denied",
            request_id,
        )
    }

    pub fn collection_denied(request_id: impl Into<String>) -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "collection_denied",
            "Collection access denied",
            request_id,
        )
    }

    pub fn not_implemented(message: impl Into<String>, request_id: impl Into<String>) -> Self {
        Self::new(
            StatusCode::NOT_IMPLEMENTED,
            "not_implemented",
            message,
            request_id,
        )
    }

    pub fn conflict(
        code: impl Into<String>,
        message: impl Into<String>,
        request_id: impl Into<String>,
    ) -> Self {
        Self::new(StatusCode::CONFLICT, code, message, request_id)
    }

    pub fn internal(request_id: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Internal server error",
            request_id,
        )
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn body(&self) -> &ApiError {
        &self.body
    }
}

impl IntoResponse for ApiRejection {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}
