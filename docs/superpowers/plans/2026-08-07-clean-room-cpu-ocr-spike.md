# Clean-room CPU OCR Service Spike Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build and measure an isolated CPU service that converts Vietnamese scanned PDFs with PP-OCRv6, then add layout/table/formula parsing only if the OCR gate passes.

**Architecture:** A benchmark-only Python package under `bench/ocr_cpu_service/` owns bounded PDF rendering, OCR orchestration, deterministic reading order, API serialization, corpus acquisition, and reports. PaddleOCR is accessed through its public API and models are cached outside git; no MinerU code or runtime is imported.

**Tech Stack:** Python 3.12, FastAPI, Pydantic, PyMuPDF, Pillow, PaddleOCR/PP-OCRv6, PaddlePaddle CPU, psutil, pytest, and httpx.

## Global Constraints

- Do not copy, translate, vendor, or import MinerU source code.
- Do not add Python/model dependencies to Rust crates or production worker images.
- Do not commit model binaries, downloaded PDFs, corpora, full OCR outputs, or secret-bearing logs.
- Use only allowlisted public corpus URLs with recorded license, byte ceiling, and SHA-256.
- Run all candidates on the same host and rendered pages.
- Stop after Phase A if aggregate real-scan CER does not improve by at least 20% relative to the better Tesseract baseline.
- Production web routing, OpenAPI changes, and converter-worker integration are out of scope.

---

### Task 1: Reproducible corpus acquisition

**Files:**
- Create: `bench/ocr_cpu_service/corpus/sources.json`
- Create: `bench/ocr_cpu_service/corpus/download.py`
- Create: `bench/ocr_cpu_service/tests/test_corpus.py`
- Modify: `.gitignore`

**Interfaces:**
- Produces: `load_sources(path: Path) -> list[CorpusSource]`
- Produces: `download_sources(manifest: Path, output: Path) -> list[DownloadedSource]`
- Produces: ignored directory `bench/ocr_cpu_service/.data/corpus/`

- [ ] **Step 1: Write failing manifest-validation tests**

```python
def test_rejects_source_without_license(tmp_path):
    manifest = tmp_path / "sources.json"
    manifest.write_text('[{"id":"bad","url":"https://example.com/a.pdf","sha256":"00"}]')
    with pytest.raises(ValueError, match="license"):
        load_sources(manifest)


def test_rejects_non_https_and_oversized_source(tmp_path):
    source = valid_source(url="http://example.com/a.pdf", max_bytes=0)
    with pytest.raises(ValueError):
        validate_source(source)
```

- [ ] **Step 2: Run the tests and verify RED**

Run: `python3 -m pytest bench/ocr_cpu_service/tests/test_corpus.py -q`  
Expected: import failure because `corpus.download` does not exist.

- [ ] **Step 3: Implement strict source loading and streaming download**

```python
@dataclass(frozen=True)
class CorpusSource:
    id: str
    url: str
    license: str
    sha256: str
    max_bytes: int
    kind: str


def validate_source(source: CorpusSource) -> None:
    if urlsplit(source.url).scheme != "https":
        raise ValueError(f"{source.id}: only HTTPS sources are allowed")
    if not source.license.strip():
        raise ValueError(f"{source.id}: license is required")
    if source.max_bytes <= 0:
        raise ValueError(f"{source.id}: max_bytes must be positive")
    if not re.fullmatch(r"[0-9a-f]{64}", source.sha256):
        raise ValueError(f"{source.id}: sha256 must be lowercase hex")
```

Download to an exclusive temporary file, enforce `Content-Length` when present, enforce
the streamed byte ceiling, verify SHA-256, and atomically replace the destination.

- [ ] **Step 4: Add reviewed sources**

Include the pinned `nrl-ai/vn-ocr-documents-eval` artifact URLs and small public-domain
Wikimedia/Wikisource PDFs. Add the official attachment for `89/2026/TT-BTC` only after
resolving it from Government document `docid=218974`; record whether inspection finds a
scan, native text, or mixed PDF. Do not substitute a third-party copy when the official
attachment is available.

- [ ] **Step 5: Run tests**

Run: `python3 -m pytest bench/ocr_cpu_service/tests/test_corpus.py -q`  
Expected: all corpus validation/downloader tests pass without network.

- [ ] **Step 6: Commit**

```bash
git add .gitignore bench/ocr_cpu_service/corpus bench/ocr_cpu_service/tests/test_corpus.py
git commit -m "test(ocr): add licensed Vietnamese scan corpus tooling"
```

### Task 2: OCR-neutral page model and reading order

**Files:**
- Create: `bench/ocr_cpu_service/markhand_ocr/__init__.py`
- Create: `bench/ocr_cpu_service/markhand_ocr/models.py`
- Create: `bench/ocr_cpu_service/markhand_ocr/ordering.py`
- Create: `bench/ocr_cpu_service/markhand_ocr/markdown.py`
- Create: `bench/ocr_cpu_service/tests/test_ordering.py`
- Create: `bench/ocr_cpu_service/tests/test_markdown.py`

**Interfaces:**
- Produces: `OcrSpan(text, confidence, polygon)`
- Produces: `order_spans(spans: Sequence[OcrSpan], page_width: int) -> list[OcrSpan]`
- Produces: `spans_to_markdown(pages: Sequence[PageResult]) -> str`

- [ ] **Step 1: Write failing reading-order tests**

```python
def test_orders_two_columns_top_to_bottom_then_left_to_right():
    spans = [
        span("R2", 600, 200), span("L2", 100, 200),
        span("R1", 600, 100), span("L1", 100, 100),
    ]
    assert texts(order_spans(spans, page_width=1000)) == ["L1", "L2", "R1", "R2"]


def test_full_width_heading_precedes_columns():
    spans = [wide_span("TITLE", 20), span("L", 100, 100), span("R", 600, 100)]
    assert texts(order_spans(spans, page_width=1000)) == ["TITLE", "L", "R"]
```

- [ ] **Step 2: Verify RED**

Run: `python3 -m pytest bench/ocr_cpu_service/tests/test_ordering.py -q`  
Expected: import failure because `markhand_ocr.ordering` does not exist.

- [ ] **Step 3: Implement deterministic ordering**

Normalize polygons to bounding boxes, group spans into y-overlapping lines, identify
full-width blocks, split remaining blocks at a stable vertical gutter, and sort each
column top-to-bottom. Never use recognized text content to determine order.

- [ ] **Step 4: Write failing Markdown/NFC tests**

```python
def test_markdown_normalizes_nfc_and_preserves_page_boundary():
    pages = [page(["Co\u0302ng ho\u0300a"]), page(["Trang hai"])]
    assert spans_to_markdown(pages) == (
        "<!-- Trang 1 (PP-OCRv6) -->\n\nCộng hòa\n\n"
        "<!-- Trang 2 (PP-OCRv6) -->\n\nTrang hai\n"
    )
```

- [ ] **Step 5: Implement Markdown serialization and run tests**

Run: `python3 -m pytest bench/ocr_cpu_service/tests/test_ordering.py bench/ocr_cpu_service/tests/test_markdown.py -q`  
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add bench/ocr_cpu_service/markhand_ocr bench/ocr_cpu_service/tests
git commit -m "feat(ocr): add deterministic page ordering"
```

### Task 3: Bounded conversion service with injected OCR backend

**Files:**
- Create: `bench/ocr_cpu_service/pyproject.toml`
- Create: `bench/ocr_cpu_service/markhand_ocr/backend.py`
- Create: `bench/ocr_cpu_service/markhand_ocr/render.py`
- Create: `bench/ocr_cpu_service/markhand_ocr/service.py`
- Create: `bench/ocr_cpu_service/markhand_ocr/api.py`
- Create: `bench/ocr_cpu_service/tests/test_service.py`
- Create: `bench/ocr_cpu_service/tests/test_api.py`

**Interfaces:**
- Consumes: `OcrSpan`, `order_spans`, and `spans_to_markdown`
- Produces: protocol `OcrBackend.recognize(image: PIL.Image.Image) -> list[OcrSpan]`
- Produces: `convert_pdf(data: bytes, request: ConvertRequest, backend: OcrBackend) -> ConvertResult`
- Produces: FastAPI app with `GET /healthz` and `POST /v1/convert`

- [ ] **Step 1: Write failing service-bound tests with a fake backend**

```python
def test_rejects_page_count_over_limit(tiny_pdf, fake_backend):
    with pytest.raises(ConversionRejected, match="page limit"):
        convert_pdf(tiny_pdf, ConvertRequest(max_pages=0), fake_backend)


def test_converts_selected_pages_and_reports_diagnostics(two_page_pdf, fake_backend):
    result = convert_pdf(two_page_pdf, ConvertRequest(pages=[2]), fake_backend)
    assert [page.page_number for page in result.pages] == [2]
    assert result.backend == "fake"
```

- [ ] **Step 2: Verify RED**

Run: `python3 -m pytest bench/ocr_cpu_service/tests/test_service.py -q`  
Expected: import failure because the service module does not exist.

- [ ] **Step 3: Implement rendering and conversion**

Use PyMuPDF only for local bytes, reject encrypted/invalid PDFs, render at bounded DPI,
enforce maximum pages and pixel dimensions before allocating images, record monotonic
duration and process peak RSS, and always close the document.

- [ ] **Step 4: Write failing API tests**

```python
def test_health_reports_ready_without_model_identity(client):
    response = client.get("/healthz")
    assert response.status_code == 200
    assert response.json() == {"status": "ready", "backend": "fake"}


def test_convert_rejects_non_pdf(client):
    response = client.post(
        "/v1/convert",
        files={"file": ("x.txt", b"not pdf", "text/plain")},
    )
    assert response.status_code == 415
```

- [ ] **Step 5: Implement API and run fast tests**

Map malformed input to 400, unsupported media to 415, bound violations to 413/422, and
unexpected backend failures to a generic 502 response without document text.

Run: `python3 -m pytest bench/ocr_cpu_service/tests/test_service.py bench/ocr_cpu_service/tests/test_api.py -q`  
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add bench/ocr_cpu_service/pyproject.toml bench/ocr_cpu_service/markhand_ocr bench/ocr_cpu_service/tests
git commit -m "feat(ocr): add bounded CPU conversion service"
```

### Task 4: PP-OCRv6 adapter and live smoke

**Files:**
- Create: `bench/ocr_cpu_service/markhand_ocr/paddle_backend.py`
- Create: `bench/ocr_cpu_service/tests/test_paddle_backend.py`
- Create: `bench/ocr_cpu_service/scripts/run_service.sh`
- Create: `bench/ocr_cpu_service/requirements.lock`
- Create: `bench/ocr_cpu_service/README.md`

**Interfaces:**
- Consumes: `OcrBackend` and `OcrSpan`
- Produces: `PaddleOcrBackend`, initialized once with CPU device and PP-OCRv6 defaults
- Produces: `MARKHAND_OCR_BACKEND=paddle` runtime selection

- [ ] **Step 1: Write failing result-adaptation tests**

```python
def test_adapts_paddle_result_without_numpy_values():
    spans = adapt_result(
        {"dt_polys": [[[1, 2], [5, 2], [5, 8], [1, 8]]],
         "rec_texts": ["Cộng hòa"], "rec_scores": [0.98]}
    )
    assert spans[0].text == "Cộng hòa"
    assert spans[0].confidence == pytest.approx(0.98)
    assert spans[0].polygon == ((1, 2), (5, 2), (5, 8), (1, 8))
```

- [ ] **Step 2: Verify RED**

Run: `python3 -m pytest bench/ocr_cpu_service/tests/test_paddle_backend.py -q`  
Expected: import failure because `paddle_backend` does not exist.

- [ ] **Step 3: Implement the public PaddleOCR adapter**

Initialize `PaddleOCR(device="cpu", use_doc_orientation_classify=False,
use_doc_unwarping=False, use_textline_orientation=False)` once, call `predict()` with a
NumPy page image, and adapt only documented `dt_polys`, `rec_texts`, and `rec_scores`.
Reject mismatched result lengths instead of truncating silently.

- [ ] **Step 4: Install latest compatible dependencies and lock the resolved environment**

Create a dedicated virtual environment. Upgrade pip, install the package with test/model
extras through pip, then record exact resolved versions in `requirements.lock`. Review
the lock for GPU-only packages and licenses before commit.

- [ ] **Step 5: Run unit and live smoke tests**

Run: `python3 -m pytest bench/ocr_cpu_service/tests -q`  
Expected: fast suite passes.

Run with cached models:
`MARKHAND_OCR_LIVE=1 python3 -m pytest bench/ocr_cpu_service/tests/test_paddle_backend.py -m live -q`  
Expected: one generated Vietnamese page returns non-empty NFC text on CPU.

- [ ] **Step 6: Commit**

```bash
git add bench/ocr_cpu_service
git commit -m "feat(ocr): add PP-OCRv6 CPU adapter"
```

### Task 5: Comparative Phase A benchmark and gate

**Files:**
- Create: `bench/ocr_cpu_service/benchmark/__init__.py`
- Create: `bench/ocr_cpu_service/benchmark/metrics.py`
- Create: `bench/ocr_cpu_service/benchmark/run.py`
- Create: `bench/ocr_cpu_service/benchmark/report.py`
- Create: `bench/ocr_cpu_service/tests/test_metrics.py`
- Create: `bench/ocr_cpu_service/tests/test_report.py`
- Create: `bench/ocr_cpu_service/reports/phase-a.md`
- Create: `bench/ocr_cpu_service/reports/phase-a.json`

**Interfaces:**
- Produces: `normalize_for_metric(text: str) -> str`
- Produces: `cer(reference: str, hypothesis: str) -> float`
- Produces: candidate adapters for Markhand default, `tessdata_best`, and PP-OCRv6
- Produces: machine-readable raw summary and generated Markdown report

- [ ] **Step 1: Write failing metric tests**

```python
def test_cer_normalizes_nfc_and_whitespace():
    assert cer("Cộng  hòa", "Co\u0323\u0302ng hòa") == 0.0


def test_empty_reference_policy_is_explicit():
    assert cer("", "") == 0.0
    assert cer("", "x") == 1.0
```

- [ ] **Step 2: Verify RED, implement metrics, and verify GREEN**

Run: `python3 -m pytest bench/ocr_cpu_service/tests/test_metrics.py -q`  
Expected before implementation: import failure.  
Expected after implementation: all metric tests pass.

- [ ] **Step 3: Write report gate tests**

```python
def test_gate_uses_better_tesseract_baseline():
    result = evaluate_gate(default_cer=.20, best_cer=.10, paddle_cer=.07)
    assert result.relative_improvement == pytest.approx(.30)
    assert result.passed


def test_gate_fails_at_less_than_twenty_percent():
    assert not evaluate_gate(.20, .10, .081).passed
```

- [ ] **Step 4: Implement candidates and deterministic report**

Use exact page ranges and source checksums from the corpus manifest. Capture command
versions, elapsed time, peak RSS, errors, and per-page CER/WER. The Markdown file must be
rendered entirely from `phase-a.json` and contain no complete document text.

- [ ] **Step 5: Run the measured benchmark**

Download validated corpus/model assets into ignored paths, build the current release
`fileconv`, and run all three candidates serially on the same 8-vCPU host. Sample bounded
pages from `89/2026/TT-BTC`; report its actual scan/native/mixed classification.

Run: `PYTHONPATH=bench/ocr_cpu_service python3 -m benchmark.run --output bench/ocr_cpu_service/reports/phase-a.json`  
Run: `PYTHONPATH=bench/ocr_cpu_service python3 -m benchmark.report bench/ocr_cpu_service/reports/phase-a.json`  
Expected: report states PASS or STOP from measured values, never from a hard-coded claim.

- [ ] **Step 6: Commit evidence**

```bash
git add bench/ocr_cpu_service/benchmark bench/ocr_cpu_service/tests bench/ocr_cpu_service/reports
git commit -m "bench(ocr): compare Vietnamese CPU OCR quality"
```

### Task 6: Conditional Phase B structure parsing

**Files:**
- Create only on Phase A PASS:
  - `bench/ocr_cpu_service/markhand_ocr/layout.py`
  - `bench/ocr_cpu_service/markhand_ocr/regions.py`
  - `bench/ocr_cpu_service/markhand_ocr/structure_markdown.py`
  - `bench/ocr_cpu_service/tests/test_layout.py`
  - `bench/ocr_cpu_service/tests/test_structure_markdown.py`
  - `bench/ocr_cpu_service/reports/phase-b.md`
  - `bench/ocr_cpu_service/reports/phase-b.json`

**Interfaces:**
- Consumes: Phase A renderer, OCR backend, page diagnostics, and corpus
- Produces: `LayoutRegion(kind, polygon, confidence)`
- Produces: region router for text/table/formula/figure
- Produces: structure-aware Markdown and independent Phase B gate

- [ ] **Step 1: Read the Phase A gate**

If `phase-a.json` says STOP, do not create Phase B production code. Record Phase B as
not run and cite the measured failed criterion.

- [ ] **Step 2: On PASS, select license-reviewed model weights**

Record upstream source, model license, runtime, checksum, and resource footprint in a
dependency evidence document. Do not rely on a MinerU-packaged copy.

- [ ] **Step 3: Write failing region-order and serializer tests**

```python
def test_routes_table_without_flattening_it_into_text():
    regions = [region("text", "Title"), region("table", [["A", "B"], ["1", "2"]])]
    markdown = structure_to_markdown(regions)
    assert "<table>" in markdown
    assert "<td>2</td>" in markdown


def test_formula_region_serializes_as_latex():
    assert structure_to_markdown([formula("x^2")]) == "$$x^2$$\n"
```

- [ ] **Step 4: Implement one replaceable stage at a time**

Add layout first and measure reading order. Add table routing only after layout tests
pass. Add formula routing only after table tests pass. Keep each model behind a protocol
so tests use deterministic adapters.

- [ ] **Step 5: Run Phase B benchmark**

Measure text CER, reviewed reading-order violations, table cell/edit accuracy, formula
exact/normalized match, seconds/page, and peak RSS. Do not claim Phase B success from
text CER alone.

- [ ] **Step 6: Commit conditional evidence**

```bash
git add bench/ocr_cpu_service/markhand_ocr bench/ocr_cpu_service/tests bench/ocr_cpu_service/reports
git commit -m "bench(ocr): evaluate structured PDF parsing"
```

### Task 7: Final verification and handoff

**Files:**
- Modify: `bench/ocr_cpu_service/README.md`
- Modify: `.github/PULL_REQUEST_TEMPLATE.md` only if a reusable evidence field is proven necessary

**Interfaces:**
- Consumes: all implementation and benchmark evidence
- Produces: reproducible commands, known limits, and explicit production-adoption decision

- [ ] **Step 1: Run focused verification**

```bash
python3 -m pytest bench/ocr_cpu_service/tests -q
python3 scripts/check-architecture-boundaries.py
python3 scripts/check-architecture-boundaries.py --self-test
cargo fmt --all -- --check
cargo metadata --locked --format-version 1 --no-deps
python3 scripts/check-dependency-policy.py
git diff --check
```

- [ ] **Step 2: Validate clean repository contents**

Confirm no model binary, downloaded PDF, corpus image, complete OCR output, virtual
environment, cache, or secret-bearing log is tracked.

- [ ] **Step 3: Update README and report limitations**

Document exact setup/run commands, CPU/RAM host, corpus strata, measured gate outcome,
Thông tư 89 classification and sampled pages, and whether Phase B ran.

- [ ] **Step 4: Commit and push**

```bash
git add bench/ocr_cpu_service .gitignore docs/superpowers
git commit -m "docs(ocr): record CPU OCR spike results"
git push -u origin cursor/mineru-ocr-spike-e533
```
