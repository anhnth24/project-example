#!/usr/bin/env python3
"""Unit / hermetic tests for P1B-O05 measured soak harness (TDD)."""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

SOAK_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SOAK_DIR))

import gates_eval  # noqa: E402
import injection  # noqa: E402
import mathutil  # noqa: E402
import prerequisites  # noqa: E402
import profile  # noqa: E402
import redact  # noqa: E402
import report  # noqa: E402

ROOT = Path(__file__).resolve().parents[3]
WORKLOAD = ROOT / "bench/markhand_web/workloads/phase1b-mixed.yaml"
GATES = ROOT / "bench/markhand_web/gates.yaml"


class PercentileMathTests(unittest.TestCase):
    def test_percentile_boundaries(self) -> None:
        samples = [10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0]
        self.assertEqual(mathutil.percentile(samples, 50), 55.0)
        self.assertEqual(mathutil.percentile(samples, 95), 95.5)
        self.assertEqual(mathutil.percentile(samples, 99), 99.1)

    def test_percentile_empty_is_none(self) -> None:
        self.assertIsNone(mathutil.percentile([], 95))

    def test_percentile_single(self) -> None:
        self.assertEqual(mathutil.percentile([42.0], 99), 42.0)


class RateScheduleTests(unittest.TestCase):
    def test_schedule_respects_rps_over_window(self) -> None:
        times = mathutil.schedule_event_times(rps=2.0, duration_seconds=5.0, seed=7)
        self.assertGreaterEqual(len(times), 9)
        self.assertLessEqual(len(times), 11)
        self.assertEqual(times, sorted(times))
        self.assertGreaterEqual(times[0], 0.0)
        self.assertLess(times[-1], 5.0)

    def test_zero_rps_yields_empty(self) -> None:
        self.assertEqual(mathutil.schedule_event_times(rps=0.0, duration_seconds=10.0), [])


class ProfileParseTests(unittest.TestCase):
    def test_loads_phase1b_mixed(self) -> None:
        loaded = profile.load_workload_profile(WORKLOAD)
        self.assertEqual(loaded["name"], "phase1b-mixed")
        self.assertEqual(loaded["durationSeconds"], 1800)
        self.assertEqual(
            sorted(loaded["actors"]["ingest"]["formats"]),
            ["csv", "docx", "html", "pdf", "png", "pptx", "txt", "xlsx"],
        )
        self.assertEqual(loaded["bounds"]["maxRssGrowthMb"], 256)
        self.assertEqual(loaded["failureInjection"]["killWorkerEverySeconds"], 600)


class ThresholdBoundaryTests(unittest.TestCase):
    def test_thresholds_from_profile_gates_sla(self) -> None:
        loaded = profile.load_workload_profile(WORKLOAD)
        thr = gates_eval.load_thresholds(loaded, GATES)
        self.assertEqual(thr["queryP95Ms"], 500)
        self.assertEqual(thr["queryP99Ms"], 1000)
        self.assertEqual(thr["ingestDocsPerHour"], 1200)
        self.assertTrue(thr["ingestGateBinding"])
        self.assertEqual(thr["maxRssGrowthMb"], 256)
        self.assertEqual(thr["maxTempGrowthMb"], 512)
        self.assertEqual(thr["maxQueueDepth"], 100)
        self.assertEqual(thr["maxDbConnections"], 40)

    def test_evaluate_pass_at_exact_boundaries(self) -> None:
        thr = {
            "queryP95Ms": 500,
            "queryP99Ms": 1000,
            "ingestDocsPerHour": 1200,
            "ingestGateBinding": True,
            "maxRssGrowthMb": 256,
            "maxTempGrowthMb": 512,
            "maxQueueDepth": 100,
            "maxDbConnections": 40,
            "officialDurationSeconds": 1800,
        }
        metrics = {
            "queryP50Ms": 100.0,
            "queryP95Ms": 500.0,
            "queryP99Ms": 1000.0,
            "ingestDocsPerHour": 1200.0,
            "rssGrowthMb": 256.0,
            "tempGrowthMb": 512.0,
            "queueDepthMax": 100,
            "dbConnectionsMax": 40,
            "durationSeconds": 1800,
            "smoke": False,
            "workerRecoveryPass": True,
            "dependencyRecoveryPass": True,
            "postRestoreRetrievalPass": True,
            "requestErrors": 0,
        }
        gates = gates_eval.evaluate_numeric_gates(metrics, thr)
        self.assertTrue(all(v == "pass" for v in gates.values()), gates)

    def test_evaluate_fail_just_over_boundaries(self) -> None:
        thr = {
            "queryP95Ms": 500,
            "queryP99Ms": 1000,
            "ingestDocsPerHour": 1200,
            "ingestGateBinding": True,
            "maxRssGrowthMb": 256,
            "maxTempGrowthMb": 512,
            "maxQueueDepth": 100,
            "maxDbConnections": 40,
            "officialDurationSeconds": 1800,
        }
        metrics = {
            "queryP50Ms": 100.0,
            "queryP95Ms": 500.1,
            "queryP99Ms": 1000.0,
            "ingestDocsPerHour": 1200.0,
            "rssGrowthMb": 256.0,
            "tempGrowthMb": 512.0,
            "queueDepthMax": 100,
            "dbConnectionsMax": 40,
            "durationSeconds": 1800,
            "smoke": False,
            "workerRecoveryPass": True,
            "dependencyRecoveryPass": True,
            "postRestoreRetrievalPass": True,
            "requestErrors": 0,
        }
        gates = gates_eval.evaluate_numeric_gates(metrics, thr)
        self.assertEqual(gates["queryP95"], "fail")


class SmokeCannotPassTests(unittest.TestCase):
    def test_smoke_duration_never_pass(self) -> None:
        thr = gates_eval.load_thresholds(profile.load_workload_profile(WORKLOAD), GATES)
        metrics = {
            "queryP50Ms": 10.0,
            "queryP95Ms": 10.0,
            "queryP99Ms": 10.0,
            "ingestDocsPerHour": 99999.0,
            "rssGrowthMb": 1.0,
            "tempGrowthMb": 1.0,
            "queueDepthMax": 1,
            "dbConnectionsMax": 1,
            "durationSeconds": 5,
            "smoke": True,
            "workerRecoveryPass": True,
            "dependencyRecoveryPass": True,
            "postRestoreRetrievalPass": True,
            "requestErrors": 0,
        }
        status, blockers = report.evaluate_status(
            markhand_soak=True,
            prerequisites_ok=True,
            measured=True,
            smoke=True,
            gates=gates_eval.evaluate_numeric_gates(metrics, thr),
            injection_ok=True,
            redaction_ok=True,
            duration_seconds=5,
            official_duration=1800,
        )
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("smoke" in b for b in blockers), blockers)


class MissingPrerequisiteTests(unittest.TestCase):
    def test_missing_f02_incomplete(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            result = prerequisites.validate_prerequisites(
                f02_path=out / "missing-f02.json",
                o02_path=out / "missing-o02.json",
                o03_path=out / "missing-o03.json",
                o04_path=out / "missing-o04.json",
                current_git_full="abc",
                compose_project="markhand-poc",
            )
            self.assertFalse(result["ok"])
            self.assertTrue(any("f02" in b for b in result["blockers"]), result)


class StaleEvidenceTests(unittest.TestCase):
    def test_git_mismatch_is_stale(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            f02 = base / "f02.json"
            o02 = base / "o02.json"
            o03 = base / "o03.json"
            o04 = base / "o04.json"
            raw = base / "raw"
            raw.mkdir()
            (raw / "x.txt").write_text("ok\n", encoding="utf-8")
            image_ids = {
                svc: f"sha256:{i:064d}"
                for i, svc in enumerate(
                    ["api", "minio", "postgres", "qdrant", "worker-convert", "worker-index"]
                )
            }
            f02.write_text(
                json.dumps(
                    {
                        "issue": "P1B-F02",
                        "passed": True,
                        "composeProject": "markhand-poc",
                        "imageIds": image_ids,
                        "containerIds": image_ids,
                        "gitShaFull": "deadbeef",
                        "rawDir": str(raw),
                        "provenance": {"gitShaFull": "deadbeef"},
                    }
                ),
                encoding="utf-8",
            )
            o02.write_text(
                json.dumps(
                    {
                        "issue": "P1B-O02",
                        "status": "pass",
                        "failCount": 0,
                        "passCount": 3,
                        "liveFaultExecuted": True,
                        "transitions": {"MarkhandDependencyDown": {"ok": True}},
                        "rawDir": str(raw),
                        "provenance": {"gitShaFull": "deadbeef"},
                    }
                ),
                encoding="utf-8",
            )
            o03.write_text(
                json.dumps(
                    {
                        "issue": "P1B-O03",
                        "status": "pass",
                        "consistencyRpoPass": True,
                        "queryReadyRtoPass": True,
                        "rpoSecondsMeasured": 60,
                        "queryReadyRtoSecondsMeasured": 120,
                        "fullVectorRtoSecondsMeasured": 300,
                        "rawDir": str(raw),
                        "provenance": {"gitShaFull": "deadbeef"},
                    }
                ),
                encoding="utf-8",
            )
            o04.write_text(
                json.dumps(
                    {
                        "issue": "P1B-O04",
                        "status": "pass",
                        "provenance": {
                            "gitShaFull": "deadbeef",
                            "composeProject": "markhand-poc",
                            "imageIds": image_ids,
                        },
                        "rawDir": str(raw),
                    }
                ),
                encoding="utf-8",
            )
            result = prerequisites.validate_prerequisites(
                f02_path=f02,
                o02_path=o02,
                o03_path=o03,
                o04_path=o04,
                current_git_full="ffffffffffffffff",
                compose_project="markhand-poc",
            )
            self.assertFalse(result["ok"])
            self.assertTrue(any("stale" in b or "git" in b for b in result["blockers"]), result)


class FailedInjectionTests(unittest.TestCase):
    def test_failed_injection_blocks_pass(self) -> None:
        thr = gates_eval.load_thresholds(profile.load_workload_profile(WORKLOAD), GATES)
        metrics = {
            "queryP50Ms": 10.0,
            "queryP95Ms": 10.0,
            "queryP99Ms": 10.0,
            "ingestDocsPerHour": 99999.0,
            "rssGrowthMb": 1.0,
            "tempGrowthMb": 1.0,
            "queueDepthMax": 1,
            "dbConnectionsMax": 1,
            "durationSeconds": 1800,
            "smoke": False,
            "workerRecoveryPass": False,
            "dependencyRecoveryPass": True,
            "postRestoreRetrievalPass": True,
            "requestErrors": 0,
        }
        gates = gates_eval.evaluate_numeric_gates(metrics, thr)
        status, blockers = report.evaluate_status(
            markhand_soak=True,
            prerequisites_ok=True,
            measured=True,
            smoke=False,
            gates=gates,
            injection_ok=False,
            redaction_ok=True,
            duration_seconds=1800,
            official_duration=1800,
        )
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("injection" in b or "recovery" in b for b in blockers), blockers)


class UnboundedGrowthTests(unittest.TestCase):
    def test_rss_growth_over_bound_fails(self) -> None:
        thr = gates_eval.load_thresholds(profile.load_workload_profile(WORKLOAD), GATES)
        metrics = {
            "queryP50Ms": 10.0,
            "queryP95Ms": 10.0,
            "queryP99Ms": 10.0,
            "ingestDocsPerHour": 99999.0,
            "rssGrowthMb": 257.0,
            "tempGrowthMb": 1.0,
            "queueDepthMax": 1,
            "dbConnectionsMax": 1,
            "durationSeconds": 1800,
            "smoke": False,
            "workerRecoveryPass": True,
            "dependencyRecoveryPass": True,
            "postRestoreRetrievalPass": True,
            "requestErrors": 0,
        }
        gates = gates_eval.evaluate_numeric_gates(metrics, thr)
        self.assertEqual(gates["rssGrowth"], "fail")
        self.assertEqual(gates["unboundedGrowth"], "fail")


class SecretScanTests(unittest.TestCase):
    def test_redact_and_scan(self) -> None:
        dirty = (
            "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.aaa.bbb\n"
            "postgres://user:supersecret@localhost:5432/db\n"
            'password="hunter2"\n'
        )
        cleaned = redact.redact_text(dirty)
        self.assertNotIn("supersecret", cleaned)
        self.assertNotIn("hunter2", cleaned)
        self.assertNotIn("eyJhbGciOiJIUzI1NiJ9", cleaned)
        with tempfile.TemporaryDirectory() as tmp:
            raw = Path(tmp)
            (raw / "log.txt").write_text(dirty, encoding="utf-8")
            scan = redact.scan_raw_dir(raw)
            self.assertFalse(scan["passed"])
            (raw / "log.txt").write_text(cleaned, encoding="utf-8")
            scan2 = redact.scan_raw_dir(raw)
            self.assertTrue(scan2["passed"])


class DefaultNotRunTests(unittest.TestCase):
    def test_no_opt_in_is_not_run(self) -> None:
        status, blockers = report.evaluate_status(
            markhand_soak=False,
            prerequisites_ok=False,
            measured=False,
            smoke=False,
            gates={
                "queryP95": "unknown",
                "queryP99": "unknown",
                "ingestThroughput": "unknown",
                "rssGrowth": "unknown",
                "tempGrowth": "unknown",
                "queueDepth": "unknown",
                "dbConnections": "unknown",
                "unboundedGrowth": "unknown",
                "recovery": "unknown",
                "postRestoreRetrieval": "unknown",
            },
            injection_ok=False,
            redaction_ok=True,
            duration_seconds=0,
            official_duration=1800,
        )
        self.assertEqual(status, "not_run")
        self.assertIn("MARKHAND_SOAK!=1", blockers)

    def test_opt_in_without_evidence_incomplete(self) -> None:
        status, blockers = report.evaluate_status(
            markhand_soak=True,
            prerequisites_ok=False,
            measured=False,
            smoke=False,
            gates={
                "queryP95": "unknown",
                "queryP99": "unknown",
                "ingestThroughput": "unknown",
                "rssGrowth": "unknown",
                "tempGrowth": "unknown",
                "queueDepth": "unknown",
                "dbConnections": "unknown",
                "unboundedGrowth": "unknown",
                "recovery": "unknown",
                "postRestoreRetrieval": "unknown",
            },
            injection_ok=False,
            redaction_ok=True,
            duration_seconds=0,
            official_duration=1800,
        )
        self.assertIn(status, {"incomplete", "fail"})
        self.assertNotEqual(status, "pass")


class ReportCollisionTests(unittest.TestCase):
    def test_o05_report_issue_and_paths(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            payload = report.build_not_run_report(
                profile_path=str(WORKLOAD),
                out_dir=out,
                git_short="abc1234",
                git_full="abc1234dead",
                raw_dir=out / "raw" / "o05-test",
            )
            report.write_reports(out, payload)
            o05 = json.loads((out / "o05-soak.json").read_text(encoding="utf-8"))
            self.assertEqual(o05["issue"], "P1B-O05")
            self.assertEqual(o05["status"], "not_run")
            summary = json.loads((out / "summary.json").read_text(encoding="utf-8"))
            self.assertEqual(summary["issue"], "P1B-O05")
            self.assertEqual(summary.get("canonicalReport"), "o05-soak.json")
            self.assertNotEqual(summary.get("issue"), "P1B-O04")


class InjectionAllowlistTests(unittest.TestCase):
    def test_refuses_arbitrary_container(self) -> None:
        with self.assertRaises(injection.InjectionError):
            injection.resolve_target_container(
                compose_project="markhand-poc",
                service="postgres",
                container_id="deadbeef",
                allowed_ids={},
            )


if __name__ == "__main__":
    unittest.main()
