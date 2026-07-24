//! Vertical-slice / security release suite gate (P1B-O04).
//!
//! Python harness (`run_o04_release_suite.py`) is the source of truth for
//! evaluate_report. This Rust binary:
//! - asserts default evidence is honest `not_run` / non-pass
//! - refuses to treat O05 `summary.json` as O04 evidence
//! - under `MARKHAND_E2E=1` + `--ignored`, invokes Python `--validate-report`
//!
//! Integration CI that uses `--include-ignored` must `--skip e2e_live_vertical_slice`.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const O04_REPORT: &str = "bench/markhand_web/reports/phase-1b-gate/o04-release.json";
const O05_SUMMARY: &str = "bench/markhand_web/reports/phase-1b-gate/summary.json";
const O04_HARNESS: &str = "bench/markhand_web/scripts/run_o04_release_suite.py";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load_json(rel: &str) -> Option<Value> {
    let path = workspace_root().join(rel);
    let raw = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn status_of(value: &Value) -> Option<&str> {
    value.get("status").and_then(|s| s.as_str())
}

/// Invoke Python evaluator (source of truth). Returns (status, blockers).
fn validate_report_via_python(report_path: &Path) -> (String, Vec<String>, i32) {
    let harness = workspace_root().join(O04_HARNESS);
    let output = Command::new("python3")
        .arg(&harness)
        .arg("--validate-report")
        .arg(report_path)
        .current_dir(workspace_root())
        .output()
        .unwrap_or_else(|error| panic!("spawn python validator: {error}"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "python --validate-report returned non-JSON (exit {}): {error}; stdout={stdout}; stderr={}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    let status = parsed
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("fail")
        .to_string();
    let blockers = parsed
        .get("blockers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    (status, blockers, output.status.code().unwrap_or(1))
}

#[test]
fn e2e_suite_default_is_not_run() {
    let report = load_json(O04_REPORT).expect("o04-release.json must exist");
    assert_eq!(
        report.get("issue").and_then(|v| v.as_str()),
        Some("P1B-O04"),
        "O04 report issue field"
    );
    // Must not treat O05 soak summary as O04 evidence.
    if let Some(summary) = load_json(O05_SUMMARY) {
        assert_ne!(
            summary.get("issue").and_then(|v| v.as_str()),
            Some("P1B-O04"),
            "O05 summary.json must not be used as O04 release evidence"
        );
    }
    let path = workspace_root().join(O04_REPORT);
    let (status, blockers, _code) = validate_report_via_python(&path);
    match status.as_str() {
        "pass" => {
            assert_eq!(
                std::env::var("MARKHAND_E2E").ok().as_deref(),
                Some("1"),
                "o04-release.json claims pass without MARKHAND_E2E=1"
            );
        }
        "not_run" | "incomplete" | "fail" => {
            eprintln!("e2e_release_suite: status={status} blockers={blockers:?}");
        }
        other => panic!("unexpected o04 gate status: {other:?}"),
    }
    // Default committed evidence must remain honest non-pass.
    if std::env::var("MARKHAND_E2E").ok().as_deref() != Some("1") {
        assert_ne!(status, "pass", "default evidence must not be pass");
        assert_eq!(status_of(&report), Some("not_run"));
        assert!(
            blockers.iter().any(|b| b == "MARKHAND_E2E!=1"),
            "expected MARKHAND_E2E!=1 blocker, got {blockers:?}"
        );
    }
}

#[test]
fn o04_python_validator_rejects_non_pass_fixtures() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let raw = tmp.path().join("raw");
    std::fs::create_dir_all(&raw).unwrap();

    // Minimal not_run fixture: missing markhandE2e truth => not_run / non-pass.
    let not_run = serde_json::json!({
        "issue": "P1B-O04",
        "status": "not_run",
        "markhandE2e": false,
        "expectedFormats": [],
        "formatsObserved": [],
        "suites": {},
        "findings": [],
        "provenance": {},
        "redactionScan": {"passed": true, "findings": []},
        "rawDir": raw.to_string_lossy(),
        "blockers": ["MARKHAND_E2E!=1"],
        "architecture": {
            "kind": "in_process_workers_against_poc_services",
            "apiHttpExercised": false
        },
        "f02Boot": {"passed": false}
    });
    let path = tmp.path().join("not_run.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&not_run).unwrap()).unwrap();
    let (status, blockers, code) = validate_report_via_python(&path);
    assert_eq!(status, "not_run");
    assert_ne!(code, 0);
    assert!(
        blockers.iter().any(|b| b == "MARKHAND_E2E!=1"),
        "{blockers:?}"
    );

    // Partial format + e2e opted in => fail (not pass).
    let formats = ["csv", "docx", "html", "pdf", "png", "pptx", "txt", "xlsx"];
    let mut suites = serde_json::Map::new();
    for key in [
        "vertical_slice_formats",
        "unauthorized_cross_tenant",
        "suspend_membership_delete_deny",
        "adversarial_upload",
        "worker_kill_replay",
    ] {
        suites.insert(
            key.into(),
            serde_json::json!({
                "commands": [["cargo","test","-p","fileconv-server","--test","uploads","--","--nocapture"]],
                "command": ["cargo","test","-p","fileconv-server","--test","uploads","--","--nocapture"],
                "exitCode": 0,
                "testsRun": 1,
                "testsPassed": 1,
                "testsFailed": 0,
                "skipped": false,
                "ignored": false,
                "passed": true
            }),
        );
    }
    let image_ids = serde_json::json!({
        "api": "sha256:1",
        "minio": "sha256:2",
        "postgres": "sha256:3",
        "qdrant": "sha256:4",
        "worker-convert": "sha256:5",
        "worker-index": "sha256:6"
    });
    let partial = serde_json::json!({
        "issue": "P1B-O04",
        "status": "fail",
        "markhandE2e": true,
        "expectedFormats": formats,
        "formatsObserved": ["pdf", "txt"],
        "suites": suites,
        "findings": [],
        "provenance": {
            "gitSha": "abc",
            "gitShaFull": "abcdef",
            "dockerVersion": "Docker",
            "composeVersion": "Compose",
            "composeProject": "markhand-poc",
            "migrationManifestSha256": "a".repeat(64),
            "indexSignature": "b".repeat(64),
            "imageIds": image_ids,
            "imageDigests": {}
        },
        "redactionScan": {"passed": true, "findings": []},
        "rawDir": raw.to_string_lossy(),
        "blockers": [],
        "architecture": {
            "kind": "in_process_workers_against_poc_services",
            "apiHttpExercised": false
        },
        "f02Boot": {
            "passed": true,
            "composeProject": "markhand-poc",
            "imageIds": {
                "api": "sha256:1",
                "minio": "sha256:2",
                "postgres": "sha256:3",
                "qdrant": "sha256:4",
                "worker-convert": "sha256:5",
                "worker-index": "sha256:6"
            }
        }
    });
    let path = tmp.path().join("partial.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&partial).unwrap()).unwrap();
    let (status, blockers, code) = validate_report_via_python(&path);
    assert_ne!(status, "pass");
    assert_ne!(code, 0);
    assert!(
        blockers.iter().any(|b| b == "partial_format"),
        "{blockers:?}"
    );
}

#[test]
#[ignore = "live in-process workers against POC services; run with --ignored and MARKHAND_E2E=1 after o04 harness"]
fn e2e_live_vertical_slice() {
    assert_eq!(
        std::env::var("MARKHAND_E2E").ok().as_deref(),
        Some("1"),
        "e2e_live_vertical_slice requires MARKHAND_E2E=1; CI must --skip this test under plain --include-ignored"
    );
    let database = std::env::var("MARKHAND_TEST_DATABASE_URL")
        .expect("MARKHAND_E2E=1 requires MARKHAND_TEST_DATABASE_URL");
    assert!(
        database.starts_with("postgres"),
        "MARKHAND_TEST_DATABASE_URL must be a postgres URL"
    );

    let report =
        load_json(O04_REPORT).expect("o04-release.json missing — run run_o04_release_suite.py");
    assert_eq!(
        report.get("issue").and_then(|v| v.as_str()),
        Some("P1B-O04"),
        "live gate must read O04 report, not O05 summary.json"
    );
    assert_eq!(
        report
            .get("architecture")
            .and_then(|v| v.get("kind"))
            .and_then(|v| v.as_str()),
        Some("in_process_workers_against_poc_services")
    );
    assert_eq!(
        report
            .get("architecture")
            .and_then(|v| v.get("apiHttpExercised"))
            .and_then(|v| v.as_bool()),
        Some(false),
        "must not claim Compose API HTTP was exercised"
    );

    let path = workspace_root().join(O04_REPORT);
    let (status, blockers, code) = validate_report_via_python(&path);
    assert_eq!(
        (status.as_str(), code),
        ("pass", 0),
        "O04 live report failed Python provenance/coverage gates: {blockers:?}"
    );
    assert_eq!(status_of(&report), Some("pass"));

    let raw_dir = report
        .get("rawDir")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .expect("rawDir");
    assert!(
        raw_dir.is_dir()
            && raw_dir
                .read_dir()
                .ok()
                .map(|mut d| d.next().is_some())
                .unwrap_or(false),
        "raw evidence dir must contain suite logs"
    );
    eprintln!("e2e_live_vertical_slice: O04 report pass via Python validator");
}
