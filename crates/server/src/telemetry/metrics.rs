//! Bounded metric name/label contracts for Phase 1B observability (P1B-O01).

use super::validate_metric;

/// Canonical metric names used across API → jobs → convert/embed/retrieval/GLM.
pub const METRIC_REQUEST_LATENCY: &str = "markhand_http_request_duration_seconds";
pub const METRIC_QUEUE_DEPTH: &str = "markhand_job_queue_depth";
pub const METRIC_QUEUE_AGE: &str = "markhand_job_queue_age_seconds";
pub const METRIC_CONVERSION: &str = "markhand_conversion_duration_seconds";
pub const METRIC_EMBEDDING_BATCH: &str = "markhand_embedding_batch_duration_seconds";
pub const METRIC_RETRIEVAL_LEG: &str = "markhand_retrieval_leg_duration_seconds";
pub const METRIC_DRIFT: &str = "markhand_reconcile_drift_total";
pub const METRIC_QUOTA: &str = "markhand_quota_reservation_total";
pub const METRIC_BACKUP_AGE: &str = "markhand_backup_age_seconds";

pub fn assert_safe_metric(name: &str, labels: &[&str]) -> Result<(), String> {
    validate_metric(name, labels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_metrics_pass_cardinality_policy() {
        assert!(assert_safe_metric(METRIC_REQUEST_LATENCY, &["route", "status"]).is_ok());
        assert!(assert_safe_metric(METRIC_QUEUE_DEPTH, &["job_type"]).is_ok());
        assert!(assert_safe_metric(METRIC_RETRIEVAL_LEG, &["leg", "outcome"]).is_ok());
        assert!(assert_safe_metric(METRIC_DRIFT, &["kind"]).is_ok());
        assert!(assert_safe_metric(METRIC_BACKUP_AGE, &["store"]).is_ok());
        assert!(assert_safe_metric(METRIC_QUEUE_DEPTH, &["org_id"]).is_err());
    }
}
