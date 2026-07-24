#!/usr/bin/env python3
"""Unit / hermetic tests for P1B-O05 measured soak harness (Sol vòng 2)."""

from __future__ import annotations

import json
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest import mock

SOAK_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SOAK_DIR))

import dataset  # noqa: E402
import fixtures  # noqa: E402
import gates_eval  # noqa: E402
import injection  # noqa: E402
import mathutil  # noqa: E402
import prerequisites  # noqa: E402
import profile  # noqa: E402
import redact  # noqa: E402
import report  # noqa: E402
import sampler  # noqa: E402
import workload  # noqa: E402

ROOT = Path(__file__).resolve().parents[3]
WORKLOAD = ROOT / "bench/markhand_web/workloads/phase1b-mixed.yaml"
GATES = ROOT / "bench/markhand_web/gates.yaml"
FORMATS = ["pdf", "docx", "pptx", "xlsx", "csv", "html", "txt", "png"]


class PercentileMathTests(unittest.TestCase):
    def test_percentile_boundaries(self) -> None:
        samples = [10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0]
        self.assertEqual(mathutil.percentile(samples, 50), 55.0)
        self.assertEqual(mathutil.percentile(samples, 95), 95.5)
        self.assertEqual(mathutil.percentile(samples, 99), 99.1)

    def test_percentile_empty_is_none(self) -> None:
        self.assertIsNone(mathutil.percentile([], 95))


class RateScheduleTests(unittest.TestCase):
    def test_schedule_respects_rps_over_window(self) -> None:
        times = mathutil.schedule_event_times(rps=2.0, duration_seconds=5.0, seed=7)
        self.assertGreaterEqual(len(times), 9)
        self.assertLessEqual(len(times), 11)


class FixturePreflightTests(unittest.TestCase):
    def test_generated_fixtures_are_byte_deterministic(self) -> None:
        for fmt in FORMATS:
            self.assertEqual(
                fixtures.generate_bytes(fmt),
                fixtures.generate_bytes(fmt),
                fmt,
            )

    def test_all_eight_formats_structural_and_converter(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            info = fixtures.preflight_fixtures(FORMATS, base=base)
            self.assertTrue(info["ok"])
            self.assertEqual(sorted(info["formats"]), sorted(FORMATS))
            for fmt in FORMATS:
                path = fixtures.fixture_path(fmt, base=base)
                self.assertTrue(path.is_file(), fmt)
                fixtures.validate_structure(fmt, path)
            # When fileconv is present, converter must recover every marker.
            if fixtures.resolve_fileconv() is not None:
                self.assertTrue(info["converterChecked"])
                for fmt in FORMATS:
                    self.assertTrue(info["convertResults"][fmt]["ok"], fmt)

    def test_missing_fixture_fails_preflight(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            fixtures.ensure_fixtures(FORMATS, base=base)
            fixtures.fixture_path("pdf", base=base).unlink()
            with self.assertRaises(fixtures.FixtureError):
                fixtures.preflight_fixtures(FORMATS, base=base, generate=False)

    def test_fake_ooxml_pdf_png_fail_structural_preflight(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            for fmt in ("docx", "pptx", "xlsx", "pdf", "png"):
                path = base / f"soak-{fmt}.{fmt}"
                path.write_bytes(fixtures.invalid_stub_bytes(fmt))
                with self.assertRaises(fixtures.FixtureError, msg=fmt):
                    fixtures.validate_structure(fmt, path)
            # Full preflight with fake stubs must fail closed.
            with self.assertRaises(fixtures.FixtureError):
                fixtures.preflight_fixtures(
                    ["docx", "pptx", "xlsx", "pdf", "png"],
                    base=base,
                    generate=False,
                    require_converter=False,
                )


class QuerySuccessTests(unittest.TestCase):
    def test_compare_without_dataset_is_not_success(self) -> None:
        stats = workload.RequestStats()
        client = workload.ApiClient("http://127.0.0.1:9", token="t", collection_id="c")
        workload.do_query(client, "compare", stats, start_mono=time.monotonic())
        self.assertEqual(stats.success.get("query", 0), 0)
        self.assertTrue(any("compare_dataset_unavailable" in r for r in stats.not_ready))
        self.assertEqual(stats.query_success_latencies_ms, [])

    def test_only_2xx_count_latency(self) -> None:
        stats = workload.RequestStats()
        client = mock.Mock()
        client.collection_id = "c"
        client.request.return_value = (400, b"{}", 12.0)
        workload.do_query(client, "current", stats, start_mono=time.monotonic())
        self.assertEqual(stats.success.get("query", 0), 0)
        self.assertEqual(stats.query_success_latencies_ms, [])
        client.request.return_value = (200, b'{"hits":[],"citations":[],"requestId":"x"}', 15.0)
        workload.do_query(client, "current", stats, start_mono=time.monotonic())
        self.assertEqual(stats.success.get("query", 0), 1)
        self.assertEqual(stats.query_success_latencies_ms, [15.0])


class CompareDatasetTests(unittest.TestCase):
    def test_compare_without_env_is_unavailable_non_pass(self) -> None:
        with mock.patch.dict("os.environ", {}, clear=False):
            import os

            os.environ.pop(dataset.COMPARE_ENV, None)
            info = dataset.resolve_compare_or_block(None, modes=["current", "compare"])
        self.assertTrue(info["required"])
        self.assertFalse(info["available"])
        self.assertEqual(info["blocker"], "compare_dataset_unavailable")

    def test_compare_dataset_verified_when_api_2xx(self) -> None:
        client = mock.Mock()
        client.collection_id = "c"
        client.request.return_value = (200, b'{"hits":[]}', 5.0)
        raw = json.dumps(
            {
                "documentId": "doc-1",
                "versionA": "ver-a",
                "versionB": "ver-b",
            }
        )
        with mock.patch.dict("os.environ", {dataset.COMPARE_ENV: raw}):
            info = dataset.resolve_compare_or_block(client, modes=["compare"])
        self.assertTrue(info["available"])
        self.assertEqual(info["dataset"]["documentId"], "doc-1")


class ZeroSamplesFailTests(unittest.TestCase):
    def test_zero_query_samples_fail_when_measured(self) -> None:
        thr = gates_eval.load_thresholds(profile.load_workload_profile(WORKLOAD), GATES)
        metrics = {
            "measured": True,
            "queryModesReady": False,
            "querySuccessSamples": 0,
            "queryP95Ms": None,
            "queryP99Ms": None,
            "ingestDocsPerHour": 1200.0,
            "ingestOk": 10,
            "rssGrowthMb": 1.0,
            "tempGrowthMb": 1.0,
            "queueDepthMax": 1,
            "dbConnectionsMax": 1,
            "workerRecoveryPass": True,
            "dependencyRecoveryPass": True,
            "postRestoreRetrievalPass": True,
            "requestErrorsOutsideInjection": 0,
            "completenessPassed": True,
        }
        gates = gates_eval.evaluate_numeric_gates(metrics, thr)
        self.assertEqual(gates["queryP95"], "fail")
        self.assertEqual(gates["queryP99"], "fail")


class MissingMetricUnknownTests(unittest.TestCase):
    def test_missing_queue_series_is_none(self) -> None:
        with mock.patch("sampler.urlopen") as urlopen_mock:
            resp = mock.Mock()
            resp.read.return_value = b"# HELP other\nother_metric 1\n"
            resp.__enter__ = mock.Mock(return_value=resp)
            resp.__exit__ = mock.Mock(return_value=False)
            urlopen_mock.return_value = resp
            out = sampler.sample_api_metrics("http://example.invalid")
        self.assertIsNone(out["queueDepthMax"])
        self.assertIsNone(out["queueAgeMax"])

    def test_growth_tracker_defaults_unknown_until_observation(self) -> None:
        tracker = sampler.GrowthTracker()
        summary = tracker.summary()
        self.assertIsNone(summary["queueDepthMax"])
        self.assertIsNone(summary["dbConnectionsMax"])
        self.assertIsNone(summary["tempBytes"]["growth"])


class CompletenessThresholdTests(unittest.TestCase):
    def test_95_percent_completeness(self) -> None:
        stats = workload.RequestStats()
        stats.scheduled["ingest"] = 100
        stats.scheduled["query"] = 100
        stats.success["ingest"] = 95
        stats.success["query"] = 94
        result = workload.completeness_ok(stats, ratio=0.95)
        self.assertFalse(result["passed"])
        stats.success["query"] = 95
        result2 = workload.completeness_ok(stats, ratio=0.95)
        self.assertTrue(result2["passed"])


class RequestErrorGateTests(unittest.TestCase):
    def test_errors_outside_injection_fail(self) -> None:
        thr = gates_eval.load_thresholds(profile.load_workload_profile(WORKLOAD), GATES)
        metrics = {
            "measured": True,
            "queryModesReady": True,
            "querySuccessSamples": 10,
            "queryP95Ms": 10.0,
            "queryP99Ms": 20.0,
            "ingestDocsPerHour": 99999.0,
            "ingestOk": 10,
            "rssGrowthMb": 1.0,
            "tempGrowthMb": 1.0,
            "queueDepthMax": 1,
            "dbConnectionsMax": 1,
            "workerRecoveryPass": True,
            "dependencyRecoveryPass": True,
            "postRestoreRetrievalPass": True,
            "requestErrorsOutsideInjection": 1,
            "completenessPassed": True,
        }
        gates = gates_eval.evaluate_numeric_gates(metrics, thr)
        self.assertEqual(gates["requestErrors"], "fail")


class ExceptionPropagationTests(unittest.TestCase):
    def test_worker_exception_propagates(self) -> None:
        client = workload.ApiClient("http://127.0.0.1:9", token="t", collection_id="c")
        loaded = profile.load_workload_profile(WORKLOAD)
        with mock.patch.object(workload, "fixture_path", side_effect=RuntimeError("boom")):
            with mock.patch.object(workload, "preflight_fixtures", return_value={"ok": True}):
                with self.assertRaises(RuntimeError):
                    workload.run_mixed_load(
                        client=client,
                        profile={
                            **loaded,
                            "actors": {
                                **loaded["actors"],
                                "ingest": {"rps": 20.0, "formats": ["txt"]},
                                "query": {"rps": 0.0, "modes": ["current"]},
                                "delete": {"rps": 0.0},
                                "reconcile": {"intervalSeconds": 9999},
                            },
                        },
                        duration_seconds=1,
                        compose_project="markhand-poc",
                        enable_reconcile=False,
                        max_workers=2,
                        skip_fixture_preflight=True,
                    )


class InjectionTimingTests(unittest.TestCase):
    def test_injection_schedule_invoked_during_workload(self) -> None:
        calls: list[tuple[float, str]] = []
        client = workload.ApiClient("http://127.0.0.1:9", token="t", collection_id="c")
        loaded = profile.load_workload_profile(WORKLOAD)

        def cb(elapsed: float, kind: str) -> None:
            calls.append((elapsed, kind))

        with mock.patch.object(workload, "preflight_fixtures", return_value={"ok": True}):
            with mock.patch.object(workload, "do_ingest", return_value=None):
                with mock.patch.object(workload, "do_query", return_value=None):
                    with mock.patch.object(workload, "do_delete", return_value=None):
                        workload.run_mixed_load(
                            client=client,
                            profile={
                                **loaded,
                                "actors": {
                                    "ingest": {"rps": 1.0, "formats": ["txt"]},
                                    "query": {"rps": 1.0, "modes": ["current"]},
                                    "delete": {"rps": 0.0},
                                    "reconcile": {"intervalSeconds": 9999},
                                },
                            },
                            duration_seconds=2,
                            compose_project="markhand-poc",
                            enable_reconcile=False,
                            injection_callback=cb,
                            injection_schedule=[(0.5, "kill_worker"), (1.0, "dependency_blip")],
                            skip_fixture_preflight=True,
                        )
        kinds = [k for _t, k in calls]
        self.assertIn("kill_worker", kinds)
        self.assertIn("dependency_blip", kinds)
        self.assertTrue(all(0 <= t < 2 for t, _k in calls))

    def test_async_injection_does_not_block_scheduler(self) -> None:
        """Synchronous 15s blip must not masquerade as complete on the scheduler thread."""
        plan = injection.InjectionPlan()
        plan.workload_start_mono = time.monotonic()
        plan.start_pool(max_workers=2)
        started = time.monotonic()
        barrier = threading.Event()

        def slow_blip() -> dict:
            barrier.set()
            time.sleep(0.4)
            return {"action": "dependency_blip", "recovered": True}

        # schedule returns immediately (non-blocking)
        plan.schedule(kind="dependency_blip", scheduled_at=0.0, fn=slow_blip)
        self.assertLess(time.monotonic() - started, 0.15)
        self.assertTrue(barrier.wait(timeout=1.0))
        summary = plan.join(timeout=2.0)
        self.assertTrue(summary["ok"])
        self.assertEqual(summary["expected"], 1)
        self.assertEqual(summary["observed"], 1)

    def test_partial_injection_counts_fail(self) -> None:
        """2 scheduled / 1 observed must fail closed (no overwritten bool)."""
        plan = injection.InjectionPlan()
        plan.workload_start_mono = time.monotonic()
        plan.start_pool(max_workers=2)

        def ok() -> dict:
            return {"action": "kill_worker", "recovered": True}

        def boom() -> dict:
            raise injection.InjectionError("forced")

        plan.schedule(kind="kill_worker", scheduled_at=0.0, fn=ok)
        plan.schedule(kind="kill_worker", scheduled_at=0.1, fn=boom)
        with self.assertRaises(injection.InjectionError) as ctx:
            plan.join(timeout=2.0)
        self.assertIn("injection_incomplete", str(ctx.exception))


class PostRestoreTests(unittest.TestCase):
    def test_without_same_run_restore_unknown(self) -> None:
        client = workload.ApiClient("http://127.0.0.1:9", token="t", collection_id="c")
        result = dataset.post_restore_retrieval_check(
            client,
            retained_ids=["a"],
            deleted_ids=["b"],
            unauthorized_client=None,
            same_run_restore=False,
            restored_endpoint_ok=False,
        )
        self.assertIsNone(result["passed"])
        self.assertEqual(result["gate"], "unknown")

    def test_restored_same_as_blue_non_pass(self) -> None:
        info = dataset.resolve_restored_api_base(
            blue_base="http://127.0.0.1:8788",
            o03_report={"restoredApiBase": "http://127.0.0.1:8788"},
        )
        self.assertFalse(info["available"])
        self.assertEqual(info["blocker"], "restored_api_same_as_blue")

    def test_restored_missing_non_pass(self) -> None:
        with mock.patch.dict("os.environ", {}, clear=False):
            import os

            os.environ.pop(dataset.RESTORED_API_ENV, None)
            info = dataset.resolve_restored_api_base(
                blue_base="http://127.0.0.1:8788", o03_report={"status": "pass"}
            )
        self.assertFalse(info["available"])
        self.assertEqual(info["blocker"], "restored_api_base_missing")

    def test_retained_hit_absent_non_pass(self) -> None:
        restored = mock.Mock()
        restored.collection_id = "c"
        # Search empty + document GET 404 ⇒ retained absent.
        restored.request.side_effect = [
            (200, b'{"hits":[]}', 5.0),
            (404, b"", 1.0),
        ]
        unauthorized = mock.Mock()
        unauthorized.request.return_value = (401, b"", 1.0)
        result = dataset.post_restore_retrieval_check(
            restored,
            retained_ids=["ret-1"],
            deleted_ids=["del-1"],
            unauthorized_client=unauthorized,
            same_run_restore=True,
            restored_endpoint_ok=True,
        )
        self.assertFalse(result["passed"])
        self.assertEqual(result["reason"], "retained_hit_absent")

    def test_unauthorized_2xx_non_pass(self) -> None:
        restored = mock.Mock()
        restored.collection_id = "c"
        restored.request.return_value = (
            200,
            b'{"hits":[{"documentId":"ret-1"}]}',
            5.0,
        )
        unauthorized = mock.Mock()
        unauthorized.request.return_value = (200, b'{"id":"ret-1"}', 5.0)
        result = dataset.post_restore_retrieval_check(
            restored,
            retained_ids=["ret-1"],
            deleted_ids=["del-1"],
            unauthorized_client=unauthorized,
            same_run_restore=True,
            restored_endpoint_ok=True,
        )
        self.assertFalse(result["passed"])
        self.assertEqual(result["reason"], "unauthorized_access_2xx")

    def test_post_restore_pass_requires_retained_deleted_authz(self) -> None:
        restored = mock.Mock()
        restored.collection_id = "c"
        restored.request.return_value = (
            200,
            b'{"hits":[{"documentId":"ret-1"}]}',
            5.0,
        )
        unauthorized = mock.Mock()
        unauthorized.request.return_value = (403, b"", 1.0)
        result = dataset.post_restore_retrieval_check(
            restored,
            retained_ids=["ret-1"],
            deleted_ids=["del-1"],
            unauthorized_client=unauthorized,
            same_run_restore=True,
            restored_endpoint_ok=True,
        )
        self.assertTrue(result["passed"])
        self.assertTrue(result["unauthorizedDenied"])


class SamplerThreadTests(unittest.TestCase):
    def test_background_sampler_does_not_block_caller(self) -> None:
        hits = []
        lock = threading.Lock()

        def sample_fn() -> None:
            time.sleep(0.2)
            with lock:
                hits.append(time.monotonic())

        bg = sampler.BackgroundSampler(interval_seconds=0.3, sample_fn=sample_fn)
        started = time.monotonic()
        bg.start()
        time.sleep(0.05)
        self.assertLess(time.monotonic() - started, 0.15)
        time.sleep(0.5)
        bg.stop()
        self.assertGreaterEqual(len(hits), 1)


class ProvenancePrereqTests(unittest.TestCase):
    def test_ancestor_git_ok_when_images_match(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            raw = base / "raw"
            raw.mkdir()
            (raw / "x.txt").write_text("ok\n", encoding="utf-8")
            image_ids = {
                svc: f"sha256:{i:064d}"
                for i, svc in enumerate(prerequisites.EXPECTED_POC_SERVICES)
            }
            mig = prerequisites.current_deploy_fingerprint()["migrationManifestSha256"]
            compose = prerequisites.current_deploy_fingerprint()["composeFileSha256"]
            f02 = base / "f02.json"
            o02 = base / "o02.json"
            o03 = base / "o03.json"
            o04 = base / "o04.json"
            f02.write_text(
                json.dumps(
                    {
                        "issue": "P1B-F02",
                        "passed": True,
                        "composeProject": "markhand-poc",
                        "imageIds": image_ids,
                        "containerIds": image_ids,
                        "gitShaFull": "deadbeef_old",
                        "rawDir": str(raw),
                        "migrationManifestSha256": mig,
                        "composeFileSha256": compose,
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
                        "provenance": {
                            "gitShaFull": "deadbeef_old",
                            "migrationManifestSha256": mig,
                            "imageIds": image_ids,
                        },
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
                        "provenance": {"gitShaFull": "deadbeef_old", "migrationManifestSha256": mig},
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
                            "gitShaFull": "deadbeef_old",
                            "composeProject": "markhand-poc",
                            "imageIds": image_ids,
                            "migrationManifestSha256": mig,
                            "composeFileSha256": compose,
                            "indexSignature": "b" * 64,
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
                live_image_ids=image_ids,
                live_index_signature="b" * 64,
            )
            self.assertTrue(result["ok"], result["blockers"])

    def test_incompatible_image_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            raw = base / "raw"
            raw.mkdir()
            (raw / "x.txt").write_text("ok\n", encoding="utf-8")
            image_ids = {
                svc: f"sha256:{i:064d}"
                for i, svc in enumerate(prerequisites.EXPECTED_POC_SERVICES)
            }
            live = dict(image_ids)
            live["api"] = "sha256:" + ("f" * 64)
            mig = prerequisites.current_deploy_fingerprint()["migrationManifestSha256"]
            f02 = base / "f02.json"
            o02 = base / "o02.json"
            o03 = base / "o03.json"
            o04 = base / "o04.json"
            for path, payload in [
                (
                    f02,
                    {
                        "issue": "P1B-F02",
                        "passed": True,
                        "composeProject": "markhand-poc",
                        "imageIds": image_ids,
                        "containerIds": image_ids,
                        "rawDir": str(raw),
                        "migrationManifestSha256": mig,
                    },
                ),
                (
                    o02,
                    {
                        "issue": "P1B-O02",
                        "status": "pass",
                        "failCount": 0,
                        "passCount": 1,
                        "liveFaultExecuted": True,
                        "rawDir": str(raw),
                    },
                ),
                (
                    o03,
                    {
                        "issue": "P1B-O03",
                        "consistencyRpoPass": True,
                        "queryReadyRtoPass": True,
                        "rpoSecondsMeasured": 1,
                        "queryReadyRtoSecondsMeasured": 1,
                        "fullVectorRtoSecondsMeasured": 1,
                        "rawDir": str(raw),
                    },
                ),
                (
                    o04,
                    {
                        "issue": "P1B-O04",
                        "status": "pass",
                        "provenance": {
                            "composeProject": "markhand-poc",
                            "imageIds": image_ids,
                            "migrationManifestSha256": mig,
                        },
                        "rawDir": str(raw),
                    },
                ),
            ]:
                path.write_text(json.dumps(payload), encoding="utf-8")
            result = prerequisites.validate_prerequisites(
                f02_path=f02,
                o02_path=o02,
                o03_path=o03,
                o04_path=o04,
                current_git_full="abc",
                compose_project="markhand-poc",
                live_image_ids=live,
            )
            self.assertFalse(result["ok"])
            self.assertTrue(any("stale_incompatible:image" in b for b in result["blockers"]))


class SmokeCannotPassTests(unittest.TestCase):
    def test_smoke_duration_never_pass(self) -> None:
        thr = gates_eval.load_thresholds(profile.load_workload_profile(WORKLOAD), GATES)
        metrics = {
            "measured": True,
            "queryModesReady": True,
            "querySuccessSamples": 100,
            "queryP50Ms": 10.0,
            "queryP95Ms": 10.0,
            "queryP99Ms": 10.0,
            "ingestDocsPerHour": 99999.0,
            "ingestOk": 100,
            "rssGrowthMb": 1.0,
            "tempGrowthMb": 1.0,
            "queueDepthMax": 1,
            "dbConnectionsMax": 1,
            "workerRecoveryPass": True,
            "dependencyRecoveryPass": True,
            "postRestoreRetrievalPass": True,
            "requestErrorsOutsideInjection": 0,
            "completenessPassed": True,
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


class DefaultNotRunTests(unittest.TestCase):
    def test_no_opt_in_is_not_run(self) -> None:
        status, blockers = report.evaluate_status(
            markhand_soak=False,
            prerequisites_ok=False,
            measured=False,
            smoke=False,
            gates=report.unknown_gates(),
            injection_ok=False,
            redaction_ok=True,
            duration_seconds=0,
            official_duration=1800,
        )
        self.assertEqual(status, "not_run")
        self.assertIn("MARKHAND_SOAK!=1", blockers)


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
            summary = json.loads((out / "summary.json").read_text(encoding="utf-8"))
            self.assertEqual(summary["issue"], "P1B-O05")
            self.assertEqual(summary.get("canonicalReport"), "o05-soak.json")


class InjectionAllowlistTests(unittest.TestCase):
    def test_refuses_arbitrary_container(self) -> None:
        with self.assertRaises(injection.InjectionError):
            injection.resolve_target_container(
                compose_project="markhand-poc",
                service="postgres",
                container_id="deadbeef",
                allowed_ids={},
            )


class SecretScanTests(unittest.TestCase):
    def test_redact_and_scan(self) -> None:
        dirty = (
            "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.aaa.bbb\n"
            "postgres://user:supersecret@localhost:5432/db\n"
            'password="hunter2"\n'
        )
        cleaned = redact.redact_text(dirty)
        self.assertNotIn("supersecret", cleaned)
        with tempfile.TemporaryDirectory() as tmp:
            raw = Path(tmp)
            (raw / "log.txt").write_text(dirty, encoding="utf-8")
            self.assertFalse(redact.scan_raw_dir(raw)["passed"])
            (raw / "log.txt").write_text(cleaned, encoding="utf-8")
            self.assertTrue(redact.scan_raw_dir(raw)["passed"])


class ThresholdBoundaryTests(unittest.TestCase):
    def test_thresholds_from_profile_gates_sla(self) -> None:
        loaded = profile.load_workload_profile(WORKLOAD)
        thr = gates_eval.load_thresholds(loaded, GATES)
        self.assertEqual(thr["queryP95Ms"], 500)
        self.assertEqual(thr["queryP99Ms"], 1000)
        self.assertEqual(thr["ingestDocsPerHour"], 1200)
        self.assertEqual(thr["allowedErrorsOutsideInjection"], 0)

    def test_evaluate_pass_at_exact_boundaries(self) -> None:
        thr = gates_eval.load_thresholds(profile.load_workload_profile(WORKLOAD), GATES)
        metrics = {
            "measured": True,
            "queryModesReady": True,
            "querySuccessSamples": 100,
            "queryP50Ms": 100.0,
            "queryP95Ms": 500.0,
            "queryP99Ms": 1000.0,
            "ingestDocsPerHour": 1200.0,
            "ingestOk": 100,
            "rssGrowthMb": 256.0,
            "tempGrowthMb": 512.0,
            "queueDepthMax": 100,
            "dbConnectionsMax": 40,
            "workerRecoveryPass": True,
            "dependencyRecoveryPass": True,
            "postRestoreRetrievalPass": True,
            "requestErrorsOutsideInjection": 0,
            "completenessPassed": True,
        }
        gates = gates_eval.evaluate_numeric_gates(metrics, thr)
        self.assertTrue(all(v == "pass" for v in gates.values()), gates)


class ProfileParseTests(unittest.TestCase):
    def test_loads_phase1b_mixed(self) -> None:
        loaded = profile.load_workload_profile(WORKLOAD)
        self.assertEqual(loaded["durationSeconds"], 1800)
        self.assertEqual(sorted(loaded["actors"]["ingest"]["formats"]), sorted(FORMATS))


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


if __name__ == "__main__":
    unittest.main()
