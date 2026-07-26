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
use sha2::{Digest, Sha256};

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

/// Assert a committed `o05-soak.json` that claims pass is bound to the live run
/// that produced it. Mirrors the O04 rule below, where a committed pass is
/// accepted only with the recorded `markhandE2e` opt-in.
fn assert_committed_o05_pass_is_attested(o05: &Value) {
    let flag = |key: &str| o05.get(key).and_then(Value::as_bool);
    assert_eq!(
        flag("markhandSoak"),
        Some(true),
        "o05-soak.json claims pass without the recorded MARKHAND_SOAK opt-in"
    );
    assert_eq!(
        flag("smoke"),
        Some(false),
        "o05-soak.json claims pass from a smoke run"
    );
    assert_eq!(
        flag("smokeNonQualifying"),
        Some(false),
        "o05-soak.json claims pass while flagged non-qualifying"
    );
    for key in ["blockers", "architecturalBlockers"] {
        let blockers = o05
            .get(key)
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("o05-soak.json claims pass without a {key} array"));
        assert!(
            blockers.is_empty(),
            "o05-soak.json claims pass with {key}: {blockers:?}"
        );
    }
    let seconds = |key: &str| o05.get(key).and_then(Value::as_u64);
    let official = seconds("officialDurationSeconds")
        .expect("o05-soak.json claims pass without officialDurationSeconds");
    let measured =
        seconds("durationSeconds").expect("o05-soak.json claims pass without durationSeconds");
    assert!(
        measured >= official,
        "o05-soak.json claims pass from {measured}s against an official {official}s run"
    );
    let git_sha = o05
        .get("versions")
        .and_then(|versions| versions.get("gitShaFull"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        git_sha.len() == 40 && git_sha.chars().all(|c| c.is_ascii_hexdigit()),
        "o05-soak.json claims pass without a full commit binding; gitShaFull={git_sha:?}"
    );

    // The raw evidence must travel with the claim, not stay on the run host.
    let raw_dir = o05
        .get("rawDir")
        .and_then(Value::as_str)
        .expect("o05-soak.json claims pass without a rawDir");
    let raw_root = workspace_root().join(raw_dir);
    assert!(
        raw_root.is_dir(),
        "o05-soak.json claims pass but its raw evidence is not committed at {raw_dir}"
    );
    let files = o05
        .get("rawManifest")
        .and_then(|manifest| manifest.get("files"))
        .and_then(Value::as_array)
        .expect("o05-soak.json claims pass without a rawManifest.files array");
    assert!(
        !files.is_empty(),
        "o05-soak.json claims pass with an empty rawManifest"
    );
    for entry in files {
        let path = entry
            .get("path")
            .and_then(Value::as_str)
            .expect("rawManifest entry without a path");
        let expected = entry
            .get("sha256")
            .and_then(Value::as_str)
            .expect("rawManifest entry without a sha256");
        let bytes = std::fs::read(raw_root.join(path)).unwrap_or_else(|error| {
            panic!("rawManifest lists {path}, which is not committed under {raw_dir}: {error}")
        });
        let actual = hex::encode(Sha256::digest(&bytes));
        assert_eq!(
            actual, expected,
            "committed raw evidence {path} does not match the manifest sha256"
        );
    }
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
    match status_of(&o05) {
        // A committed O05 pass is only as good as the live run behind it, so it
        // must carry that run's provenance: the soak opt-in, a full-length
        // non-smoke run, no blockers, a full git sha, and a raw evidence
        // directory committed alongside whose every manifest entry still
        // hashes. Anything short of that is a pass nobody can re-check, which
        // is what forbidding pass outright used to prevent.
        Some("pass") => assert_committed_o05_pass_is_attested(&o05),
        Some("not_run" | "incomplete") => {}
        other => panic!(
            "committed O05 evidence must be pass with attested provenance, \
             or an honest non-qualifying not_run/incomplete; got {other:?}"
        ),
    }
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
