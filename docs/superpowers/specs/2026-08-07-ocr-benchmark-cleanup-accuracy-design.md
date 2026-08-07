# OCR Benchmark Cleanup and Accuracy-First Design

Created: 2026-08-07  
Decision: retain a generic benchmark harness and corpus; remove the rejected service  
Priority: Vietnamese character accuracy before layout/table work

## Objective

Turn the completed PP-OCRv6 spike into a small, reusable Vietnamese OCR benchmark
without leaving a dead HTTP service, Paddle runtime, or model-specific production
surface in the repository. Preserve the measured negative result and the downloaded
corpus locally, then define the next accuracy-first investigation for Markhand's
existing Tesseract path.

## Cleanup boundary

### Retained

- `bench/ocr_cpu_service/.data/corpus/` remains local and ignored. These files are not
  committed; the checked-in source manifest, licenses, size limits, and SHA-256 values
  remain the durable way to reproduce them after a Cloud Agent is destroyed.
- Corpus validation/downloading and its offline tests.
- Generic PDFium rendering helpers required to produce bounded, identical page images.
- Generic NFC CER/WER, reading-order, timing, RSS, and deterministic report utilities.
- A serial candidate runner for the two current Markhand/Tesseract baselines and future
  local experiment candidates.
- The checked-in Phase A JSON/Markdown report as immutable evidence that generic
  PP-OCRv6 was rejected.
- A minimal Python benchmark/test environment with a hash-locked permissive dependency
  set.

### Removed

- FastAPI routes, multipart handling, request admission, queue/deadline logic, and the
  service launcher.
- PaddleOCR adapter, cache validation, model startup, inference worker, and live tests.
- PaddleOCR, PaddlePaddle, PaddleX, FastAPI, Uvicorn, multipart, and HTTP client
  dependencies.
- Service-only conversion models, Markdown serializer, backend protocol, and tests.
- Model cache tooling and documentation that implies the rejected service should run.

No downloaded PDF or model binary becomes tracked during cleanup.

## Generic benchmark shape

The retained package is renamed conceptually from a “service” to a “Vietnamese OCR
benchmark.” Its stable components are:

```text
corpus/sources.json + corpus/download.py
                  |
                  v
benchmark/corpus.py -> verified BenchmarkPage records
benchmark/render.py -> bounded PDFium page images
benchmark/candidates.py -> serial local candidate process contract
benchmark/metrics.py -> NFC CER/WER and ordering counts
benchmark/run.py -> raw metadata-only JSON
benchmark/report.py -> deterministic Markdown
```

Candidate definitions contain an ID, label, command arguments, allowlisted environment,
and provenance. Candidate output is text plus timing/resource metadata. The runner never
stores recognized document text in checked-in reports.

The old Phase A report remains renderable, but future decisions are data-driven rather
than hard-coded to a Paddle candidate name. A comparison record names the baseline,
challenger, threshold, strata, and result explicitly.

## Accuracy-first investigation

### Corpus before tuning

The current nine real pages are too small for optimization. Before changing production
OCR behavior:

1. expand to at least 50 human-transcribed real Vietnamese scan pages from public,
   license-reviewed government and public-domain sources;
2. cover clean official documents, skew, low contrast, stamps/watermarks, dense forms,
   small text, and older print;
3. freeze a tuning set and an untouched holdout set by source document, not random page,
   so adjacent pages cannot leak between sets;
4. store only short metadata and human ground truth permitted by the source license;
   downloaded PDFs/images remain ignored.

Synthetic scans remain regression fixtures but cannot determine adoption.

### Controlled experiment matrix

Run one variable at a time against the frozen tuning set:

- render at 300 versus 400 DPI;
- no deskew versus bounded automatic deskew;
- grayscale normalization, Otsu thresholding, and adaptive thresholding;
- conservative denoise/watermark suppression;
- Tesseract PSM 3, 4, 6, and 11;
- system `vie+eng` data versus `tessdata_best`;
- conditional retry selected only from runtime-observable confidence/layout signals.

Every experiment records configuration, source checksum, binary checksum, CER/WER,
seconds/page, RSS, and failures. Ground truth is used only by the evaluator, never by
runtime selection.

### Selection and Rust delivery

The experiment winner must be explainable as a bounded rule. It is first reproduced on
the untouched holdout set. Only then is the minimal behavior ported into
`crates/core/src/image_ocr.rs`, with failing Rust tests added before implementation.

No language-model correction, dictionary replacement, or generative rewriting is
allowed: preserving source content is more important than producing fluent text.

## Gates

An OCR change is eligible for production review only if it:

- improves real-scan holdout CER by at least 20% relative to the frozen baseline;
- reaches CER at or below 9.5% when the expanded corpus remains comparable to the
  current baseline;
- regresses no named difficulty stratum by more than 2 absolute CER points;
- keeps median and p95 latency at or below 2x baseline;
- introduces no new crash, timeout, dependency-missing, or unbounded-memory behavior.

If Tesseract optimization misses the quality gate, the next investigation may evaluate
an open Vietnamese-specific line recognizer such as VietOCR/TrOCR behind the same
generic candidate contract. That requires a separate model/license/resource decision;
it is not part of this cleanup.

## Verification

- Fast tests run without network, models, or downloaded corpus.
- The corpus downloader retains HTTPS allowlisting, public-address/TLS binding, byte,
  timeout, and checksum enforcement.
- The archived Phase A report regenerates exactly from its JSON after genericization.
- A generic synthetic candidate test proves reports do not depend on Paddle-specific
  IDs.
- Tracked-file audit rejects PDFs, images, model weights, virtual environments, caches,
  and full OCR output.
- Rust formatting, locked metadata, dependency policy, and architecture boundaries
  remain green because cleanup does not change production Rust behavior.
