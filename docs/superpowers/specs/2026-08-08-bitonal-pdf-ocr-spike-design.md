# Bitonal PDF OCR Improvement Spike Design

Created: 2026-08-08  
Scope: CPU-only OCR comparison on Thông tư 89/2026/TT-BTC  
Decision boundary: benchmark first; no web deployment in this spike

## Objective

Test whether Markhand currently damages clean bitonal PDF scans by rendering at
300 DPI and then reducing the rendered image to a 2400-pixel long edge before
Tesseract. Produce directly comparable before/after Markdown for the 839-page
Thông tư without lexicon replacement, LLM correction, or table-cell OCR.

## Verified starting facts

- The source contains 839 image-bearing pages and PDFium exposes zero text-layer
  characters on sampled pages 1, 60, 450, and 839.
- Embedded page images are approximately 1636×2348 pixels, about 200 DPI.
- PDFium renders the same pages at approximately 2450×3520 pixels at 300 DPI.
- Current image preprocessing converts to grayscale, reduces any long edge over
  2400 pixels, applies unsharp masking, and stretches the histogram.
- Therefore the current PDF path discards most of the intended 300-DPI scale
  before invoking Tesseract.
- The current worker-compatible full run completed in 21m28s with 839/839 page
  markers, 265 MiB peak RSS, and a 1.8 MiB Markdown result.
- The user-provided 20-page Qwen Markdown is a private acceptance reference, not
  human-verified ground truth and never a repository artifact.

## Approaches

The spike compares four isolated variants:

1. `baseline-system-fast`: current preprocessing, system `vie+eng`.
2. `baseline-best`: current preprocessing, `tessdata_best` with `vie+eng`.
3. `bitonal-best-vie-eng`: conditional bitonal bypass, best `vie+eng`.
4. `bitonal-best-vie`: conditional bitonal bypass, best `vie`.

This isolates model, preprocessing, and language effects. It does not combine
the result with contextual substitutions, heading rewriting, table
segmentation, or generative correction.

## Conditional preprocessing

The Rust preprocessing boundary gains a deterministic “effectively bitonal”
classification based only on image pixels and dimensions. The classifier must:

- use bounded sampling or a bounded full histogram;
- require a high proportion of pixels near black or near white;
- reject photographs, grayscale scans, low-contrast pages, and empty pages;
- be deterministic and independent of ground truth or OCR output.

For a qualifying image whose long edge exceeds 2400 pixels:

- convert to grayscale;
- preserve its original dimensions;
- skip resize, unsharp, and histogram normalization.

All other images retain the existing preprocessing exactly. Existing decode,
dimension, allocation, tempfile, command, and output bounds remain unchanged.

The threshold is frozen by tests before the live comparison. It cannot be
tuned from the private reference output.

## Comparison flow

### Calibration set

Run every variant serially on pages:

- 1–20: comparison with the user-provided reference;
- 60: prose/diacritic diagnostic from the supplied improvement note;
- 450: table/form content-preservation diagnostic.

All variants receive identical PDFium 300-DPI renders. Page 60 and 450 have no
verified transcription, so their diagnostics cannot be called CER.

### Metrics

- Pages 1–20: normalized character and word disagreement against the private
  reference after removing Markdown decoration and standalone page numbers.
- Pages 60 and 450: deterministic counts for preserved digits, legal-document
  identifiers, known accent-error proxies, non-whitespace characters, and
  suspicious symbols.
- Every page: latency, sampled process-tree RSS, exit state, output bound, and
  page-marker coverage.

The reference disagreement is a comparative acceptance metric only. It must
never be reported as CER or ground-truth accuracy.

## Winner gate

A variant is eligible for the full run only if all are true:

- lower aggregate character disagreement than the current baseline on pages
  1–20;
- no page 1–20 regresses by more than two absolute disagreement points;
- page 60 accent-error proxies improve or remain equal;
- page 450 digit, identifier, and non-whitespace coverage do not regress;
- all 22 pages succeed with one marker each;
- median latency is at most 2× baseline;
- sampled RSS remains below the 768 MiB worker memory limit.

If multiple variants pass, choose the lowest character disagreement, then the
lowest word disagreement, then the lowest median latency. If none passes, stop
without a full rerun or production change.

## Full comparison

Only the winning variant runs all 839 pages. The output and timing log remain
ignored artifacts. The final tracked report records:

- exact binary, source, tessdata, configuration, and tool hashes;
- complete page-marker coverage;
- wall time and peak RSS;
- before/after proxy metrics;
- pages with the largest reference disagreement changes;
- the explicit limitation that only pages 1–20 have a reference.

Both Markdown files are provided to the user outside Git for manual review.

## Failure and safety behavior

- Candidate execution uses argv arrays, never a shell.
- Time, output, image, tempfile, process-tree, and memory measurements remain
  bounded.
- A failed page or missing marker fails the candidate; partial output cannot
  win.
- No recognized text, private reference text, downloaded PDF, rendered page,
  or full Markdown output is committed.
- No contextual lexicon or language model changes canonical legal text.
- No web-worker timeout, queue, sandbox, API, or deployment setting changes.

## Verification

- TDD covers classifier boundaries, unchanged non-bitonal behavior, preserved
  bitonal dimensions, deterministic output, and image bounds.
- A fixture proves the comparison labels reference disagreement rather than
  CER.
- Raw additive counts regenerate the report.
- Rust formatting, locked metadata, dependency policy, architecture checks,
  full relevant tests, and tracked-artifact audits pass before push.
- OCR/native-runtime behavior requires independent review before any later
  production adoption.
