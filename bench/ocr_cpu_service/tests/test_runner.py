from __future__ import annotations

import json
import os
import signal
import sys
import time
from pathlib import Path

import psutil
import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

from benchmark.candidates import CommandCandidateSpec  # noqa: E402
from benchmark.corpus import BenchmarkPage  # noqa: E402
from benchmark.run import (  # noqa: E402
    _isolated_worker,
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
