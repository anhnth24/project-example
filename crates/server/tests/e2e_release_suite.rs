//! Vertical-slice / security release suite gate (P1B-O04).
//!
//! Default `cargo test` must not claim a green live E2E. Evidence lives in
//! `bench/markhand_web/reports/phase-1b-gate/o04-release.json` (never O05
//! `summary.json`). The live suite is `#[ignore]` and only runs with
//! `--ignored` under `MARKHAND_E2E=1`. Integration CI that uses
//! `--include-ignored` must `--skip e2e_live_vertical_slice` so absence of the
//! live opt-in stays an honest not_run, not a fake pass.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

const O04_REPORT: &str = "bench/markhand_web/reports/phase-1b-gate/o04-release.json";
const O05_SUMMARY: &str = "bench/markhand_web/reports/phase-1b-gate/summary.json";

const EXPECTED_FORMATS: &[&str] = &["csv", "docx", "html", "pdf", "pptx", "txt", "xlsx"];

const REQUIRED_SUITES: &[&str] = &[
    "vertical_slice_formats",
    "unauthorized_cross_tenant",
    "suspend_membership_delete_deny",
    "adversarial_upload",
    "worker_kill_replay",
];

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

fn string_set(value: &Value, key: &str) -> BTreeSet<String> {
    value
        .get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Schema/validator: missing/skipped/ignored/zero-test/partial/high-critical => non-pass.
fn evaluate_o04_report(report: &Value, raw_exists: bool) -> (String, Vec<String>) {
    let mut blockers = Vec::new();
    for key in [
        "issue",
        "status",
        "markhandE2e",
        "expectedFormats",
        "formatsObserved",
        "suites",
        "findings",
        "provenance",
        "redactionScan",
        "rawDir",
        "blockers",
    ] {
        if report.get(key).is_none() {
            blockers.push(format!("missing:{key}"));
        }
    }
    if report.get("issue").and_then(|v| v.as_str()) != Some("P1B-O04") {
        blockers.push("issue_mismatch".into());
    }
    if report.get("markhandE2e").and_then(|v| v.as_bool()) != Some(true) {
        blockers.push("MARKHAND_E2E!=1".into());
        return ("not_run".into(), blockers);
    }

    let expected = string_set(report, "expectedFormats");
    let want: BTreeSet<String> = EXPECTED_FORMATS.iter().map(|s| (*s).to_string()).collect();
    if expected != want {
        blockers.push("expected_formats_mismatch".into());
    }
    let observed = string_set(report, "formatsObserved");
    if observed != expected {
        blockers.push("partial_format".into());
    }

    let suites = report.get("suites").and_then(|v| v.as_object());
    for suite_key in REQUIRED_SUITES {
        let Some(suite) = suites.and_then(|m| m.get(*suite_key)) else {
            blockers.push(format!("missing_suite:{suite_key}"));
            continue;
        };
        if suite.get("skipped").and_then(|v| v.as_bool()) == Some(true) {
            blockers.push(format!("skipped:{suite_key}"));
        }
        if suite.get("ignored").and_then(|v| v.as_bool()) == Some(true) {
            blockers.push(format!("ignored:{suite_key}"));
        }
        let tests_run = suite.get("testsRun").and_then(|v| v.as_u64()).unwrap_or(0);
        if tests_run == 0 {
            blockers.push(format!("zero_test:{suite_key}"));
        }
        if suite.get("exitCode").and_then(|v| v.as_i64()) != Some(0) {
            blockers.push(format!("exit:{suite_key}"));
        }
        if suite.get("passed").and_then(|v| v.as_bool()) != Some(true) {
            blockers.push(format!("failed:{suite_key}"));
        }
    }

    if let Some(findings) = report.get("findings").and_then(|v| v.as_array()) {
        for finding in findings {
            let sev = finding
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if sev == "high" || sev == "critical" {
                blockers.push(format!("finding:{sev}"));
            }
        }
    }

    let prov = report.get("provenance").and_then(|v| v.as_object());
    for key in [
        "gitSha",
        "gitShaFull",
        "dockerVersion",
        "composeVersion",
        "migrationManifestSha256",
        "indexSignature",
        "imageIds",
        "imageDigests",
    ] {
        let missing = match prov.and_then(|m| m.get(key)) {
            None => true,
            Some(Value::Null) => true,
            Some(Value::String(s)) if s.is_empty() => true,
            Some(Value::Object(o)) if o.is_empty() => true,
            Some(Value::Array(a)) if a.is_empty() => true,
            _ => false,
        };
        if missing {
            blockers.push(format!("provenance_missing:{key}"));
        }
    }

    if report
        .get("rawDir")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .is_none()
        || !raw_exists
    {
        blockers.push("raw_dir_missing".into());
    }

    if report
        .get("redactionScan")
        .and_then(|v| v.get("passed"))
        .and_then(|v| v.as_bool())
        != Some(true)
    {
        blockers.push("redaction_failed".into());
    }

    blockers.sort();
    blockers.dedup();
    if blockers.is_empty() {
        ("pass".into(), blockers)
    } else {
        ("fail".into(), blockers)
    }
}

fn sample_good_report(raw_dir: &str) -> Value {
    let mut suites = serde_json::Map::new();
    for key in REQUIRED_SUITES {
        suites.insert(
            (*key).into(),
            serde_json::json!({
                "command": ["cargo", "test"],
                "exitCode": 0,
                "testsRun": 2,
                "testsPassed": 2,
                "testsFailed": 0,
                "skipped": false,
                "ignored": false,
                "passed": true,
                "rawLog": "x.txt"
            }),
        );
    }
    serde_json::json!({
        "issue": "P1B-O04",
        "status": "pass",
        "markhandE2e": true,
        "expectedFormats": EXPECTED_FORMATS,
        "formatsObserved": EXPECTED_FORMATS,
        "suites": suites,
        "findings": [],
        "provenance": {
            "gitSha": "abc1234",
            "gitShaFull": "abc1234deadbeef",
            "dockerVersion": "Docker version 29",
            "composeVersion": "Docker Compose version 2",
            "migrationManifestSha256": "a".repeat(64),
            "indexSignature": "idx-sig",
            "imageIds": {"api": "sha256:1"},
            "imageDigests": {"api": "repo@sha256:2"}
        },
        "redactionScan": {"passed": true, "findings": []},
        "rawDir": raw_dir,
        "blockers": []
    })
}

#[test]
fn e2e_suite_default_is_not_run() {
    let report = load_json(O04_REPORT);
    let status = report.as_ref().and_then(status_of);
    match status {
        Some("pass") => {
            assert_eq!(
                std::env::var("MARKHAND_E2E").ok().as_deref(),
                Some("1"),
                "o04-release.json claims pass without MARKHAND_E2E=1"
            );
        }
        Some("not_run") | Some("incomplete") | Some("fail") | None => {
            eprintln!("e2e_release_suite: status={status:?} (honest non-pass default)");
        }
        other => panic!("unexpected o04 gate status: {other:?}"),
    }

    // Must not treat O05 soak summary as O04 evidence.
    if let Some(summary) = load_json(O05_SUMMARY) {
        assert_ne!(
            summary.get("issue").and_then(|v| v.as_str()),
            Some("P1B-O04"),
            "O05 summary.json must not be used as O04 release evidence"
        );
    }
    if let Some(report) = report {
        assert_eq!(
            report.get("issue").and_then(|v| v.as_str()),
            Some("P1B-O04"),
            "O04 report issue field"
        );
    }
}

#[test]
fn o04_validator_rejects_missing_skipped_ignored_zero_partial_and_critical() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let raw = tmp.path().join("raw");
    std::fs::create_dir_all(&raw).unwrap();
    let raw_s = raw.to_string_lossy().into_owned();

    let good = sample_good_report(&raw_s);
    let (status, blockers) = evaluate_o04_report(&good, true);
    assert_eq!(status, "pass", "{blockers:?}");

    let mut missing = good.clone();
    missing.as_object_mut().unwrap().remove("suites");
    let (status, blockers) = evaluate_o04_report(&missing, true);
    assert_ne!(status, "pass");
    assert!(
        blockers.iter().any(|b| b.starts_with("missing:")),
        "{blockers:?}"
    );

    let mut skipped = good.clone();
    skipped["suites"]["vertical_slice_formats"]["skipped"] = Value::Bool(true);
    skipped["suites"]["vertical_slice_formats"]["passed"] = Value::Bool(false);
    let (status, blockers) = evaluate_o04_report(&skipped, true);
    assert_ne!(status, "pass");
    assert!(blockers
        .iter()
        .any(|b| b == "skipped:vertical_slice_formats"));

    let mut ignored = good.clone();
    ignored["suites"]["worker_kill_replay"]["ignored"] = Value::Bool(true);
    ignored["suites"]["worker_kill_replay"]["passed"] = Value::Bool(false);
    let (status, blockers) = evaluate_o04_report(&ignored, true);
    assert_ne!(status, "pass");
    assert!(blockers.iter().any(|b| b == "ignored:worker_kill_replay"));

    let mut zero = good.clone();
    zero["suites"]["adversarial_upload"]["testsRun"] = Value::from(0);
    zero["suites"]["adversarial_upload"]["passed"] = Value::Bool(false);
    let (status, blockers) = evaluate_o04_report(&zero, true);
    assert_ne!(status, "pass");
    assert!(blockers.iter().any(|b| b == "zero_test:adversarial_upload"));

    let mut partial = good.clone();
    partial["formatsObserved"] = serde_json::json!(["pdf", "txt"]);
    let (status, blockers) = evaluate_o04_report(&partial, true);
    assert_ne!(status, "pass");
    assert!(blockers.iter().any(|b| b == "partial_format"));

    let mut critical = good.clone();
    critical["findings"] = serde_json::json!([{ "severity": "critical", "id": "x" }]);
    let (status, blockers) = evaluate_o04_report(&critical, true);
    assert_ne!(status, "pass");
    assert!(blockers.iter().any(|b| b == "finding:critical"));

    let mut not_run = good.clone();
    not_run["markhandE2e"] = Value::Bool(false);
    let (status, _) = evaluate_o04_report(&not_run, true);
    assert_eq!(status, "not_run");
}

#[test]
#[ignore = "live Compose POC vertical slice; run with --ignored and MARKHAND_E2E=1 after o04 harness"]
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
    // Provenance / coverage verification against O04 evidence only.
    let raw_dir = report
        .get("rawDir")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .expect("rawDir");
    let raw_exists = raw_dir.is_dir();
    let (evaluated, blockers) = evaluate_o04_report(&report, raw_exists);
    assert_eq!(
        evaluated, "pass",
        "O04 live report failed provenance/coverage gates: {blockers:?}"
    );
    assert_eq!(status_of(&report), Some("pass"));
    assert_eq!(
        report.get("markhandE2e").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        string_set(&report, "expectedFormats"),
        string_set(&report, "formatsObserved")
    );
    let want: BTreeSet<String> = EXPECTED_FORMATS.iter().map(|s| (*s).to_string()).collect();
    assert_eq!(string_set(&report, "formatsObserved"), want);

    let prov = report.get("provenance").expect("provenance");
    for key in [
        "gitSha",
        "gitShaFull",
        "dockerVersion",
        "composeVersion",
        "migrationManifestSha256",
        "indexSignature",
    ] {
        let value = prov.get(key).and_then(|v| v.as_str()).unwrap_or("");
        assert!(!value.is_empty(), "provenance.{key} required");
    }
    assert!(
        prov.get("imageIds")
            .and_then(|v| v.as_object())
            .is_some_and(|m| !m.is_empty())
            || prov
                .get("imageDigests")
                .and_then(|v| v.as_object())
                .is_some_and(|m| !m.is_empty()),
        "image ids/digests required"
    );
    assert!(
        report
            .get("redactionScan")
            .and_then(|v| v.get("passed"))
            .and_then(|v| v.as_bool())
            == Some(true)
    );
    assert!(
        Path::new(&raw_dir)
            .join("vertical_slice_formats.txt")
            .is_file()
            || raw_dir
                .read_dir()
                .ok()
                .map(|mut d| d.next().is_some())
                .unwrap_or(false),
        "raw evidence dir must contain suite logs"
    );
    eprintln!("e2e_live_vertical_slice: O04 report pass + provenance/coverage verified");
}
