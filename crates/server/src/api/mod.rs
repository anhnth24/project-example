//! Versioned wire-contract types shared by routes, workers and fixtures.

mod error;
mod openapi;
mod pagination;
mod sse;
mod types;

pub use error::ApiError;
pub use openapi::{embedded_openapi_yaml, openapi_path_count};
pub use pagination::{decode_cursor, encode_cursor, PageInfo, Pagination};
pub use sse::SseEnvelope;
pub use types::*;

#[cfg(test)]
mod tests {
    use super::{ApiError, PageInfo, SseEnvelope};

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
}
