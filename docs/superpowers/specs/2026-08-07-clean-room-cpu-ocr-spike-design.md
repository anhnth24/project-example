# Clean-room CPU OCR Service Spike Design

Created: 2026-08-07  
Decision: independently implement the pipeline; open model weights are allowed  
Scope: benchmark-only service, Phase A before Phase B

## Objective

Determine whether a separately deployed CPU OCR service can materially improve
Vietnamese scanned-PDF conversion quality over Markhand's current Tesseract path
without copying or importing MinerU source code.

Phase A validates text recognition and basic reading order with PP-OCRv6. Phase B adds
learned layout, table, and formula stages only if Phase A passes its adoption gate.
This spike does not change the production web upload path.

## Clean-room and dependency constraints

- MinerU documentation, published papers, observable output, and high-level architecture
  may inform the design. MinerU source code is not copied, translated, vendored, or
  imported.
- OCR is called through PaddleOCR's supported public API. Markhand owns request
  validation, PDF rendering, page orchestration, reading-order reconstruction,
  Markdown serialization, diagnostics, benchmarking, and service contracts.
- Model binaries and benchmark documents are downloaded at setup time and remain
  ignored. The repository stores only source metadata, licenses, checksums, download
  tooling, and reproducible reports.
- The service remains outside `fileconv-core` and outside the production server
  dependency graph. Any later production integration requires a separate ADR,
  dependency/security review, and measured acceptance evidence.

## Considered approaches

1. **Independent Python CPU service using PaddleOCR public APIs — selected.**
   This is the shortest path to testing PP-OCRv6 on the current 8-vCPU/48-GiB Cloud
   Agent while keeping Python/model dependencies out of Rust production artifacts.
2. **Direct ONNX integration in Rust.**
   This would reduce runtime diversity but requires recreating model preprocessing and
   postprocessing before quality is known. It is deferred until the model has passed a
   representative benchmark.
3. **Run MinerU as the service.**
   This would provide a useful external reference but would not satisfy the independent
   implementation decision and would introduce MinerU's service/license surface.

## Phase A architecture

The benchmark service lives under `bench/ocr_cpu_service/` and exposes:

- `GET /healthz`: process readiness and loaded OCR backend metadata;
- `POST /v1/convert`: multipart PDF input plus bounded page selection;
- response JSON containing Markdown, elapsed time, peak RSS, and page-level spans with
  confidence, bounding boxes, text, and diagnostics.

Data flow:

```text
bounded PDF upload
  -> PDFium/PyMuPDF page render at configured DPI
  -> PP-OCRv6 detection and recognition on CPU
  -> deterministic line grouping and column-aware reading order
  -> NFC normalization
  -> Markdown plus page-level diagnostic JSON
```

The process loads models once. Requests execute with bounded page count, input bytes,
render dimensions, timeout, and concurrency. Temporary inputs are request-scoped and
removed on completion or failure. The service does not fetch URLs supplied by callers.

The initial Markdown serializer preserves page boundaries and paragraphs. It does not
claim heading, table, or formula reconstruction; those are Phase B responsibilities.

## Benchmark corpus

The reproducible corpus has three layers:

1. `nrl-ai/vn-ocr-documents-eval` real and synthetic-scan configurations. It provides
   public/CC0 documents and human-verified page text, making normalized CER measurable.
2. Public-domain Vietnamese scans from Wikimedia Commons/Wikisource, selected across
   clean books, degraded historical print, and multi-column newspaper layouts. These
   provide realistic qualitative reading-order cases; only pages with reviewed
   transcriptions are used for quantitative claims.
3. Deterministically generated Vietnamese scan PDFs with exact source text and controlled
   blur, skew, contrast, stamp, and compression artifacts.

The downloader uses an allowlisted source manifest with canonical URL, license, expected
size ceiling, and SHA-256. Corpus and model files are never committed.

Thông tư `89/2026/TT-BTC` is verified against the official Government document record
(`docid=218974`). If an official downloadable PDF is available, the downloader records
its canonical attachment URL and checksum. Because the document is approximately
839 pages, the benchmark selects a deterministic bounded page sample containing body
text and forms rather than processing the entire document in the adoption gate.
If inspection shows it is born-digital rather than scanned, it is retained as a
mixed/native regression case and is not reported as scan-OCR evidence.

## Measurement and adoption gate

Every candidate runs on the same host and page images:

- current Markhand default (`vie+eng`);
- Markhand with `tessdata_best`;
- the independent PP-OCRv6 service.

The report records:

- whitespace-normalized NFC CER and WER per page and corpus stratum;
- median and p95 seconds/page;
- peak RSS and failure/timeout count;
- reading-order violations on reviewed multi-column pages;
- model/runtime versions, CPU count, source/model checksums, and commit SHA.

Phase A passes only when:

- PP-OCRv6 reduces aggregate real-scan CER by at least 20% relative to the better
  Tesseract baseline;
- no benchmark stratum regresses by more than 5 absolute CER points without a documented
  reason;
- all bounded requests complete without crash or resource-limit violation; and
- the report clearly separates synthetic, real-scan, and native/mixed PDF evidence.

Failure to meet this gate stops the spike after Phase A. Results are still documented.

## Phase B design

Phase B is conditional. It introduces replaceable stages for layout regions, tables, and
formulas while preserving Phase A OCR and diagnostics:

```text
page render
  -> layout regions
  -> region router
       text    -> PP-OCRv6
       table   -> table structure model
       formula -> formula recognition model
       figure  -> retained asset reference
  -> page reading-order graph
  -> Markdown/HTML/LaTeX serialization
```

Model choice is made from license-reviewed open weights after Phase A. Each stage has a
stable internal result type so model runtimes can change without changing the service
contract. Phase B receives its own quality gates for table structure and reading order;
plain text CER alone cannot qualify it.

## Testing

- Unit tests cover page bounds, upload limits, line grouping, deterministic ordering,
  Markdown escaping, NFC normalization, and diagnostic serialization.
- API tests use generated tiny PDFs and a fake OCR adapter; model downloads are not
  required for the fast suite.
- A live model smoke test is explicit and runs only when the model cache is present.
- The benchmark runner validates corpus checksums, runs all candidates, emits raw JSON,
  and renders a Markdown summary from that JSON.
- Reports must not contain complete source documents, model identifiers prohibited by
  repository policy, secrets, or unbounded OCR output.

## Delivery boundary

This change delivers a reproducible benchmark service and measured report. It does not:

- route production web uploads to the new service;
- add Python or Paddle dependencies to Rust crates or production worker images;
- declare deployment readiness from a benchmark alone; or
- mark Phase B successful without its own measured evidence.
