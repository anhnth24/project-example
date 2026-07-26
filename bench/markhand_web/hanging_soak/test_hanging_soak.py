#!/usr/bin/env python3
"""Hermetic unit tests for the P1B-R06 hanging-dependency soak harness.

This is the only part of R06's remaining gap that can run without a Docker
host: no `docker` daemon, no Compose stack. Two layers are covered:

  1. The gate evaluators in `gate_report.py` are pure functions over sample
     data, so pass / hang-not-detected / deadline-exceeded / unbounded-growth
     / failed-restore are exercised directly against constructed inputs
     (same style as `bench/markhand_web/soak/test_o05_soak.py` against
     `report.py`/`gates_eval.py`).
  2. `PauseGuard`'s arm/pause/restore state machine is exercised against an
     injected fake `docker` runner (same dependency-injection pattern as
     `bench/markhand_web/soak/injection.py`), including the abort path.
  3. `sample_endpoint`'s HTTP layer is exercised against a real local
     "blackhole" TCP listener that accepts a connection and never answers —
     the same technique
     `crates/server/src/services/readiness.rs`'s `blackhole_listener` test
     uses — to prove the client-side timeout actually bounds a genuine hang,
     not just a mocked one.
"""

from __future__ import annotations

import socket
import sys
import threading
import time
import unittest
from pathlib import Path

SELF_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SELF_DIR))

import failpoint  # noqa: E402
import gate_report  # noqa: E402
from run_hanging_soak import sample_endpoint  # noqa: E402


def _ready_sample(*, code: str, status: int = 503, elapsed: float = 1.0) -> dict:
    return {"httpStatus": status, "probeCode": code, "elapsedSeconds": elapsed, "error": None}


def _live_sample(*, status: int = 200, elapsed: float = 0.05) -> dict:
    return {"httpStatus": status, "elapsedSeconds": elapsed, "error": None}


def _base_result(**overrides) -> dict:
    result = {
        "expectedProbeCode": "ready_database",
        "pauseConfirmed": True,
        "samples": {
            "ready": [_ready_sample(code="ready_database") for _ in range(5)],
            "live": [_live_sample() for _ in range(5)],
            "openapi": [_live_sample() for _ in range(5)],
        },
        "concurrencyBatches": [
            {"n": 8, "spanSeconds": 1.5},
            {"n": 8, "spanSeconds": 1.6},
        ],
        "restore": {"restoredConfirmed": True},
        "recovery": {"recoveredWithinDeadline": True, "recoverySeconds": 2.0},
    }
    result.update(overrides)
    return result


class EvaluateDependencyPassTests(unittest.TestCase):
    def test_all_gates_pass_with_clean_samples(self) -> None:
        evaluation = gate_report.evaluate_dependency(_base_result())
        self.assertTrue(
            all(v == "pass" for v in evaluation["gates"].values()),
            evaluation["gates"],
        )
        self.assertEqual(evaluation["blockers"], [])


class HangNotDetectedTests(unittest.TestCase):
    def test_ready_200_during_pause_fails_code_gate(self) -> None:
        result = _base_result(
            samples={
                "ready": [_ready_sample(code="ready_database", status=200, elapsed=0.05)]
                * 5,
                "live": [_live_sample() for _ in range(5)],
                "openapi": [_live_sample() for _ in range(5)],
            }
        )
        evaluation = gate_report.evaluate_dependency(result)
        self.assertEqual(evaluation["gates"]["readyCodeCorrect"], "fail")
        self.assertIn("gate:readyCodeCorrect:fail", evaluation["blockers"])

    def test_wrong_probe_code_fails_code_gate(self) -> None:
        result = _base_result(
            samples={
                "ready": [_ready_sample(code="ready_object_store") for _ in range(5)],
                "live": [_live_sample() for _ in range(5)],
                "openapi": [_live_sample() for _ in range(5)],
            }
        )
        evaluation = gate_report.evaluate_dependency(result)
        self.assertEqual(evaluation["gates"]["readyCodeCorrect"], "fail")


class DeadlineExceededTests(unittest.TestCase):
    def test_ready_sample_over_bound_fails(self) -> None:
        over = gate_report.READY_BOUND_SECONDS + 2.0
        result = _base_result(
            samples={
                "ready": [_ready_sample(code="ready_database", elapsed=over)],
                "live": [_live_sample() for _ in range(5)],
                "openapi": [_live_sample() for _ in range(5)],
            }
        )
        evaluation = gate_report.evaluate_dependency(result)
        self.assertEqual(evaluation["gates"]["readyBounded"], "fail")
        self.assertIn("gate:readyBounded:fail", evaluation["blockers"])

    def test_business_endpoint_over_budget_fails(self) -> None:
        over = gate_report.LIVE_BUDGET_SECONDS + 5.0
        result = _base_result(
            samples={
                "ready": [_ready_sample(code="ready_database") for _ in range(5)],
                "live": [_live_sample(elapsed=over)],
                "openapi": [_live_sample() for _ in range(5)],
            }
        )
        evaluation = gate_report.evaluate_dependency(result)
        self.assertEqual(evaluation["gates"]["liveBounded"], "fail")


class UnboundedGrowthTests(unittest.TestCase):
    def test_growth_beyond_factor_fails_even_under_absolute_bound(self) -> None:
        # 1.0s then 3.0s: both under CONCURRENCY_BOUND_SECONDS on their own,
        # but 3.0 > 1.0 * CONCURRENCY_GROWTH_FACTOR (1.75) -> a leak, not a hang.
        result = _base_result(
            concurrencyBatches=[
                {"n": 8, "spanSeconds": 1.0},
                {"n": 8, "spanSeconds": 3.0},
            ]
        )
        evaluation = gate_report.evaluate_dependency(result)
        self.assertEqual(evaluation["gates"]["concurrencyBounded"], "pass")
        self.assertEqual(evaluation["gates"]["concurrencyNoGrowth"], "fail")
        self.assertIn("gate:concurrencyNoGrowth:fail", evaluation["blockers"])

    def test_batch_over_absolute_bound_fails_bounded_gate(self) -> None:
        result = _base_result(
            concurrencyBatches=[
                {"n": 8, "spanSeconds": 1.0},
                {"n": 8, "spanSeconds": gate_report.CONCURRENCY_BOUND_SECONDS + 1.0},
            ]
        )
        evaluation = gate_report.evaluate_dependency(result)
        self.assertEqual(evaluation["gates"]["concurrencyBounded"], "fail")


class FailedRestoreTests(unittest.TestCase):
    def test_unconfirmed_restore_is_a_hard_blocker(self) -> None:
        result = _base_result(restore={"restoredConfirmed": False})
        evaluation = gate_report.evaluate_dependency(result)
        self.assertEqual(evaluation["gates"]["restoreConfirmed"], "fail")
        self.assertIn("gate:restoreConfirmed:fail", evaluation["blockers"])

    def test_status_evaluation_treats_failed_restore_as_hard_fail(self) -> None:
        status, blockers = gate_report.evaluate_status(
            opted_in=True,
            smoke=False,
            covers_all_dependencies=True,
            dependency_blockers=["database:gate:restoreConfirmed:fail"],
            redaction_ok=True,
        )
        self.assertEqual(status, "fail")
        self.assertIn("database:gate:restoreConfirmed:fail", blockers)


class EvaluateStatusTests(unittest.TestCase):
    def test_not_run_when_not_opted_in(self) -> None:
        status, blockers = gate_report.evaluate_status(
            opted_in=False,
            smoke=False,
            covers_all_dependencies=True,
            dependency_blockers=[],
            redaction_ok=True,
        )
        self.assertEqual(status, "not_run")
        self.assertIn("MARKHAND_HANGING_SOAK!=1", blockers)

    def test_smoke_run_cannot_pass_but_is_not_hard_fail(self) -> None:
        status, blockers = gate_report.evaluate_status(
            opted_in=True,
            smoke=True,
            covers_all_dependencies=True,
            dependency_blockers=[],
            redaction_ok=True,
        )
        self.assertEqual(status, "incomplete")
        self.assertIn("smoke_non_qualifying_sustain", blockers)

    def test_clean_full_run_passes(self) -> None:
        status, blockers = gate_report.evaluate_status(
            opted_in=True,
            smoke=False,
            covers_all_dependencies=True,
            dependency_blockers=[],
            redaction_ok=True,
        )
        self.assertEqual(status, "pass")
        self.assertEqual(blockers, [])

    def test_redaction_failure_is_hard(self) -> None:
        status, _blockers = gate_report.evaluate_status(
            opted_in=True,
            smoke=False,
            covers_all_dependencies=True,
            dependency_blockers=[],
            redaction_ok=False,
        )
        self.assertEqual(status, "fail")


class PauseGuardArmRestoreTests(unittest.TestCase):
    """Exercise the arm/pause/restore state machine against a fake docker runner."""

    def _make_runner(self, script: dict[str, list]):
        """script maps a command tuple prefix -> list of CompletedProcess-likes
        returned in order for repeated calls with that prefix."""

        calls: list[list[str]] = []

        class _Proc:
            def __init__(self, returncode: int, stdout: str = ""):
                self.returncode = returncode
                self.stdout = stdout
                self.stderr = ""

        def runner(args, **_kwargs):
            calls.append(args)
            key = tuple(args[1:3])  # e.g. ("inspect", "-f") or ("pause",) etc.
            for prefix, responses in script.items():
                if tuple(args[1 : 1 + len(prefix)]) == prefix:
                    if responses:
                        return responses.pop(0)
                    return _Proc(0)
            return _Proc(0)

        return runner, calls, _Proc

    def test_normal_arm_pause_restore_confirms(self) -> None:
        runner, calls, Proc = self._make_runner(
            {
                ("inspect",): [
                    Proc(0, "true false"),  # precondition check: running, not paused
                    Proc(0, "true true"),  # after pause: running, paused
                    Proc(0, "true false"),  # after unpause: running, not paused
                ],
                ("pause",): [Proc(0)],
                ("unpause",): [Proc(0)],
            }
        )
        guard = failpoint.PauseGuard(service="postgres", container_id="c" * 12, runner=runner)
        guard.arm_and_pause()
        self.assertTrue(guard.armed)
        self.assertTrue(guard.pause_confirmed)
        result = guard.restore_if_armed()
        self.assertTrue(result["restoredConfirmed"])
        self.assertFalse(guard.armed)
        # Idempotent: calling again while disarmed is a no-op, not a re-pause.
        second = guard.restore_if_armed()
        self.assertTrue(second["restoreSkipped"])

    def test_abort_mid_sustain_still_restores(self) -> None:
        """Simulates an uncaught exception during the sustain loop: the
        caller's `finally: guard.restore_if_armed()` (see run_dependency)
        must still run and confirm the unpause, exactly as if atexit had
        fired."""
        runner, calls, Proc = self._make_runner(
            {
                ("inspect",): [
                    Proc(0, "true false"),
                    Proc(0, "true true"),
                    Proc(0, "true false"),
                ],
                ("pause",): [Proc(0)],
                ("unpause",): [Proc(0)],
            }
        )
        guard = failpoint.PauseGuard(service="qdrant", container_id="d" * 12, runner=runner)
        try:
            guard.arm_and_pause()
            raise RuntimeError("simulated crash mid-sustain")
        except RuntimeError:
            pass
        finally:
            restore = guard.restore_if_armed()
        self.assertTrue(restore["restoredConfirmed"])
        self.assertFalse(guard.armed)

    def test_failed_unpause_leaves_armed_and_unconfirmed(self) -> None:
        runner, calls, Proc = self._make_runner(
            {
                ("inspect",): [
                    Proc(0, "true false"),  # precondition
                    Proc(0, "true true"),  # after pause
                    Proc(0, "true true"),  # after failed unpause: still paused
                ],
                ("pause",): [Proc(0)],
                ("unpause",): [Proc(1)],  # unpause command itself fails
            }
        )
        guard = failpoint.PauseGuard(service="minio", container_id="e" * 12, runner=runner)
        guard.arm_and_pause()
        result = guard.restore_if_armed()
        self.assertFalse(result["restoredConfirmed"])
        # Armed stays true: a future retry must still attempt the restore
        # rather than assuming it already happened.
        self.assertTrue(guard.armed)

    def test_precondition_not_running_refuses_to_pause(self) -> None:
        runner, calls, Proc = self._make_runner(
            {("inspect",): [Proc(0, "false false")]}
        )
        guard = failpoint.PauseGuard(service="postgres", container_id="f" * 12, runner=runner)
        with self.assertRaises(failpoint.FailpointError):
            guard.arm_and_pause()
        self.assertFalse(guard.armed)
        self.assertFalse(any(a[:1] == ["pause"] for a in calls))

    def test_resolve_target_container_rejects_non_allowlisted_service(self) -> None:
        with self.assertRaises(failpoint.FailpointError):
            failpoint.resolve_target_container(
                service="worker-convert", allowed_ids={"worker-convert": "a" * 12}
            )

    def test_resolve_target_container_requires_discovered_id(self) -> None:
        with self.assertRaises(failpoint.FailpointError):
            failpoint.resolve_target_container(service="postgres", allowed_ids={})


class BlackholeHttpTimeoutTests(unittest.TestCase):
    """Real (non-mocked) local socket that accepts a connection and never
    answers — same technique as
    crates/server/src/services/readiness.rs::blackhole_listener — to prove
    sample_endpoint's client-side timeout actually bounds a genuine hang."""

    def _start_blackhole(self):
        server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        server.bind(("127.0.0.1", 0))
        server.listen(8)
        server.settimeout(0.2)
        port = server.getsockname()[1]
        stop = threading.Event()
        held: list[socket.socket] = []

        def serve() -> None:
            while not stop.is_set():
                try:
                    conn, _addr = server.accept()
                except socket.timeout:
                    continue
                except OSError:
                    break
                # Accept but never read/write/close: the connection is held
                # open forever, exactly like readiness.rs's blackhole test.
                held.append(conn)

        thread = threading.Thread(target=serve, daemon=True)
        thread.start()
        return port, server, stop, thread, held

    def test_sample_endpoint_times_out_on_a_real_hung_socket(self) -> None:
        port, server, stop, thread, held = self._start_blackhole()
        try:
            timeout_seconds = 1.0
            started = time.monotonic()
            sample = sample_endpoint(
                f"http://127.0.0.1:{port}/api/v1/health/ready",
                timeout_seconds=timeout_seconds,
            )
            elapsed = time.monotonic() - started
            self.assertIsNone(sample["httpStatus"])
            self.assertIsNotNone(sample["error"])
            # Bounded: the client must not wait meaningfully longer than the
            # timeout it was given, mirroring the readiness.rs assertion
            # `started.elapsed() <= PER_PROBE_DEADLINE + Duration::from_millis(750)`.
            self.assertLessEqual(elapsed, timeout_seconds + 1.5)
        finally:
            stop.set()
            thread.join(timeout=2.0)
            for conn in held:
                try:
                    conn.close()
                except OSError:
                    pass
            server.close()


if __name__ == "__main__":
    unittest.main()
