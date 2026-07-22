//! Versioned wire-contract types shared by routes, workers and fixtures.

mod error;
mod extract;
mod pagination;
mod types;

pub use error::{ApiError, ApiRejection};
pub use extract::{
    body_limit_error, map_multipart_stream_error, AppJson, AppMultipart, AppPath, AppQuery,
};
pub use pagination::{
    decode_cursor, encode_cursor, CreatedAtIdCursor, NameIdCursor, PageInfo, PageParams,
    VersionNumberIdCursor, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, MIN_PAGE_SIZE,
};
pub use types::{
    CollectionResponse, ConflictEvidenceResponse, ConflictResponse, DocumentResponse,
    DocumentVersionResponse, JobResponse, ListResponse, ReindexResponse, SseEnvelope,
    VersionDiffResponse,
};

#[cfg(test)]
mod tests {
    use super::{body_limit_error, ApiError, ConflictResponse, PageInfo, SseEnvelope};
    use uuid::Uuid;

    #[test]
    fn fixtures_round_trip_through_wire_types() {
        let error: ApiError =
            serde_json::from_str(include_str!("../../openapi/fixtures/error.json")).unwrap();
        assert_eq!(error.code, "validation_failed");
        assert_eq!(
            serde_json::to_string_pretty(&error).unwrap(),
            include_str!("../../openapi/fixtures/error.json")
                .trim()
                .replace("\r\n", "\n")
        );

        let page: PageInfo =
            serde_json::from_str(include_str!("../../openapi/fixtures/pagination.json")).unwrap();
        assert!(page.has_more);

        let event: SseEnvelope =
            serde_json::from_str(include_str!("../../openapi/fixtures/sse.json")).unwrap();
        assert_eq!(event.version, 1);
        assert_eq!(event.sequence, 42);
    }

    #[test]
    fn conflict_response_includes_resolution_version_ids() {
        let now = chrono::Utc::now();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let body = ConflictResponse {
            id: Uuid::new_v4(),
            status: "resolved".into(),
            severity: "warning".into(),
            conflict_type: "numeric".into(),
            claim_a_id: Uuid::new_v4(),
            claim_b_id: Uuid::new_v4(),
            first_detected_at: now,
            first_detected_version_id: None,
            resolved_at: Some(now),
            resolution_note: Some("ok".into()),
            resolution_version_a_id: Some(a),
            resolution_version_b_id: Some(b),
            created_at: now,
            updated_at: now,
            request_id: "req".into(),
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["resolutionVersionAId"], a.to_string());
        assert_eq!(json["resolutionVersionBId"], b.to_string());
    }

    #[test]
    fn body_limit_error_is_canonical_413_envelope() {
        let error = body_limit_error("req-413");
        assert_eq!(error.code, "payload_too_large");
        assert_eq!(error.request_id, "req-413");
    }
}
