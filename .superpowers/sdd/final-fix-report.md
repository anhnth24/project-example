# Final-review remediation report

Date: 2026-08-07  
Branch: `cursor/mineru-ocr-spike-e533`  
Reviewed decision: Phase A **STOP**; Phase B not run; no production adoption.

## Delivered findings

- Runtime/model loading is cache-only. Detection and recognition directories
  are mandatory; each is resolved locally and checked for non-empty
  `inference.json`, `inference.yml`, and `inference.pdiparams` before
  `PaddleOCR` construction. Runtime startup disables PaddleX remote model-source
  probes, and the launcher uses one Uvicorn worker.
- ASGI middleware acquires the single process-wide request slot before reading
  multipart data. One waiter is allowed, excess waiters fail immediately, and
  acquisition has a finite timeout. The request deadline covers body receipt,
  parsing, rendering, inference, and response construction.
- A timed-out Python inference is not described as cancelled. It returns `504`,
  retains capacity, and releases the slot only after underlying work exits and
  request-scoped PDF/page/image cleanup completes.
- The pinned 1907 Wikimedia multi-column scan now has a five-item,
  human-reviewed short-heading sequence. The report computes observed anchors,
  comparable pairs, inversions, and missing anchors and labels this evidence
  qualitative and limited.
- PyMuPDF is absent from runtime and lock dependencies. Rendering and PDF
  inspection use `pypdfium2`/PDFium, with dimensions and pixel count checked
  before bitmap allocation.
- `psutil` is direct, Tesseract subprocess timing is distinguished from warm
  in-process Paddle timing, corpus downloads pin validated public DNS answers
  to TLS connections and enforce connection/whole-download deadlines, and
  `pylock.toml` pins wheel URLs and SHA-256 hashes.

## Strict TDD evidence

1. Pre-body admission, bounded queue, deadline, cleanup:
   - Test commit: `739385b`.
   - RED: `python3 -m pytest bench/ocr_cpu_service/tests/test_api.py -q`
     exited 1 with `3 failed, 14 passed`: missing `acquire`, missing bounded
     waiter support, and slow body returned 400 instead of 504.
   - Implementation commit: `0159d73`.
   - GREEN: same command exited 0 with `17 passed`.
2. Backend-level cache-only construction:
   - Test commit: `56d11aa`.
   - RED:
     `python3 -m pytest bench/ocr_cpu_service/tests/test_paddle_backend.py -q -m 'not live'`
     exited 1 with `3 failed, 13 passed, 2 deselected`; no-cache and incomplete
     cache paths reached pipeline construction.
   - Implementation commit: `1abf36c`.
   - GREEN: same command exited 0 with `16 passed, 2 deselected`.
3. Historical reviewed ordering evidence:
   - Test commit: `a1df5ca`.
   - RED:
     `python3 -m pytest bench/ocr_cpu_service/tests/test_metrics.py bench/ocr_cpu_service/tests/test_render.py bench/ocr_cpu_service/tests/test_report.py -q`
     failed collection because the historical anchor contract did not exist.
   - Implementation commit: `bc7194a`.
   - GREEN: same command exited 0 with `24 passed`.
4. Ordering-report page filtering:
   - Test commit: `11f0d5e`.
   - RED:
     `python3 -m pytest bench/ocr_cpu_service/tests/test_report.py::test_markdown_is_deterministic_metadata_only_rendering -q`
     exited 1 because unreviewed pages appeared as empty ordering rows.
   - Implementation/evidence commit: `b56ee9a`.
   - GREEN: same command exited 0 with `1 passed`.
5. Runtime model-source network probes:
   - Test commit: `6544b92`.
   - RED:
     `python3 -m pytest bench/ocr_cpu_service/tests/test_paddle_backend.py::test_runtime_selection_preserves_backend_injection_and_safe_health -q`
     exited 1 because the source-check disable flag was absent.
   - Implementation commit: `50e7b8b`.
   - GREEN: same command exited 0 with `1 passed`.

## Benchmark rerun

The complete benchmark was rerun because reviewed historical ordering became a
new evaluated input. Command:

```text
PYTHONPATH=bench/ocr_cpu_service bench/ocr_cpu_service/.venv/bin/python -m benchmark.run --output bench/ocr_cpu_service/reports/phase-a.json --manifest bench/ocr_cpu_service/corpus/sources.json --corpus-dir bench/ocr_cpu_service/.data/corpus --work-dir bench/ocr_cpu_service/.data/benchmark --fileconv target/release/fileconv --system-tessdata /usr/share/tesseract-ocr/5/tessdata --best-tessdata tessdata_best --paddle-detection-dir bench/ocr_cpu_service/.data/models/detection --paddle-recognition-dir bench/ocr_cpu_service/.data/models/recognition --cpu-threads 8 --max-rss-bytes 4294967296
PYTHONPATH=bench/ocr_cpu_service bench/ocr_cpu_service/.venv/bin/python -m benchmark.report bench/ocr_cpu_service/reports/phase-a.json
```

Result: exit 0; all 63 candidate/page runs completed. Phase A remains **STOP**:
better Tesseract real-scan CER `0.1187078874`, PP-OCRv6 real-scan CER
`0.4335447219`, relative improvement `-265.22%`, and two RSS-limit
violations. On historical page 4, both Tesseract configurations observed 5/5
anchors with 0/10 inversions; PP-OCRv6 observed 3/5 with 0/3 inversions and two
missing anchors. Reports were regenerated in `b56ee9a`.

## Fresh verification

- `python3 -m pytest bench/ocr_cpu_service/tests -q`:
  `99 passed, 2 skipped`, exit 0.
- Real cache-only startup:
  `MARKHAND_OCR_LIVE=1 .../.venv/bin/python -m pytest .../test_paddle_backend.py -m live -q`:
  `2 passed, 16 deselected`, exit 0; one benign missing-ccache warning.
- Generated report comparison and `bash -n scripts/run_service.sh`: exit 0.
- `cargo fmt --all -- --check`: exit 0.
- `cargo metadata --locked --format-version 1 --no-deps`: exit 0.
- `python3 scripts/check-dependency-policy.py`: exit 0.
- Architecture check plus self-test: exit 0, five self-tests passed.
- `pip install --dry-run -r pylock.toml`: exit 0; pip warned only that pylock
  requirement-source support is experimental.
- `make check-web`: exit 0; 52 files/556 tests passed, build passed; eight
  pre-existing lint warnings and the existing large-chunk warning remain.
- Tracked artifact audit: zero OCR `.data`, virtualenv, cache, model binary,
  corpus PDF/image, or OCR text-output artifacts; one tracked hash lock.
- Markdown report regeneration exactly matches tracked `phase-a.md`.
- `git diff --check`: exit 0.

## Repository-wide gate failures outside this diff

- `make check-foundation` exited 2 in `check-rust`: Clippy rejects an
  uninlined `format!` argument in
  `crates/server/tests/common/fts_visibility_diagnostic.rs`. That file has no
  branch diff; its last commit is `64b5f5f`.
- Independent `make check-knowledge-extraction` exited 2 while rebuilding
  `whisper-rs-sys`: the environment linker cannot find `-lstdc++`. This is a
  host toolchain failure outside the Python OCR changes.

## Residual concerns

- The Phase A evidence remains a small bounded sample; STOP and non-adoption
  are unchanged.
- Historical ordering is deliberately qualitative: one page and five short
  headings, with fuzzy matching bounded to 25% character edits.
- Python cannot hard-cancel in-process Paddle inference; timeout containment
  relies on retaining the sole slot until real completion.
- Any binary redistribution of pypdfium2/PDFium still requires shipping and
  reviewing the wheel's `BUILD_LICENSES` notices.
