from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path
from unittest.mock import Mock, patch

import psutil
import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

from benchmark.candidates import CommandCandidateSpec  # noqa: E402
from benchmark.corpus import BenchmarkPage  # noqa: E402
from benchmark.run import (  # noqa: E402
    CandidateResourceLimitError,
    CandidateResourceSamplingError,
    CandidateWorkerCleanupError,
    CandidateWorkerProtocolError,
    IsolatedCandidateWorker,
    _isolated_worker,
    _process_tree_rss,
    run_candidate,
    sanitized_candidate_environment,
)


def _benchmark_page(path: Path, *, reference: str = "xin chào") -> BenchmarkPage:
    return BenchmarkPage(
        source_id="fixture",
        source_sha256="a" * 64,
        stratum="real-scan",
        page_number=1,
        path=path,
        reference=reference,
    )


def test_runs_arbitrary_candidate_id_without_shell(tmp_path: Path) -> None:
    recognizer = tmp_path / "fixture recognizer.py"
    recognizer.write_text(
        "from pathlib import Path\n"
        "import sys\n"
        "print(Path(sys.argv[1]).read_text(encoding='utf-8'))\n",
        encoding="utf-8",
    )
    input_path = tmp_path / "page; touch shell-was-used"
    input_path.write_text("xin chào", encoding="utf-8")
    spec = CommandCandidateSpec(
        id="future-preprocess-a",
        label="Future preprocess A",
        argv=(sys.executable, str(recognizer), "{input}"),
        environment=sanitized_candidate_environment(cpu_threads=1),
        provenance={},
    )

    result = run_candidate(
        spec,
        _benchmark_page(input_path),
        timeout_seconds=5.0,
        max_rss_bytes=1024 * 1024 * 1024,
    )

    assert result.candidate_id == "future-preprocess-a"
    assert result.success
    assert result.record["cer"] == 0.0
    assert not (tmp_path / "shell-was-used").exists()


def test_candidate_result_keeps_recognized_text_memory_only(tmp_path: Path) -> None:
    recognizer = tmp_path / "recognizer.py"
    recognizer.write_text("print('private recognized text')\n", encoding="utf-8")
    spec = CommandCandidateSpec(
        id="memory-only",
        label="Memory only",
        argv=(sys.executable, str(recognizer), "{input}"),
        environment=sanitized_candidate_environment(cpu_threads=1),
        provenance={},
    )

    result = run_candidate(
        spec,
        _benchmark_page(tmp_path / "page.png", reference="private recognized text"),
        timeout_seconds=5.0,
        max_rss_bytes=1024 * 1024 * 1024,
    )

    serialized = json.dumps(result.record)
    assert "private recognized text" not in serialized
    assert "text" not in result.record


def test_normal_unicode_output_within_hard_cap(tmp_path: Path) -> None:
    recognizer = tmp_path / "unicode-recognizer.py"
    recognizer.write_text(
        "print('Tiếng Việt có dấu: Trường Sa')\n",
        encoding="utf-8",
    )
    spec = CommandCandidateSpec(
        id="unicode",
        label="Unicode",
        argv=(sys.executable, str(recognizer), "{input}"),
        environment=sanitized_candidate_environment(cpu_threads=1),
        provenance={},
    )

    result = run_candidate(
        spec,
        _benchmark_page(
            tmp_path / "page.png",
            reference="Tiếng Việt có dấu: Trường Sa",
        ),
        timeout_seconds=5.0,
        max_rss_bytes=1024 * 1024 * 1024,
        max_output_bytes=4096,
    )

    assert result.success
    assert result.record["cer"] == 0.0
    assert result.metadata["output_limits"]["stdout_bytes"] == 4096
    assert result.metadata["output_limits"]["stderr_bytes"] == 4096


@pytest.mark.parametrize("stream", [1, 2], ids=["stdout", "stderr"])
def test_unlimited_candidate_output_is_hard_bounded_and_sanitized(
    tmp_path: Path, stream: int
) -> None:
    canary = "OUTPUT_CANARY_MUST_NOT_ESCAPE"
    recognizer = tmp_path / "unlimited-output.py"
    recognizer.write_text(
        "import os\n"
        f"chunk = ({canary!r} * 128).encode()\n"
        "while True:\n"
        f"    os.write({stream}, chunk)\n",
        encoding="utf-8",
    )
    spec = CommandCandidateSpec(
        id="unlimited-output",
        label="Unlimited output",
        argv=(sys.executable, str(recognizer), "{input}"),
        environment={
            **sanitized_candidate_environment(cpu_threads=1),
            "OCR_CANARY_SECRET": canary,
        },
        provenance={},
    )

    result = run_candidate(
        spec,
        _benchmark_page(tmp_path / "page.png"),
        timeout_seconds=5.0,
        max_rss_bytes=1024 * 1024 * 1024,
        max_output_bytes=4096,
    )
    serialized = json.dumps(
        {"record": result.record, "metadata": result.metadata},
        ensure_ascii=False,
    )

    assert not result.success
    assert result.record["error_kind"] == "output_limit"
    assert canary not in serialized
    assert "OCR_CANARY_SECRET" in result.metadata["environment_variable_names"]


def test_output_limit_does_not_restrict_candidate_internal_files(
    tmp_path: Path,
) -> None:
    recognizer = tmp_path / "internal-file.py"
    internal = tmp_path / "internal.bin"
    recognizer.write_text(
        "from pathlib import Path\n"
        f"Path({str(internal)!r}).write_bytes(b'x' * 8192)\n"
        "print('xin chào')\n",
        encoding="utf-8",
    )
    spec = CommandCandidateSpec(
        id="internal-file",
        label="Internal file",
        argv=(sys.executable, str(recognizer), "{input}"),
        environment=sanitized_candidate_environment(cpu_threads=1),
        provenance={},
    )

    result = run_candidate(
        spec,
        _benchmark_page(tmp_path / "page.png", reference="xin chào"),
        timeout_seconds=5.0,
        max_rss_bytes=1024 * 1024 * 1024,
        max_output_bytes=4096,
    )

    assert result.success
    assert result.record["cer"] == 0.0
    assert internal.stat().st_size == 8192


def test_candidate_provenance_is_thawed_only_for_json_output(
    tmp_path: Path,
) -> None:
    recognizer = tmp_path / "recognizer.py"
    recognizer.write_text("print('reference')\n", encoding="utf-8")
    spec = CommandCandidateSpec(
        id="frozen",
        label="Frozen",
        argv=(sys.executable, str(recognizer), "{input}"),
        environment=sanitized_candidate_environment(cpu_threads=1),
        provenance={
            "build": {
                "assets": [{"name": "recognizer"}],
                "features": {"offline", "cpu"},
            }
        },
    )

    result = run_candidate(
        spec,
        _benchmark_page(tmp_path / "page.png", reference="reference"),
        timeout_seconds=5.0,
        max_rss_bytes=1024 * 1024 * 1024,
    )

    assert json.loads(json.dumps(result.metadata))["build"] == {
        "assets": [{"name": "recognizer"}],
        "features": ["cpu", "offline"],
    }


def test_candidate_environment_has_no_model_source_assumptions() -> None:
    assert sanitized_candidate_environment(cpu_threads=2) == {
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "OMP_NUM_THREADS": "2",
        "OPENBLAS_NUM_THREADS": "2",
        "PATH": "/usr/local/bin:/usr/bin:/bin",
        "PYTHONNOUSERSITE": "1",
        "PYTHONPATH": "bench/ocr_cpu_service",
    }


def test_timeout_kills_worker_and_recognizer_process_tree(tmp_path: Path) -> None:
    process_ids = tmp_path / "process-ids.json"
    recognizer = tmp_path / "spawning-recognizer.py"
    recognizer.write_text(
        "import json\n"
        "import os\n"
        "from pathlib import Path\n"
        "import subprocess\n"
        "import sys\n"
        "import time\n"
        "child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])\n"
        "Path(sys.argv[1]).write_text(json.dumps({\n"
        "    'recognizer': os.getpid(), 'child': child.pid\n"
        "}), encoding='utf-8')\n"
        "time.sleep(60)\n",
        encoding="utf-8",
    )
    spec = CommandCandidateSpec(
        id="timeout-tree",
        label="Timeout tree",
        argv=(sys.executable, str(recognizer), "{input}"),
        environment=sanitized_candidate_environment(cpu_threads=1),
        provenance={},
    )
    pids: dict[str, int] = {}

    try:
        result = run_candidate(
            spec,
            _benchmark_page(process_ids),
            timeout_seconds=0.25,
            max_rss_bytes=1024 * 1024 * 1024,
        )
        pids = json.loads(process_ids.read_text(encoding="utf-8"))

        assert not result.success
        assert result.record["error_kind"] == "timeout"
        deadline = time.monotonic() + 3.0
        while time.monotonic() < deadline and any(
            psutil.pid_exists(pid) for pid in pids.values()
        ):
            time.sleep(0.02)
        assert all(not psutil.pid_exists(pid) for pid in pids.values())
    finally:
        for pid in pids.values():
            if psutil.pid_exists(pid):
                os.kill(pid, signal.SIGKILL)


def test_timeout_sigkills_descendant_that_ignores_sigterm(tmp_path: Path) -> None:
    process_ids = tmp_path / "process-ids.json"
    recognizer = tmp_path / "stubborn-recognizer.py"
    recognizer.write_text(
        "import json\n"
        "import os\n"
        "from pathlib import Path\n"
        "import subprocess\n"
        "import sys\n"
        "import time\n"
        "child = subprocess.Popen([\n"
        "    sys.executable, '-c',\n"
        "    'import signal, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(60)',\n"
        "])\n"
        "Path(sys.argv[1]).write_text(json.dumps({\n"
        "    'recognizer': os.getpid(), 'child': child.pid\n"
        "}), encoding='utf-8')\n"
        "time.sleep(60)\n",
        encoding="utf-8",
    )
    spec = CommandCandidateSpec(
        id="stubborn-timeout-tree",
        label="Stubborn timeout tree",
        argv=(sys.executable, str(recognizer), "{input}"),
        environment=sanitized_candidate_environment(cpu_threads=1),
        provenance={},
    )
    pids: dict[str, int] = {}

    try:
        result = run_candidate(
            spec,
            _benchmark_page(process_ids),
            timeout_seconds=0.25,
            max_rss_bytes=1024 * 1024 * 1024,
        )
        pids = json.loads(process_ids.read_text(encoding="utf-8"))

        assert not result.success
        assert result.record["error_kind"] == "timeout"
        deadline = time.monotonic() + 3.0
        while time.monotonic() < deadline and any(
            psutil.pid_exists(pid) for pid in pids.values()
        ):
            time.sleep(0.02)
        assert all(not psutil.pid_exists(pid) for pid in pids.values())
    finally:
        for pid in pids.values():
            if psutil.pid_exists(pid):
                os.kill(pid, signal.SIGKILL)


def test_normal_close_kills_daemonized_candidate_descendant_and_is_idempotent(
    tmp_path: Path,
) -> None:
    child_pid_path = tmp_path / "child.pid"
    recognizer = tmp_path / "daemonizing-recognizer.py"
    recognizer.write_text(
        "from pathlib import Path\n"
        "import subprocess\n"
        "import sys\n"
        "import time\n"
        "ready = Path(str(sys.argv[1]) + '.ready')\n"
        "child = subprocess.Popen([\n"
        "    sys.executable, '-c',\n"
        "    'import signal, sys, time; from pathlib import Path; "
        "signal.signal(signal.SIGTERM, signal.SIG_IGN); "
        "Path(sys.argv[1]).write_text(\"ready\"); time.sleep(60)', str(ready),\n"
        "], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)\n"
        "while not ready.exists():\n"
        "    time.sleep(0.005)\n"
        "Path(sys.argv[1]).write_text(str(child.pid), encoding='utf-8')\n"
        "print('xin chào')\n",
        encoding="utf-8",
    )
    spec = CommandCandidateSpec(
        id="daemonizing",
        label="Daemonizing",
        argv=(sys.executable, str(recognizer), "{input}"),
        environment=sanitized_candidate_environment(cpu_threads=1),
        provenance={},
    )
    worker = _isolated_worker(
        spec,
        timeout_seconds=5.0,
        max_output_bytes=4096,
        max_rss_bytes=1024 * 1024 * 1024,
    )
    child_pid = 0

    try:
        measurement = worker.recognize(
            _benchmark_page(child_pid_path, reference="xin chào")
        )
        child_pid = int(child_pid_path.read_text(encoding="utf-8"))
        assert measurement.text.strip() == "xin chào"

        worker.close()
        worker.close()

        deadline = time.monotonic() + 3.0
        while time.monotonic() < deadline and psutil.pid_exists(child_pid):
            time.sleep(0.02)
        assert not psutil.pid_exists(child_pid)
    finally:
        worker.close()
        if child_pid and psutil.pid_exists(child_pid):
            os.kill(child_pid, signal.SIGKILL)


def test_isolated_worker_allows_stricter_per_request_timeout(
    tmp_path: Path,
) -> None:
    recognizer = tmp_path / "slow-recognizer.py"
    recognizer.write_text(
        "import time\n"
        "time.sleep(2)\n"
        "print('too late')\n",
        encoding="utf-8",
    )
    spec = CommandCandidateSpec(
        id="per-request-timeout",
        label="Per-request timeout",
        argv=(sys.executable, str(recognizer), "{input}"),
        environment=sanitized_candidate_environment(cpu_threads=1),
        provenance={},
    )
    worker = _isolated_worker(
        spec,
        timeout_seconds=5.0,
        max_output_bytes=4096,
        max_rss_bytes=1024 * 1024 * 1024,
    )
    try:
        with pytest.raises(TimeoutError):
            worker.recognize(
                _benchmark_page(tmp_path / "page.png"),
                timeout_seconds=0.1,
            )
    finally:
        worker.close()


def _capturing_popen(processes: list[subprocess.Popen[str]]):
    real_popen = subprocess.Popen

    def capture(*args, **kwargs):
        process = real_popen(*args, **kwargs)
        processes.append(process)
        return process

    return capture


def _assert_processes_gone(processes: list[subprocess.Popen[str]]) -> None:
    deadline = time.monotonic() + 3.0
    while time.monotonic() < deadline and any(
        psutil.pid_exists(process.pid) for process in processes
    ):
        time.sleep(0.02)
    assert processes
    assert all(not psutil.pid_exists(process.pid) for process in processes)


def test_worker_startup_rss_cap_is_hard_and_constructor_cleans_process() -> None:
    spec = CommandCandidateSpec(
        id="startup-rss",
        label="Startup RSS",
        argv=(sys.executable, "-c", "print('unused')", "{input}"),
        environment=sanitized_candidate_environment(cpu_threads=1),
        provenance={},
    )
    processes: list[subprocess.Popen[str]] = []
    with (
        patch(
            "benchmark.run.subprocess.Popen",
            side_effect=_capturing_popen(processes),
        ),
        pytest.raises(CandidateResourceLimitError) as caught,
    ):
        _isolated_worker(
            spec,
            timeout_seconds=5.0,
            max_output_bytes=4096,
            max_rss_bytes=1,
        )
    assert caught.value.error_kind == "resource_limit"
    _assert_processes_gone(processes)


def test_request_rss_cap_kills_worker_and_candidate_tree(tmp_path: Path) -> None:
    process_ids = tmp_path / "rss-process-ids.json"
    recognizer = tmp_path / "memory-recognizer.py"
    recognizer.write_text(
        "import json\n"
        "import os\n"
        "from pathlib import Path\n"
        "import time\n"
        "Path(os.environ['PROCESS_IDS']).write_text(\n"
        "    json.dumps({'recognizer': os.getpid()}), encoding='utf-8'\n"
        ")\n"
        "allocation = bytearray(100 * 1024 * 1024)\n"
        "time.sleep(60)\n",
        encoding="utf-8",
    )
    environment = sanitized_candidate_environment(cpu_threads=1)
    environment["PROCESS_IDS"] = str(process_ids)
    spec = CommandCandidateSpec(
        id="request-rss",
        label="Request RSS",
        argv=(sys.executable, str(recognizer), "{input}"),
        environment=environment,
        provenance={},
    )
    worker = _isolated_worker(
        spec,
        timeout_seconds=5.0,
        max_output_bytes=4096,
        max_rss_bytes=80 * 1024 * 1024,
    )
    recognizer_pid = 0
    try:
        with pytest.raises(CandidateResourceLimitError) as caught:
            worker.recognize(_benchmark_page(tmp_path / "page.png"))
        assert caught.value.error_kind == "resource_limit"
        recognizer_pid = json.loads(
            process_ids.read_text(encoding="utf-8")
        )["recognizer"]
        deadline = time.monotonic() + 3.0
        while time.monotonic() < deadline and psutil.pid_exists(recognizer_pid):
            time.sleep(0.02)
        assert not psutil.pid_exists(recognizer_pid)
    finally:
        worker.close()
        if recognizer_pid and psutil.pid_exists(recognizer_pid):
            os.kill(recognizer_pid, signal.SIGKILL)


def test_process_tree_sampling_error_fails_closed_and_cleans_worker() -> None:
    spec = CommandCandidateSpec(
        id="sampling-error",
        label="Sampling error",
        argv=(sys.executable, "-c", "print('unused')", "{input}"),
        environment=sanitized_candidate_environment(cpu_threads=1),
        provenance={},
    )
    processes: list[subprocess.Popen[str]] = []
    with (
        patch(
            "benchmark.run.subprocess.Popen",
            side_effect=_capturing_popen(processes),
        ),
        patch(
            "benchmark.run._process_tree_rss",
            side_effect=psutil.AccessDenied(pid=123),
        ),
        pytest.raises(CandidateResourceSamplingError) as caught,
    ):
        _isolated_worker(
            spec,
            timeout_seconds=5.0,
            max_output_bytes=4096,
            max_rss_bytes=1024 * 1024 * 1024,
        )
    assert caught.value.error_kind == "resource_sampling"
    _assert_processes_gone(processes)


def test_constructor_cleans_process_after_malformed_ready_metadata() -> None:
    processes: list[subprocess.Popen[str]] = []
    with (
        patch(
            "benchmark.run.subprocess.Popen",
            side_effect=_capturing_popen(processes),
        ),
        pytest.raises(CandidateWorkerProtocolError) as caught,
    ):
        IsolatedCandidateWorker(
            candidate_id="malformed-ready",
            label="Malformed ready",
            command=(
                sys.executable,
                "-c",
                "import json, time; "
                "print(json.dumps({'event':'ready'}), flush=True); "
                "time.sleep(60)",
            ),
            environment=sanitized_candidate_environment(cpu_threads=1),
            timeout_seconds=5.0,
            max_output_bytes=4096,
            max_rss_bytes=1024 * 1024 * 1024,
        )
    assert caught.value.error_kind == "worker_protocol"
    _assert_processes_gone(processes)


def test_constructor_cleans_process_after_malformed_resource_metadata() -> None:
    processes: list[subprocess.Popen[str]] = []
    with (
        patch(
            "benchmark.run.subprocess.Popen",
            side_effect=_capturing_popen(processes),
        ),
        patch(
            "benchmark.run._read_event_with_process_tree_rss",
            return_value=(
                {"event": "ready", "candidate_seconds": 0.01},
                {"peak_rss_bytes": "not-an-integer"},
            ),
        ),
        pytest.raises(CandidateWorkerProtocolError) as caught,
    ):
        IsolatedCandidateWorker(
            candidate_id="malformed-resource",
            label="Malformed resource",
            command=(sys.executable, "-c", "import time; time.sleep(60)"),
            environment=sanitized_candidate_environment(cpu_threads=1),
            timeout_seconds=5.0,
            max_output_bytes=4096,
            max_rss_bytes=1024 * 1024 * 1024,
        )
    assert caught.value.error_kind == "worker_protocol"
    _assert_processes_gone(processes)


def test_run_candidate_maps_hard_rss_limit_for_existing_user(
    tmp_path: Path,
) -> None:
    recognizer = tmp_path / "memory-recognizer.py"
    recognizer.write_text(
        "import time\n"
        "allocation = bytearray(100 * 1024 * 1024)\n"
        "time.sleep(60)\n",
        encoding="utf-8",
    )
    spec = CommandCandidateSpec(
        id="existing-user-rss",
        label="Existing user RSS",
        argv=(sys.executable, str(recognizer), "{input}"),
        environment=sanitized_candidate_environment(cpu_threads=1),
        provenance={},
    )
    result = run_candidate(
        spec,
        _benchmark_page(tmp_path / "page.png"),
        timeout_seconds=5.0,
        max_rss_bytes=80 * 1024 * 1024,
        max_output_bytes=4096,
    )
    assert not result.success
    assert result.record["error_kind"] == "resource_limit"
    assert result.record["resource_limit_violation"] is True


@pytest.mark.parametrize(
    "gone",
    [
        psutil.NoSuchProcess(pid=101),
        psutil.ZombieProcess(pid=101),
        ProcessLookupError("descendant exited"),
    ],
)
def test_process_tree_rss_skips_only_confirmed_gone_descendant(
    gone: BaseException,
) -> None:
    root = Mock()
    root.memory_info.return_value = Mock(rss=12_345)
    child = Mock()
    child.memory_info.side_effect = gone
    root.children.return_value = [child]

    assert _process_tree_rss(root) == 12_345
    root.memory_info.assert_called_once_with()
    root.children.assert_called_once_with(recursive=True)


@pytest.mark.parametrize(
    "root_failure",
    [
        psutil.NoSuchProcess(pid=100),
        psutil.ZombieProcess(pid=100),
        ProcessLookupError("root exited"),
    ],
)
def test_process_tree_rss_root_disappearance_remains_failure(
    root_failure: BaseException,
) -> None:
    root = Mock()
    root.memory_info.side_effect = root_failure

    with pytest.raises(type(root_failure)):
        _process_tree_rss(root)
    root.children.assert_not_called()


@pytest.mark.parametrize(
    ("failure_location", "failure"),
    [
        ("enumeration", psutil.AccessDenied(pid=100)),
        ("descendant", psutil.AccessDenied(pid=101)),
    ],
)
def test_process_tree_rss_access_denied_remains_fail_closed(
    failure_location: str,
    failure: BaseException,
) -> None:
    root = Mock()
    root.memory_info.return_value = Mock(rss=12_345)
    child = Mock()
    if failure_location == "enumeration":
        root.children.side_effect = failure
    else:
        root.children.return_value = [child]
        child.memory_info.side_effect = failure

    with pytest.raises(psutil.AccessDenied):
        _process_tree_rss(root)


def test_close_uses_independent_sigkill_fallback_after_persistent_failure(
    tmp_path: Path,
) -> None:
    recognizer = tmp_path / "recognizer.py"
    recognizer.write_text("print('ready')\n", encoding="utf-8")
    spec = CommandCandidateSpec(
        id="close-fallback",
        label="Close fallback",
        argv=(sys.executable, str(recognizer), "{input}"),
        environment=sanitized_candidate_environment(cpu_threads=1),
        provenance={},
    )
    worker = _isolated_worker(
        spec,
        timeout_seconds=5.0,
        max_rss_bytes=1024 * 1024 * 1024,
        max_output_bytes=4096,
    )
    root_pid = worker._process.pid
    try:
        with patch(
            "benchmark.run._terminate_process_group",
            side_effect=OSError("persistent high-level close failure"),
        ):
            worker.close(timeout_seconds=1.0)
        deadline = time.monotonic() + 3.0
        while time.monotonic() < deadline and psutil.pid_exists(root_pid):
            time.sleep(0.02)
        assert not psutil.pid_exists(root_pid)
        with pytest.raises(ProcessLookupError):
            os.killpg(root_pid, 0)
    finally:
        if psutil.pid_exists(root_pid):
            try:
                os.killpg(root_pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            worker._process.wait()


def test_close_verification_failure_is_typed_and_sanitized(
    tmp_path: Path,
) -> None:
    recognizer = tmp_path / "recognizer.py"
    recognizer.write_text("print('ready')\n", encoding="utf-8")
    spec = CommandCandidateSpec(
        id="close-verification",
        label="Close verification",
        argv=(sys.executable, str(recognizer), "{input}"),
        environment=sanitized_candidate_environment(cpu_threads=1),
        provenance={},
    )
    worker = _isolated_worker(
        spec,
        timeout_seconds=5.0,
        max_rss_bytes=1024 * 1024 * 1024,
        max_output_bytes=4096,
    )
    try:
        with (
            patch(
                "benchmark.run._terminate_process_group",
                side_effect=OSError(f"PRIVATE_HIGH_LEVEL:{tmp_path}"),
            ),
            patch(
                "benchmark.run._kill_process_group_and_verify",
                return_value=False,
                create=True,
            ),
            pytest.raises(CandidateWorkerCleanupError) as caught,
        ):
            worker.close(timeout_seconds=1.0)
        assert caught.value.error_kind == "worker_cleanup"
        assert "PRIVATE" not in str(caught.value)
        assert str(tmp_path) not in str(caught.value)
    finally:
        if psutil.pid_exists(worker._process.pid):
            try:
                os.killpg(worker._process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            worker._process.wait()
