#!/usr/bin/env python3
"""Unit / hermetic tests for P1B-O05 measured soak harness (Sol vòng 2)."""

from __future__ import annotations

import hashlib
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
import post_restore_probe  # noqa: E402
import prerequisites  # noqa: E402
import profile  # noqa: E402
import redact  # noqa: E402
import report  # noqa: E402
import sampler  # noqa: E402
import workload  # noqa: E402

ROOT = Path(__file__).resolve().parents[3]
WORKLOAD = ROOT / "bench/markhand_web/workloads/phase1b-mixed.yaml"
GATES = ROOT / "bench/markhand_web/gates.yaml"
POC_COMPOSE = ROOT / "deploy/compose.poc.yml"
FORMATS = ["pdf", "docx", "pptx", "xlsx", "csv", "html", "txt", "png"]


def write_raw_manifest_for_test(raw: Path) -> dict[str, str]:
    manifest = report.build_raw_manifest(raw)
    path = raw / "raw-manifest.json"
    path.write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return {
        "path": str(path),
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
    }


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


class PocQualificationRateLimitTests(unittest.TestCase):
    def test_api_limits_cover_canonical_mixed_load(self) -> None:
        compose = POC_COMPOSE.read_text(encoding="utf-8")
        api = compose.split("\n  api:\n", 1)[1].split("\n  api-restore-green:\n", 1)[0]

        self.assertIn(
            "MARKHAND_RATE_USER_PER_MINUTE: ${MARKHAND_RATE_USER_PER_MINUTE:-600}",
            api,
        )
        self.assertIn(
            "MARKHAND_RATE_IP_PER_MINUTE: ${MARKHAND_RATE_IP_PER_MINUTE:-1200}",
            api,
        )
        self.assertIn(
            "MARKHAND_RATE_ROUTE_PER_MINUTE: ${MARKHAND_RATE_ROUTE_PER_MINUTE:-600}",
            api,
        )


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


class PublishedVersionCompletionTests(unittest.TestCase):
    def test_seed_uses_published_not_upload_draft_version_id(self) -> None:
        marker = fixtures.marker_for("txt")
        client = mock.Mock()
        client.collection_id = "collection-1"
        client._headers.return_value = {}
        client.request.side_effect = [
            (
                200,
                b'{"documentId":"doc-1","versionId":"draft-1"}',
                1.0,
            ),
            (
                200,
                b'{"items":[{"id":"published-1","isCurrent":true}]}',
                1.0,
            ),
            (
                200,
                json.dumps(
                    {
                        "hits": [
                            {
                                "documentId": "doc-1",
                                "versionId": "published-1",
                                "snippet": marker,
                            }
                        ],
                        "citations": [
                            {
                                "logicalDocumentId": "doc-1",
                                "versionId": "published-1",
                                "quote": marker,
                            }
                        ],
                    }
                ).encode(),
                1.0,
            ),
        ]
        with mock.patch.object(
            workload,
            "_multipart",
            return_value=(b"body", "multipart/form-data; boundary=test"),
        ):
            result = dataset.seed_and_wait_indexed(
                client,
                formats=["txt"],
                fixture_path_fn=lambda _fmt: Path("ignored.txt"),
                timeout_seconds=1,
                poll_seconds=0,
            )
        self.assertTrue(result["ok"])
        self.assertEqual(result["seeded"][0]["versionId"], "published-1")
        self.assertNotIn("draft-1", json.dumps(result))

    def test_timed_ingest_records_published_version(self) -> None:
        client = mock.Mock()
        client.collection_id = "collection-1"
        client._headers.return_value = {}
        client.request.side_effect = [
            (
                200,
                b'{"documentId":"doc-1","versionId":"draft-1"}',
                1.0,
            ),
            (
                200,
                b'{"items":[{"id":"published-1","isCurrent":true}]}',
                1.0,
            ),
        ]
        stats = workload.RequestStats()
        with (
            mock.patch.object(workload, "fixture_path", return_value=Path("ignored.txt")),
            mock.patch.object(
                workload,
                "_multipart",
                return_value=(b"body", "multipart/form-data; boundary=test"),
            ),
            mock.patch.object(
                workload, "wait_until_indexed_visible", return_value=True
            ),
        ):
            workload.do_ingest(
                client,
                "txt",
                stats,
                start_mono=time.monotonic(),
            )
        self.assertEqual(stats.success["ingest"], 1)
        self.assertEqual(stats.doc_versions["doc-1"], "published-1")
        self.assertEqual(stats.versions["doc-1"][0].version_id, "published-1")
        self.assertTrue(stats.versions["doc-1"][0].published)


class DeleteRetentionTests(unittest.TestCase):
    def test_delete_never_selects_retained_baseline_document(self) -> None:
        client = mock.Mock()
        client.request.return_value = (204, b"", 1.0)
        stats = workload.RequestStats()
        stats.retained_ids = ["retained-1"]
        stats.document_ids = ["retained-1", "deletable-1"]

        workload.do_delete(client, stats, start_mono=time.monotonic())

        client.request.assert_called_once_with(
            "DELETE", "/api/v1/documents/deletable-1"
        )
        self.assertEqual(stats.retained_ids, ["retained-1"])
        self.assertEqual(stats.document_ids, ["retained-1"])
        self.assertEqual(stats.deleted_ids, ["deletable-1"])


class QuerySuccessTests(unittest.TestCase):
    def test_compare_without_dataset_is_not_success(self) -> None:
        stats = workload.RequestStats()
        client = workload.ApiClient("http://127.0.0.1:9", token="t", collection_id="c")
        workload.do_query(client, "compare", stats, start_mono=time.monotonic())
        self.assertEqual(stats.success.get("query", 0), 0)
        self.assertTrue(
            any("compare_dataset_unavailable" in r for r in stats.not_ready)
        )
        self.assertEqual(stats.query_success_latencies_ms, [])

    def test_only_2xx_count_latency(self) -> None:
        stats = workload.RequestStats()
        client = mock.Mock()
        client.collection_id = "c"
        client.request.return_value = (400, b"{}", 12.0)
        workload.do_query(client, "current", stats, start_mono=time.monotonic())
        self.assertEqual(stats.success.get("query", 0), 0)
        self.assertEqual(stats.query_success_latencies_ms, [])
        client.request.return_value = (
            200,
            b'{"hits":[],"citations":[],"requestId":"x"}',
            15.0,
        )
        workload.do_query(client, "current", stats, start_mono=time.monotonic())
        self.assertEqual(stats.success.get("query", 0), 0)
        self.assertEqual(stats.query_success_latencies_ms, [])
        stats.document_ids.append("doc-1")
        stats.record_marker("doc-1", "SOAK_TXT_MARKER")
        stats.record_expected_version("doc-1", "ver-1")
        client.request.return_value = (
            200,
            b'{"hits":[{"documentId":"doc-1","versionId":"ver-1","snippet":"SOAK_TXT_MARKER"}],'
            b'"citations":[{"logicalDocumentId":"doc-1","versionId":"ver-1","quote":"SOAK_TXT_MARKER"}],'
            b'"requestId":"x"}',
            15.0,
        )
        scheduled_mono = time.monotonic() - 0.015
        workload.do_query(
            client,
            "current",
            stats,
            start_mono=scheduled_mono,
            scheduled_mono=scheduled_mono,
        )
        self.assertEqual(stats.success.get("query", 0), 1)
        self.assertEqual(len(stats.query_success_latencies_ms), 1)
        self.assertGreaterEqual(stats.query_success_latencies_ms[0], 15.0)


class CompareDatasetTests(unittest.TestCase):
    def test_revision_multipart_includes_existing_document_id(self) -> None:
        body, content_type = workload._multipart_bytes(
            filename="compare-b.txt",
            file_bytes=b"SOAKCOMPARE B",
            collection_id="collection-1",
            document_id="document-1",
        )
        self.assertIn("multipart/form-data; boundary=", content_type)
        self.assertIn(b'name="documentId"', body)
        self.assertIn(b"document-1", body)

    def test_build_compare_dataset_uses_published_history_window(self) -> None:
        built = dataset.build_compare_dataset_from_versions(
            document_id="doc-1",
            version_a="ver-a",
            version_b="ver-b",
            query="SOAKCOMPARE15",
            marker_a="SOAKCOMPARE15A",
            marker_b="SOAKCOMPARE15B",
            versions=[
                {
                    "id": "ver-b",
                    "effectiveFrom": "2026-02-01T00:00:00Z",
                    "effectiveTo": None,
                    "isCurrent": True,
                },
                {
                    "id": "ver-a",
                    "effectiveFrom": "2026-01-01T00:00:00Z",
                    "effectiveTo": "2026-02-01T00:00:00Z",
                    "isCurrent": False,
                },
            ],
        )
        self.assertEqual(built["query"], "SOAKCOMPARE15")
        self.assertEqual(built["asOfA"], "2026-01-16T12:00:00Z")
        self.assertGreater(built["asOfB"], built["effectiveFromB"])

    def test_build_compare_dataset_avoids_publish_window_overlap(self) -> None:
        built = dataset.build_compare_dataset_from_versions(
            document_id="doc-1",
            version_a="ver-a",
            version_b="ver-b",
            query="SOAKCOMPARE15",
            marker_a="SOAKCOMPARE15A",
            marker_b="SOAKCOMPARE15B",
            versions=[
                {
                    "id": "ver-b",
                    "effectiveFrom": "2026-02-01T00:00:00.000000Z",
                    "effectiveTo": None,
                    "isCurrent": True,
                },
                {
                    "id": "ver-a",
                    "effectiveFrom": "2026-01-01T00:00:00Z",
                    "effectiveTo": "2026-02-01T00:00:00.006000Z",
                    "isCurrent": False,
                },
            ],
        )
        self.assertLess(
            dataset._parse_timestamp(built["asOfA"], "as_of_a"),
            dataset._parse_timestamp(built["effectiveFromB"], "effective_from_b"),
        )
        self.assertGreater(
            dataset._parse_timestamp(built["asOfB"], "as_of_b"),
            dataset._parse_timestamp(
                "2026-02-01T00:00:00.006000Z", "effective_to_a"
            ),
        )

    def test_create_compare_uses_published_not_upload_draft_version_ids(self) -> None:
        client = mock.Mock()
        client.request.side_effect = [
            (
                200,
                json.dumps(
                    {
                        "items": [
                            {
                                "id": "published-a",
                                "effectiveFrom": "2026-01-01T00:00:00Z",
                                "effectiveTo": None,
                                "isCurrent": True,
                            }
                        ]
                    }
                ).encode(),
                1.0,
            ),
            (
                200,
                json.dumps(
                    {
                        "items": [
                            {
                                "id": "published-b",
                                "effectiveFrom": "2026-02-01T00:00:00Z",
                                "effectiveTo": None,
                                "isCurrent": True,
                            },
                            {
                                "id": "published-a",
                                "effectiveFrom": "2026-01-01T00:00:00Z",
                                "effectiveTo": "2026-02-01T00:00:00Z",
                                "isCurrent": False,
                            },
                        ]
                    }
                ).encode(),
                1.0,
            ),
        ]
        with (
            mock.patch.object(
                dataset,
                "_upload_compare_version",
                side_effect=[("doc-1", "draft-a"), ("doc-1", "draft-b")],
            ),
            mock.patch.object(
                workload, "wait_until_indexed_visible", return_value=True
            ) as wait,
        ):
            built = dataset.create_compare_dataset(client)
        self.assertEqual(built["versionA"], "published-a")
        self.assertEqual(built["versionB"], "published-b")
        self.assertNotIn("draft-a", built.values())
        self.assertNotIn("draft-b", built.values())
        self.assertEqual(wait.call_count, 2)
        for call in wait.call_args_list:
            self.assertNotIn("expected_version", call.kwargs)

    def test_missing_env_can_create_real_public_revision_pair(self) -> None:
        generated = {
            "documentId": "doc-1",
            "versionA": "ver-a",
            "versionB": "ver-b",
            "query": "SOAKCOMPARE15",
            "markerA": "SOAKCOMPARE15A",
            "markerB": "SOAKCOMPARE15B",
            "effectiveFromA": "2026-01-01T00:00:00Z",
            "effectiveFromB": "2026-02-01T00:00:00Z",
            "asOfA": "2026-01-15T00:00:00Z",
            "asOfB": "2026-02-15T00:00:00Z",
        }
        with mock.patch.dict("os.environ", {}, clear=False):
            import os

            os.environ.pop(dataset.COMPARE_ENV, None)
            with (
                mock.patch.object(
                    dataset, "create_compare_dataset", return_value=generated
                ),
                mock.patch.object(
                    dataset,
                    "verify_compare_dataset",
                    return_value={"ok": True},
                ),
            ):
                info = dataset.resolve_compare_or_block(
                    mock.Mock(),
                    modes=["compare"],
                    create_if_missing=True,
                )
        self.assertTrue(info["available"])
        self.assertEqual(info["source"], "public_revision_upload")

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
        client.request.return_value = (
            200,
            b'{"hits":[{"documentId":"doc-1"}],"citations":[{"documentId":"doc-1"}]}',
            5.0,
        )
        raw = json.dumps(
            {
                "documentId": "doc-1",
                "versionA": "ver-a",
                "versionB": "ver-b",
                "query": "SOAK_COMPARE",
                "markerA": "SOAK_A_MARKER",
                "markerB": "SOAK_B_MARKER",
                "effectiveFromA": "2026-01-01T00:00:00Z",
                "effectiveFromB": "2026-02-01T00:00:00Z",
                "asOfA": "2026-01-15T00:00:00Z",
                "asOfB": "2026-02-15T00:00:00Z",
            }
        )
        client.request.return_value = (
            200,
            b'{"hits":['
            b'{"documentId":"doc-1","versionId":"ver-a","snippet":"SOAK_A_MARKER"},'
            b'{"documentId":"doc-1","versionId":"ver-b","snippet":"SOAK_B_MARKER"}'
            b'],"citations":[]}',
            5.0,
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
            "workloadDrainPassed": True,
            "reconcilePassed": True,
            "resourceCoveragePassed": True,
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
        stats.scheduled["delete"] = 10
        stats.scheduled["reconcile"] = 2
        stats.submitted.update(stats.scheduled)
        stats.completed.update(stats.scheduled)
        stats.success["ingest"] = 95
        stats.success["query"] = 94
        stats.success["delete"] = 10
        stats.success["reconcile"] = 2
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
        with mock.patch.object(
            workload, "fixture_path", side_effect=RuntimeError("boom")
        ):
            with mock.patch.object(
                workload, "preflight_fixtures", return_value={"ok": True}
            ):
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

        with mock.patch.object(
            workload, "preflight_fixtures", return_value={"ok": True}
        ):
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
                            injection_schedule=[
                                (0.5, "kill_worker"),
                                (1.0, "dependency_blip"),
                            ],
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

    def test_nonzero_docker_kill_fails_injection(self) -> None:
        def runner(_args, **_kwargs):
            return __import__("subprocess").CompletedProcess(_args, 1, "", "nope")

        with self.assertRaises(injection.InjectionError) as ctx:
            injection.kill_and_restart_worker(
                compose_project="markhand-poc",
                service="worker-convert",
                allowed_ids={"worker-convert": "abcdef123456"},
                runner=runner,
            )
        self.assertIn("worker_kill_failed", str(ctx.exception))

    def test_injection_window_registered_before_failure(self) -> None:
        plan = injection.InjectionPlan()
        plan.workload_start_mono = time.monotonic()
        plan.start_pool(max_workers=1)
        entered = threading.Event()

        def boom() -> dict:
            entered.set()
            time.sleep(0.1)
            raise injection.InjectionError("forced")

        plan.schedule(kind="kill_worker", scheduled_at=0.0, fn=boom)
        self.assertTrue(entered.wait(timeout=1.0))
        self.assertTrue(plan.in_window(time.monotonic() - plan.workload_start_mono))
        with self.assertRaises(injection.InjectionError):
            plan.join(timeout=2.0)


class PostRestoreTests(unittest.TestCase):
    def test_external_green_probe_uses_distinct_identity_and_unauth_context(
        self,
    ) -> None:
        request = {
            "collectionId": "collection-1",
            "retainedIds": ["ret-1"],
            "deletedIds": ["del-1"],
            "retainedMarkers": {"ret-1": "SOAK_RETAINED"},
            "deletedMarkers": {"del-1": "SOAK_DELETED"},
        }
        env = {
            "MARKHAND_SOAK_API_BASE": "http://127.0.0.1:8788",
            "MARKHAND_SOAK_PASSWORD": "secret",
            "MARKHAND_O03_GREEN_DEPLOYMENT_ID": "green-deployment",
            "MARKHAND_O03_BLUE_DEPLOYMENT_ID": "blue-deployment",
            "MARKHAND_O03_GREEN_STORAGE_SIGNATURE": "green-storage",
            "MARKHAND_O03_BLUE_STORAGE_SIGNATURE": "blue-storage",
        }
        clients = [mock.Mock(), mock.Mock()]
        with (
            mock.patch.dict("os.environ", env, clear=False),
            mock.patch.object(
                post_restore_probe.workload, "login", return_value="token"
            ),
            mock.patch.object(
                post_restore_probe.workload,
                "ApiClient",
                side_effect=clients,
            ),
            mock.patch.object(
                post_restore_probe.dataset,
                "post_restore_retrieval_check",
                return_value={"passed": True, "gate": "pass"},
            ) as check,
        ):
            result = post_restore_probe.run_probe("http://127.0.0.1:18789", request)
        self.assertTrue(result["passed"])
        self.assertEqual(result["restoredApi"]["source"], "o03_live_external_probe")
        self.assertIs(check.call_args.kwargs["unauthorized_client"], clients[1])

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

    def test_restored_requires_distinct_green_identity_and_storage(self) -> None:
        info = dataset.resolve_restored_api_base(
            blue_base="http://127.0.0.1:8788",
            o03_report={
                "restoredApiBase": "http://127.0.0.1:8789",
                "greenDeploymentIdentity": "green-a",
                "blueDeploymentIdentity": "blue-a",
                "greenStorageSignature": "store-green",
                "blueStorageSignature": "store-blue",
            },
        )
        self.assertTrue(info["available"], info)
        alias_only = dataset.resolve_restored_api_base(
            blue_base="http://127.0.0.1:8788",
            o03_report={"restoredApiBase": "http://127.0.0.1:8789"},
        )
        self.assertFalse(alias_only["available"])
        self.assertEqual(alias_only["blocker"], "restored_green_identity_missing")

    def test_retained_hit_absent_non_pass(self) -> None:
        restored = mock.Mock()
        restored.collection_id = "c"
        # Search empty + document GET 404 ⇒ retained absent.
        restored.request.side_effect = [
            (200, b'{"id":"ret-1"}', 1.0),
            (404, b"", 1.0),
            (200, b'{"hits":[],"citations":[]}', 5.0),
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
            retained_markers={"ret-1": "SOAK_TXT_MARKER"},
        )
        self.assertFalse(result["passed"])
        self.assertEqual(result["reason"], "retained_hit_absent")

    def test_unauthorized_2xx_non_pass(self) -> None:
        restored = mock.Mock()
        restored.collection_id = "c"
        restored.request.return_value = (
            200,
            b'{"hits":[{"documentId":"ret-1","snippet":"SOAK_TXT_MARKER"}],'
            b'"citations":[{"logicalDocumentId":"ret-1","quote":"SOAK_TXT_MARKER"}]}',
            5.0,
        )
        unauthorized = mock.Mock()
        unauthorized.collection_id = "c"
        restored.request.side_effect = [
            (200, b'{"id":"ret-1"}', 1.0),
            (404, b"", 1.0),
            (
                200,
                b'{"hits":[{"documentId":"ret-1","snippet":"SOAK_TXT_MARKER"}],'
                b'"citations":[{"logicalDocumentId":"ret-1","quote":"SOAK_TXT_MARKER"}]}',
                5.0,
            ),
        ]
        unauthorized.request.side_effect = [
            (403, b"", 1.0),
            (200, b'{"id":"ret-1"}', 5.0),
        ]
        result = dataset.post_restore_retrieval_check(
            restored,
            retained_ids=["ret-1"],
            deleted_ids=["del-1"],
            unauthorized_client=unauthorized,
            same_run_restore=True,
            restored_endpoint_ok=True,
            retained_markers={"ret-1": "SOAK_TXT_MARKER"},
        )
        self.assertFalse(result["passed"])
        self.assertEqual(result["reason"], "unauthorized_access_2xx")

    def test_post_restore_pass_requires_retained_deleted_authz(self) -> None:
        restored = mock.Mock()
        restored.collection_id = "c"
        restored.request.return_value = (
            200,
            b'{"hits":[{"documentId":"ret-1","snippet":"SOAK_TXT_MARKER"}],'
            b'"citations":[{"logicalDocumentId":"ret-1","quote":"SOAK_TXT_MARKER"}]}',
            5.0,
        )
        unauthorized = mock.Mock()
        unauthorized.collection_id = "c"
        restored.request.side_effect = [
            (200, b'{"id":"ret-1"}', 1.0),
            (404, b"", 1.0),
            (
                200,
                b'{"hits":[{"documentId":"ret-1","snippet":"SOAK_TXT_MARKER"}],'
                b'"citations":[{"logicalDocumentId":"ret-1","quote":"SOAK_TXT_MARKER"}]}',
                5.0,
            ),
        ]
        unauthorized.request.side_effect = [
            (403, b"", 1.0),
            (403, b"", 1.0),
        ]
        result = dataset.post_restore_retrieval_check(
            restored,
            retained_ids=["ret-1"],
            deleted_ids=["del-1"],
            unauthorized_client=unauthorized,
            same_run_restore=True,
            restored_endpoint_ok=True,
            retained_markers={"ret-1": "SOAK_TXT_MARKER"},
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
    def test_external_o04_relative_raw_dir_and_artifact_manifest_validate(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            raw = out / "raw" / "o04-deadbeef"
            raw.mkdir(parents=True)
            evidence = raw / "suite.txt"
            evidence.write_text("ok\n", encoding="utf-8")
            manifest_path = raw / "raw-manifest.json"
            manifest_path.write_text(
                json.dumps(
                    {
                        "schema": 1,
                        "artifacts": {
                            "suite.txt": {
                                "sha256": hashlib.sha256(
                                    evidence.read_bytes()
                                ).hexdigest(),
                                "sizeBytes": evidence.stat().st_size,
                            }
                        },
                    }
                ),
                encoding="utf-8",
            )
            report_path = out / "o04-release.json"
            report_path.write_text("{}\n", encoding="utf-8")
            self.assertFalse(
                prerequisites._raw_ok(
                    {"rawDir": "raw/o04-deadbeef"},
                    report_path=report_path,
                    allow_external=True,
                )
            )
            self.assertTrue(
                prerequisites._raw_ok(
                    {
                        "rawDir": "raw/o04-deadbeef",
                        "rawArtifactManifest": {
                            "path": str(manifest_path),
                            "sha256": hashlib.sha256(
                                manifest_path.read_bytes()
                            ).hexdigest(),
                        },
                    },
                    report_path=report_path,
                    allow_external=True,
                )
            )

    def test_missing_referenced_raw_manifest_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            raw = Path(tmp) / "raw"
            raw.mkdir()
            self.assertFalse(
                prerequisites._raw_ok(
                    {
                        "rawDir": str(raw),
                        "rawArtifactManifest": {
                            "path": "raw-manifest.json",
                            "sha256": "0" * 64,
                        },
                    },
                    allow_external=True,
                )
            )

    def test_current_git_clean_and_full_provenance_required(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            raw = base / "raw"
            raw.mkdir()
            (raw / "evidence.json").write_text("{}\n", encoding="utf-8")
            (raw / "x.txt").write_text("ok\n", encoding="utf-8")
            raw_manifest = write_raw_manifest_for_test(raw)
            image_ids = {
                svc: f"sha256:{i:064d}"
                for i, svc in enumerate(prerequisites.EXPECTED_POC_SERVICES)
            }
            mig = prerequisites.current_deploy_fingerprint()["migrationManifestSha256"]
            compose = prerequisites.current_deploy_fingerprint()["composeFileSha256"]
            git_sha = "f" * 40
            index_sig = "b" * 64
            prov = {
                "gitShaFull": git_sha,
                "gitDirty": False,
                "composeProject": "markhand-poc",
                "imageIds": image_ids,
                "migrationManifestSha256": mig,
                "composeFileSha256": compose,
                "indexSignature": index_sig,
            }
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
                        "gitWorktree": {"dirty": False},
                        "rawDir": str(raw),
                        "rawArtifactManifest": raw_manifest,
                        "provenance": prov,
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
                        "rawArtifactManifest": raw_manifest,
                        "provenance": prov,
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
                        "rawArtifactManifest": raw_manifest,
                        "provenance": prov,
                    }
                ),
                encoding="utf-8",
            )
            o04.write_text(
                json.dumps(
                    {
                        "issue": "P1B-O04",
                        "status": "pass",
                        "provenance": prov,
                        "rawDir": str(raw),
                        "rawArtifactManifest": raw_manifest,
                    }
                ),
                encoding="utf-8",
            )
            result = prerequisites.validate_prerequisites(
                f02_path=f02,
                o02_path=o02,
                o03_path=o03,
                o04_path=o04,
                current_git_full=git_sha,
                compose_project="markhand-poc",
                current_git_clean=True,
                live_image_ids=image_ids,
                live_index_signature=index_sig,
                trusted_attestation=True,
            )
            self.assertTrue(result["ok"], result["blockers"])
            self.assertIn("f02", result["canonicalReports"])

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
            self.assertTrue(
                any("stale_incompatible:image" in b for b in result["blockers"])
            )


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


class ReportValidationTests(unittest.TestCase):
    def test_validate_report_rejects_missing_raw_manifest_even_if_status_pass(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            raw = out / "raw" / "o05-20260724T000000Z-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            raw.mkdir(parents=True)
            (raw / "metrics.json").write_text("{}\n", encoding="utf-8")
            payload = {
                "issue": "P1B-O05",
                "canonicalReport": "o05-soak.json",
                "status": "pass",
                "markhandSoak": True,
                "smoke": False,
                "smokeNonQualifying": False,
                "durationSeconds": 1800,
                "officialDurationSeconds": 1800,
                "rawDir": str(raw),
                "prerequisites": {"ok": True},
                "metrics": {
                    "measured": True,
                    "workerRecoveryPass": True,
                    "dependencyRecoveryPass": True,
                },
                "failureInjection": {"enabled": True, "summary": {"ok": True}},
                "redactionScan": {"passed": True},
                "gates": {key: "pass" for key in report.unknown_gates()},
            }
            status, blockers = report.validate_report_payload(
                payload, report_path=out / "o05-soak.json"
            )
        self.assertNotEqual(status, "pass")
        self.assertIn("raw_manifest_missing", blockers)

    def test_validate_report_missing_thresholds_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            raw = out / "raw" / "o05-20260724T000001Z-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            raw.mkdir(parents=True)
            (raw / "metrics.json").write_text("{}\n", encoding="utf-8")
            raw_manifest = report.write_raw_manifest(raw)
            payload = {
                "issue": "P1B-O05",
                "canonicalReport": "o05-soak.json",
                "status": "pass",
                "markhandSoak": True,
                "smoke": False,
                "smokeNonQualifying": False,
                "durationSeconds": 1800,
                "officialDurationSeconds": 1800,
                "rawDir": str(raw),
                "rawManifest": raw_manifest,
                "prerequisites": {"ok": True},
                "metrics": {
                    "measured": True,
                    "workerRecoveryPass": True,
                    "dependencyRecoveryPass": True,
                },
                "failureInjection": {"enabled": True, "summary": {"ok": True}},
                "redactionScan": {"passed": True},
                "gates": {key: "pass" for key in report.unknown_gates()},
            }
            status, blockers = report.validate_report_payload(
                payload, report_path=out / "o05-soak.json"
            )
        self.assertEqual(status, "fail")
        self.assertIn("thresholds_missing", blockers)
        self.assertIn("thresholds_incomplete", blockers)


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
            "https://alice:opensesame@example.invalid/path\n"
            "AWS_SECRET_KEY=abc123456789\n"
            "MINIO_ACCESS_KEY=minioadmin\n"
            "SOAK_TXT_MARKER\n"
            'password="hunter2"\n'
        )
        cleaned = redact.redact_text(dirty)
        self.assertNotIn("supersecret", cleaned)
        self.assertNotIn("opensesame", cleaned)
        self.assertNotIn("SOAK_TXT_MARKER", cleaned)
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
        # POC qualification binds the SLA normal tier; the 1200/hour peak tier
        # belongs to G0-CAP-INGEST-THROUGHPUT on on-prem-reference.
        self.assertEqual(thr["ingestDocsPerHour"], 300)
        self.assertEqual(thr["allowedErrorsOutsideInjection"], 0)
        self.assertTrue(thr["canonicalBindingPass"])

    def test_canonical_text_hash_is_checkout_line_ending_invariant(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            lf = Path(tmp) / "lf.yaml"
            crlf = Path(tmp) / "crlf.yaml"
            lf.write_bytes(b"key: value\nnested:\n  item: 1\n")
            crlf.write_bytes(b"key: value\r\nnested:\r\n  item: 1\r\n")
            self.assertEqual(gates_eval._sha256(lf), gates_eval._sha256(crlf))

    def test_evaluate_pass_at_exact_boundaries(self) -> None:
        thr = gates_eval.load_thresholds(profile.load_workload_profile(WORKLOAD), GATES)
        metrics = {
            "measured": True,
            "queryModesReady": True,
            "querySuccessSamples": 100,
            "queryP50Ms": 100.0,
            "queryP95Ms": 500.0,
            "queryP99Ms": 1000.0,
            "ingestDocsPerHour": 300.0,
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
            "workloadDrainPassed": True,
            "reconcilePassed": True,
            "resourceCoveragePassed": True,
        }
        gates = gates_eval.evaluate_numeric_gates(metrics, thr)
        self.assertTrue(all(v == "pass" for v in gates.values()), gates)

    def test_noncanonical_profile_cannot_bind_official_thresholds(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "phase1b-mixed.yaml"
            path.write_text(
                WORKLOAD.read_text(encoding="utf-8").replace(
                    "durationSeconds: 1800", "durationSeconds: 30"
                ),
                encoding="utf-8",
            )
            loaded = profile.load_workload_profile(path)
            thr = gates_eval.load_thresholds(loaded, GATES)
        self.assertEqual(thr["officialDurationSeconds"], 1800)
        self.assertFalse(thr["canonicalBindingPass"])
        gates = gates_eval.evaluate_numeric_gates(
            {
                "measured": True,
                "queryModesReady": True,
                "querySuccessSamples": 100,
                "queryP95Ms": 1.0,
                "queryP99Ms": 1.0,
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
                "workloadDrainPassed": True,
                "reconcilePassed": True,
                "resourceCoveragePassed": True,
            },
            thr,
        )
        self.assertEqual(gates["canonicalBinding"], "fail")


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
