# OCR Benchmark Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the rejected HTTP/Paddle spike while retaining a reproducible, model-neutral Vietnamese OCR benchmark and its local corpus.

**Architecture:** Keep the historical `bench/ocr_cpu_service/` directory so the ignored `.data/corpus/` does not move or disappear, but rebrand its contents as a benchmark. Replace service/model coupling with command-candidate, corpus, rendering, metrics, runner, and report modules; retain the Phase A negative report as archived evidence.

**Tech Stack:** Python 3.12, pypdfium2/PDFium, Pillow, psutil, pytest, standard-library subprocess/JSON, existing Rust `fileconv`.

## Global Constraints

- Do not delete or move `bench/ocr_cpu_service/.data/corpus/`.
- Do not commit downloaded PDFs/images, model files, virtualenvs, caches, OCR text, or logs.
- Remove FastAPI, PaddleOCR/PaddlePaddle/PaddleX, Uvicorn, multipart, and HTTP client code/dependencies.
- Preserve the checked-in Phase A JSON/Markdown report and its STOP conclusion.
- The generic runner executes argv arrays directly; it never invokes a shell.
- Ground truth is evaluator-only and recognized document text is never stored in reports.
- Production Rust conversion behavior is unchanged by this cleanup.

---

### Task 1: Model-neutral benchmark interfaces

**Files:**
- Create: `bench/ocr_cpu_service/benchmark/candidates.py`
- Create: `bench/ocr_cpu_service/benchmark/corpus.py`
- Create: `bench/ocr_cpu_service/benchmark/render.py`
- Create: `bench/ocr_cpu_service/tests/test_candidates.py`
- Modify: `bench/ocr_cpu_service/tests/test_render.py`

**Interfaces:**
- Produces: `CommandCandidateSpec(id, label, argv, environment, provenance)`
- Produces: `render_argv(spec, input_path) -> list[str]`
- Produces: `BenchmarkPage` and verified corpus-page loaders
- Produces: bounded PDFium `open_pdf()` and `render_page()`

- [ ] **Step 1: Write failing command-candidate tests**

```python
def test_renders_argv_without_a_shell(tmp_path):
    spec = CommandCandidateSpec(
        id="baseline",
        label="Baseline",
        argv=("fileconv", "one", "{input}"),
        environment={"FILECONV_TESSDATA": "/models"},
        provenance={},
    )
    assert render_argv(spec, tmp_path / "a b.png") == [
        "fileconv", "one", str(tmp_path / "a b.png")
    ]


def test_rejects_missing_or_multiple_input_placeholders():
    with pytest.raises(ValueError, match="exactly one"):
        CommandCandidateSpec("x", "X", ("tool",), {}, {})
```

- [ ] **Step 2: Run RED**

Run: `python3 -m pytest bench/ocr_cpu_service/tests/test_candidates.py -q`  
Expected: import failure because `benchmark.candidates` does not exist.

- [ ] **Step 3: Implement immutable candidate validation**

Reject empty IDs/labels/argv, require exactly one complete `{input}` argument, reject
unknown placeholders, require string-only environment values, and execute only with
`subprocess` argv plus a sanitized environment assembled by the runner.

- [ ] **Step 4: Move corpus/render helpers behind benchmark modules**

Move the reusable `BenchmarkPage`, verified metadata loading, deterministic PDF samples,
historical anchors, `RenderLimits`, `open_pdf`, and `render_page` behavior without
changing bounds or checksums. Update tests to import the new modules.

- [ ] **Step 5: Run retained unit tests**

Run:
`python3 -m pytest bench/ocr_cpu_service/tests/test_candidates.py bench/ocr_cpu_service/tests/test_corpus.py bench/ocr_cpu_service/tests/test_render.py bench/ocr_cpu_service/tests/test_metrics.py -q`  
Expected: all pass without network, model cache, or downloaded documents.

- [ ] **Step 6: Commit**

```bash
git add bench/ocr_cpu_service/benchmark bench/ocr_cpu_service/tests
git commit -m "refactor(ocr): extract model-neutral benchmark interfaces"
```

### Task 2: Generic runner and report compatibility

**Files:**
- Modify: `bench/ocr_cpu_service/benchmark/worker.py`
- Modify: `bench/ocr_cpu_service/benchmark/run.py`
- Modify: `bench/ocr_cpu_service/benchmark/report.py`
- Modify: `bench/ocr_cpu_service/tests/test_report.py`
- Create: `bench/ocr_cpu_service/tests/test_runner.py`

**Interfaces:**
- Consumes: `CommandCandidateSpec`, `BenchmarkPage`, rendering, and metrics
- Produces: serial command candidate execution with text kept only in memory
- Produces: optional generic `comparison` decision in raw JSON
- Preserves: deterministic rendering of archived `reports/phase-a.json`

- [ ] **Step 1: Write failing generic-candidate runner test**

```python
def test_runs_arbitrary_candidate_id_without_shell(tmp_path):
    spec = candidate_spec(
        candidate_id="future-preprocess-a",
        argv=(sys.executable, str(FIXTURE_RECOGNIZER), "{input}"),
    )
    result = run_candidate(spec, benchmark_page(tmp_path))
    assert result.candidate_id == "future-preprocess-a"
    assert result.success
```

- [ ] **Step 2: Write failing generic report test**

```python
def test_report_uses_explicit_comparison_roles_not_candidate_names():
    payload = measured_payload(
        candidates=("control-a", "challenger-z"),
        comparison={"baseline": "control-a", "challenger": "challenger-z"},
    )
    markdown = render_report(payload)
    assert "control-a" in markdown
    assert "challenger-z" in markdown
```

- [ ] **Step 3: Verify RED**

Run:
`python3 -m pytest bench/ocr_cpu_service/tests/test_runner.py bench/ocr_cpu_service/tests/test_report.py -q`  
Expected: generic interfaces are missing or current Paddle-specific assumptions fail.

- [ ] **Step 4: Refactor runner**

Remove all model-directory arguments, model package version collection, Paddle worker
branches, and model-source environment variables. Define the current two fileconv
candidates as ordinary `CommandCandidateSpec` values. Keep process-tree RSS sampling,
timeouts, provenance, serial execution, PDF samples, and in-memory metrics.

- [ ] **Step 5: Refactor report**

Use explicit comparison roles when present. Keep a narrow legacy adapter for the
checked-in Phase A schema so regenerating the archived report remains exact. A run with
no challenger renders metrics and states that no adoption gate was configured.

- [ ] **Step 6: Verify reports and tests**

Run all benchmark/report tests, regenerate Phase A Markdown into a temporary file, and
compare it byte-for-byte with `reports/phase-a.md`.

- [ ] **Step 7: Commit**

```bash
git add bench/ocr_cpu_service/benchmark bench/ocr_cpu_service/tests
git commit -m "refactor(ocr): make benchmark candidates model-neutral"
```

### Task 3: Remove rejected service and Paddle dependencies

**Files:**
- Delete: `bench/ocr_cpu_service/markhand_ocr/`
- Delete: `bench/ocr_cpu_service/scripts/run_service.sh`
- Delete: `bench/ocr_cpu_service/tests/test_api.py`
- Delete: `bench/ocr_cpu_service/tests/test_markdown.py`
- Delete: `bench/ocr_cpu_service/tests/test_ordering.py`
- Delete: `bench/ocr_cpu_service/tests/test_paddle_backend.py`
- Delete: `bench/ocr_cpu_service/tests/test_service.py`
- Modify: `bench/ocr_cpu_service/pyproject.toml`
- Regenerate: `bench/ocr_cpu_service/pylock.toml`
- Rewrite: `bench/ocr_cpu_service/README.md`

**Interfaces:**
- Preserves: corpus downloader, generic benchmark CLI, tests, and archived report
- Removes: all HTTP/model runtime interfaces

- [ ] **Step 1: Add an absence/dependency regression test**

Add a static test that fails if tracked benchmark Python imports `fastapi`, `paddleocr`,
`paddle`, `paddlex`, `uvicorn`, `multipart`, or `httpx`, or if those distributions
appear in the benchmark lock.

- [ ] **Step 2: Verify RED**

Run the static test and confirm it reports current service/Paddle imports and lock entries.

- [ ] **Step 3: Delete service/model code and tests**

Remove only tracked files. Do not delete `.data/corpus/`. Leave ignored model/venv
directories untouched unless separately authorized.

- [ ] **Step 4: Minimize and lock dependencies**

Keep direct runtime dependencies only for Pillow, psutil, and pypdfium2; keep pytest as
the test extra. Regenerate `pylock.toml` for CPython 3.12/Linux x86_64 with wheel URLs
and SHA-256 hashes. Install the local package with `--no-build-isolation --no-deps`.

- [ ] **Step 5: Rewrite README**

Document benchmark-only setup, corpus restoration, baseline build/run, archived
PP-OCRv6 STOP result, retained local corpus path, and the fact that service/model code
was intentionally removed. Remove all service startup and live-model instructions.

- [ ] **Step 6: Run complete benchmark tests**

Run: `python3 -m pytest bench/ocr_cpu_service/tests -q`  
Expected: all retained tests pass with no model cache or network.

- [ ] **Step 7: Commit**

```bash
git add -A bench/ocr_cpu_service
git commit -m "refactor(ocr): remove rejected Paddle benchmark service"
```

### Task 4: Verification and handoff

**Files:**
- Modify only if evidence requires: `docs/superpowers/specs/2026-08-07-ocr-benchmark-cleanup-accuracy-design.md`
- Preserve unchanged: `bench/ocr_cpu_service/reports/phase-a.json`
- Preserve unchanged: `bench/ocr_cpu_service/reports/phase-a.md`

**Interfaces:**
- Produces: clean tracked benchmark and documented next-plan boundary

- [ ] **Step 1: Verify retained local corpus**

Record file count, total bytes, and checksum validation for
`bench/ocr_cpu_service/.data/corpus/`; do not print or commit document contents.

- [ ] **Step 2: Audit tracked files**

Fail if tracked files include PDF/image/model/cache/venv/OCR-output artifacts or removed
service/Paddle imports and distributions.

- [ ] **Step 3: Run quality gates**

```bash
python3 -m pytest bench/ocr_cpu_service/tests -q
python3 scripts/check-architecture-boundaries.py
python3 scripts/check-architecture-boundaries.py --self-test
cargo fmt --all -- --check
cargo metadata --locked --format-version 1 --no-deps
python3 scripts/check-dependency-policy.py
git diff --check
```

- [ ] **Step 4: Commit and push**

```bash
git add docs/superpowers bench/ocr_cpu_service
git commit -m "docs(ocr): hand off accuracy-first benchmark plan"
git push -u origin cursor/mineru-ocr-spike-e533
```
