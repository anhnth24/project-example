//! Vertical-slice / security release suite gate (P1B-O04).
//!
//! Default `cargo test` must not claim a green live E2E. The live suite is
//! `#[ignore]` and only runs with `--ignored` under `MARKHAND_E2E=1`.

#[test]
fn e2e_suite_default_is_not_run() {
    let status = std::fs::read_to_string("bench/markhand_web/reports/phase-1b-gate/summary.json")
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|value| {
            value
                .get("status")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        });
    match status.as_deref() {
        Some("pass") => {
            // Only acceptable when a live evidence run wrote pass.
            assert_eq!(
                std::env::var("MARKHAND_E2E").ok().as_deref(),
                Some("1"),
                "gate summary claims pass without MARKHAND_E2E=1"
            );
        }
        Some("not_run") | Some("incomplete") | None => {
            eprintln!("e2e_release_suite: status=not_run (no live evidence in this environment)");
        }
        other => panic!("unexpected gate status: {other:?}"),
    }
}

#[test]
#[ignore = "live Compose POC vertical slice; run with --ignored and MARKHAND_E2E=1"]
fn e2e_live_vertical_slice() {
    // Opt-in operator gate: only exercised under MARKHAND_E2E=1. Absent the flag
    // (e.g. the rust-integration CI job that runs `--include-ignored` with a live
    // database but no live-evidence opt-in), skip gracefully instead of failing —
    // mirroring the DB-gated tests' `else { return }` idiom, per this suite's own
    // "only runs with --ignored under MARKHAND_E2E=1" contract.
    if std::env::var("MARKHAND_E2E").ok().as_deref() != Some("1") {
        eprintln!("e2e_live_vertical_slice: skipped (set MARKHAND_E2E=1 for the live suite)");
        return;
    }
    let database = std::env::var("MARKHAND_TEST_DATABASE_URL")
        .expect("MARKHAND_E2E=1 requires MARKHAND_TEST_DATABASE_URL");
    assert!(
        database.starts_with("postgres"),
        "MARKHAND_TEST_DATABASE_URL must be a postgres URL"
    );
    // Live upload→citation matrix is operator-executed against Compose POC.
    // This ignored test only validates the opt-in contract; full matrix evidence
    // must be written under bench/markhand_web/reports/phase-1b-gate/.
    eprintln!("e2e_live_vertical_slice: opt-in contract ok; fill gate report from live run");
}
