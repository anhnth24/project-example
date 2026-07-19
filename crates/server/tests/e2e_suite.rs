//! P1B-O04 live end-to-end release/security suite.
//!
//! The tests skip cleanly unless `MARKHAND_TEST_DATABASE_URL`,
//! `MARKHAND_TEST_QDRANT_URL`, and `MARKHAND_TEST_MINIO_*` are set.

#[path = "e2e/harness.rs"]
mod harness;

#[path = "e2e/scenarios.rs"]
mod scenarios;
