from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1]))

from benchmark.candidates import CommandCandidateSpec  # noqa: E402
from benchmark.corpus import BenchmarkPage  # noqa: E402
from benchmark.run import (  # noqa: E402
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
