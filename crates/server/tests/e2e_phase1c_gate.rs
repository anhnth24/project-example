//! Phase 1C G1C-SEC deployed qualification gate.
//!
//! Python harness (`run_phase1c_gate.py`) is the source of truth for
//! evaluate_report. This Rust binary:
//! - asserts default evidence is honest `not_run` / non-pass
//! - under `MARKHAND_PHASE1C_GATE=1`, requires canonical report pass
//!
//! Template/unit CI may run this test without live services; the dedicated
//! G1C job must set `MARKHAND_PHASE1C_GATE=1` after generating evidence.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const PHASE1C_REPORT: &str = "bench/markhand_web/reports/phase-1c-gate/phase-1c-gate.json";
const PHASE1C_TEMPLATE: &str = "bench/markhand_web/reports/phase-1c-gate/phase-1c-gate.template.json";
const PHASE1C_HARNESS: &str = "bench/markhand_web/scripts/run_phase1c_gate.py";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load_json(rel: &str) -> Option<Value> {
    let path = workspace_root().join(rel);
    let raw = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn phase1c_report_path() -> PathBuf {
    std::env::var("MARKHAND_PHASE1C_REPORT_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| workspace_root().join(PHASE1C_REPORT))
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

/// Invoke Python evaluator (source of truth). Returns (status, blockers, exit_code).
fn validate_report_via_python(report_path: &Path) -> (String, Vec<String>, i32) {
    let harness = workspace_root().join(PHASE1C_HARNESS);
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
fn e2e_phase1c_gate_default_is_not_run() {
    let template = load_json(PHASE1C_TEMPLATE).expect("phase-1c-gate.template.json must exist");
    assert_eq!(
        template.get("command").and_then(|v| v.as_str()),
        Some("python3 bench/markhand_web/scripts/run_phase1c_gate.py"),
        "template command binding"
    );
    assert_eq!(
        status_of(&template),
        Some("not_run"),
        "committed template must be honest not_run"
    );
    assert_eq!(
        template.get("targetMatch").and_then(Value::as_bool),
        Some(false),
        "template must not claim targetMatch"
    );

    let path = phase1c_report_path();
    if path.ends_with("phase-1c-gate.template.json") {
        let (status, blockers, _code) = validate_report_via_python(&path);
        assert_eq!(status, "not_run");
        assert!(
            blockers.iter().any(|b| b.contains("harness_not_implemented") || b.contains("template")),
            "template validation blockers: {blockers:?}"
        );
        return;
    }

    let report = load_json_path(&path).unwrap_or(template);
    let (status, blockers, _code) = validate_report_via_python(&path);
    match status.as_str() {
        "pass" => {
            assert_eq!(
                report.get("markhandPhase1cGate").and_then(Value::as_bool),
                Some(true),
                "phase-1c-gate.json claims pass without recorded MARKHAND_PHASE1C_GATE opt-in"
            );
        }
        "not_run" | "incomplete" | "fail" => {
            eprintln!("e2e_phase1c_gate: status={status} blockers={blockers:?}");
        }
        other => panic!("unexpected phase1c gate status: {other:?}"),
    }
}

#[test]
#[ignore = "requires MARKHAND_PHASE1C_GATE=1 and live deployed evidence"]
fn e2e_phase1c_gate() {
    if std::env::var("MARKHAND_PHASE1C_GATE").ok().as_deref() != Some("1") {
        eprintln!(
            "e2e_phase1c_gate: template mode; set MARKHAND_PHASE1C_GATE=1 after live G1C run"
        );
        return;
    }

    let path = phase1c_report_path();
    let report = load_json_path(&path).expect("phase-1c-gate.json must exist");
    let github_sha = std::env::var("GITHUB_SHA")
        .expect("MARKHAND_PHASE1C_GATE=1 requires explicit GITHUB_SHA binding");
    assert!(
        !github_sha.trim().is_empty(),
        "MARKHAND_PHASE1C_GATE=1 requires non-empty GITHUB_SHA"
    );
    assert_eq!(
        report
            .get("git")
            .and_then(|v| v.get("commit"))
            .and_then(|v| v.as_str()),
        Some(github_sha.as_str()),
        "Phase 1C report must be bound to the exact GITHUB_SHA"
    );
    let (status, blockers, code) = validate_report_via_python(&path);
    assert_eq!(
        (status.as_str(), code),
        ("pass", 0),
        "MARKHAND_PHASE1C_GATE=1 requires canonical Phase 1C report pass; report_status={:?}; blockers={blockers:?}",
        status_of(&report)
    );
}

#[test]
fn phase1c_python_validator_rejects_non_pass_fixtures() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let not_run = serde_json::json!({
        "version": 1,
        "reportId": "phase1c-not-run-fixture",
        "generatedAt": "2026-08-04T00:00:00Z",
        "status": "not_run",
        "command": "python3 bench/markhand_web/scripts/run_phase1c_gate.py",
        "environmentId": "phase1c-multi-org-poc",
        "workloadProfileId": "phase1c-multi-org",
        "targetMatch": false,
        "denialManifestSha256": "3f429ecc1262a4696572eea854f0ec7c82b541a8f174884c613dc2a1974c1e48",
        "git": {"commit": "0000000000000000000000000000000000000000", "dirty": false},
        "metrics": {},
        "thresholdDecisions": [],
        "workerProof": {
            "runtimeRole": "markhand_worker",
            "dedicatedDatabaseUrlVerified": false,
            "superuser": false,
            "bypassRls": false,
            "verifiedAt": "2026-08-04T00:00:00Z"
        },
        "gateResults": [],
        "redactionScan": {"passed": false},
        "vulnerabilityScan": {
            "scanner": "trivy",
            "undispositionedHighCritical": 0,
            "findings": [],
            "passed": false
        },
        "canonicalBinding": {
            "registryRevision": 1,
            "environmentSha256": "0".repeat(64),
            "workloadSha256": "0".repeat(64),
            "gatesSha256": "0".repeat(64),
            "slaSha256": "0".repeat(64),
            "thresholdDecisionsSha256": "0".repeat(64)
        }
    });
    let path = tmp.path().join("not_run.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&not_run).unwrap()).unwrap();
    let (status, blockers, code) = validate_report_via_python(&path);
    assert_eq!(status, "not_run");
    assert_ne!(code, 0);
    assert!(
        !blockers.is_empty(),
        "malformed not_run fixture must fail closed: {blockers:?}"
    );
}
