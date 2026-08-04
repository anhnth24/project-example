//! Structured Phase 1C deployed probe markers for the G1C harness.
//!
//! Emitted only when `MARKHAND_TEST_REQUIRED=1`. The Python harness parses
//! `PHASE1C_PROBE_RESULT` + `PHASE1C_PROBE_EOF` from cargo test stdout.

use serde_json::{json, Value};
use std::time::Instant;

pub const PROBE_SCHEMA_VERSION: i64 = 1;

pub fn probe_enabled() -> bool {
    std::env::var("MARKHAND_TEST_REQUIRED").ok().as_deref() == Some("1")
}

pub fn emit_probe_result(probe_id: &str, metrics: Value) {
    if !probe_enabled() {
        return;
    }
    let payload = json!({
        "schemaVersion": PROBE_SCHEMA_VERSION,
        "probeId": probe_id,
        "metrics": metrics,
    });
    println!(
        "PHASE1C_PROBE_RESULT\t{}",
        serde_json::to_string(&payload).expect("probe json")
    );
    println!("PHASE1C_PROBE_EOF\ttrue");
}

pub fn elapsed_ms(started: Instant) -> i64 {
    i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX)
}
