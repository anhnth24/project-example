//! Vertical-slice / security release suite gate (P1B-O04).
//!
//! Python harness (`run_o04_release_suite.py`) is the source of truth for
//! evaluate_report. This Rust binary:
//! - asserts default evidence is honest `not_run` / non-pass
//! - refuses to treat O05 `o05-soak.json` / `summary.json` as O04 evidence
//! - under `MARKHAND_RELEASE_GATE=1`, requires canonical O04 report pass
//!
//! Template/unit CI may run this test without live services; release-gate CI must
//! set `MARKHAND_RELEASE_GATE=1` after generating O04 evidence.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const O04_REPORT: &str = "bench/markhand_web/reports/phase-1b-gate/o04-release.json";
const O05_REPORT: &str = "bench/markhand_web/reports/phase-1b-gate/o05-soak.json";
const O05_SUMMARY: &str = "bench/markhand_web/reports/phase-1b-gate/summary.json";
const O04_HARNESS: &str = "bench/markhand_web/scripts/run_o04_release_suite.py";
const O05_HARNESS: &str = "bench/markhand_web/soak/run_soak.py";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load_json(rel: &str) -> Option<Value> {
    let path = workspace_root().join(rel);
    let raw = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn o04_report_path() -> PathBuf {
    std::env::var("O04_REPORT_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| workspace_root().join(O04_REPORT))
}

fn load_json_path(path: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn status_of(value: &Value) -> Option<&str> {
    value.get("status").and_then(|s| s.as_str())
}

fn python_exe() -> &'static str {
    if Command::new("python3").arg("--version").output().is_ok() {
        "python3"
    } else {
        "python"
    }
}

/// Invoke Python evaluator (source of truth). Returns (status, blockers).
fn validate_report_via_python(report_path: &Path) -> (String, Vec<String>, i32) {
    let harness = workspace_root().join(O04_HARNESS);
    let output = Command::new(python_exe())
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
    // Must not treat O05 soak artifacts as O04 evidence.
    let o05 = load_json(O05_REPORT).expect("o05-soak.json must exist");
    assert_eq!(
        o05.get("issue").and_then(|v| v.as_str()),
        Some("P1B-O05"),
        "canonical O05 report issue field"
    );
    assert_ne!(
        status_of(&o05),
        Some("pass"),
        "default committed O05 evidence must not claim pass without live soak"
    );
    assert!(
        matches!(status_of(&o05), Some("not_run" | "incomplete")),
        "committed O05 evidence must remain honest non-qualifying"
    );
    if let Some(summary) = load_json(O05_SUMMARY) {
        assert_ne!(
            summary.get("issue").and_then(|v| v.as_str()),
            Some("P1B-O04"),
            "O05 summary.json must not be used as O04 release evidence"
        );
        assert_eq!(
            summary.get("issue").and_then(|v| v.as_str()),
            Some("P1B-O05"),
            "summary.json pointer must remain O05-owned"
        );
    }
    // O05 harness must exist and refuse pass without MARKHAND_SOAK=1.
    let o05_harness = workspace_root().join(O05_HARNESS);
    assert!(o05_harness.is_file(), "missing O05 harness {O05_HARNESS}");
    let path = o04_report_path();
    let (status, blockers, _code) = validate_report_via_python(&path);
    match status.as_str() {
        "pass" => {
            assert_eq!(
                report.get("markhandE2e").and_then(|value| value.as_bool()),
                Some(true),
                "o04-release.json claims pass without recorded MARKHAND_E2E opt-in"
            );
        }
        "not_run" | "incomplete" | "fail" => {
            eprintln!("e2e_release_suite: status={status} blockers={blockers:?}");
        }
        other => panic!("unexpected o04 gate status: {other:?}"),
    }
}

#[test]
fn o04_release_gate_requires_canonical_pass_when_enabled() {
    if std::env::var("MARKHAND_RELEASE_GATE").ok().as_deref() != Some("1") {
        eprintln!(
            "o04_release_gate_requires_canonical_pass_when_enabled: \
             template mode; set MARKHAND_RELEASE_GATE=1 after O04 live run"
        );
        return;
    }

    let path = o04_report_path();
    let report = load_json_path(&path).expect("o04-release.json must exist");
    let github_sha = std::env::var("GITHUB_SHA")
        .expect("MARKHAND_RELEASE_GATE=1 requires explicit GITHUB_SHA binding");
    assert!(
        !github_sha.trim().is_empty(),
        "MARKHAND_RELEASE_GATE=1 requires non-empty GITHUB_SHA"
    );
    assert_eq!(
        report
            .get("provenance")
            .and_then(|v| v.get("gitShaFull"))
            .and_then(|v| v.as_str()),
        Some(github_sha.as_str()),
        "O04 report must be bound to the exact GITHUB_SHA"
    );
    let (status, blockers, code) = validate_report_via_python(&path);
    assert_eq!(
        (status.as_str(), code),
        ("pass", 0),
        "MARKHAND_RELEASE_GATE=1 requires canonical O04 report pass; report_status={:?}; blockers={blockers:?}",
        status_of(&report)
    );
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
