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

The corrected calibration is an exact 2×2 matrix on one fixed
`tessdata_best` installation:

1. `legacy-best-vie-eng`: current preprocessing, `vie+eng`.
2. `preserve-best-vie-eng`: conditional near-bitonal bypass, `vie+eng`.
3. `legacy-best-vie`: current preprocessing, `vie`.
4. `preserve-best-vie`: conditional near-bitonal bypass, `vie`.

This separates preprocessing effects within each language setting and language
effects within each preprocessing setting. The former `baseline-system-fast`
measurement remains historical context only; it is not an arm or baseline in
the corrected calibration. The spike does not combine the result with
contextual substitutions, heading rewriting, table segmentation, or
generative correction.

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

The original 98.5% extreme-pixel threshold was invalid for this hypothesis:
none of the 22 approved 300-DPI renders activated it. A bounded histogram
measurement using the frozen dark/light cutoffs found extreme ratios near
92.2%–96.8%. The corrected threshold is 92.0%. Exact post-freeze Rust
diagnostics measured 92.214812969%–96.779655670%, activating all 22 pages with
a 0.214812969-percentage-point margin at the lowest page while retaining the
existing blank, gradient/photo-like, ink
minimum, and ink maximum rejection gates. It is frozen by tests before the
corrected live comparison and cannot be tuned from OCR/reference output.

A benchmark-only Rust diagnostic emits dimensions, total/extreme/dark pixel
counts, exact classifier constants, qualification, and preserve activation.
The runner executes that diagnostic against each exact saved render. Raw
evidence binds every diagnostic to the render SHA-256, release binary SHA-256,
classifier descriptor hash, and canonical configuration hash. Reported
activation therefore comes from the exact Rust classifier used by OCR, not a
Python reimplementation. Each preserve-mode arm must have nonzero activation
to be eligible.

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
- Page 60: word-boundary counts and unaccented ratios for frozen,
  non-ambiguous accented/unaccented Vietnamese pairs. Tokens whose unaccented
  form is itself valid or contextually ambiguous are excluded.
- Page 450: total digit-character count and a checksum of the concatenated
  digit stream, plus legal-document identifiers, non-whitespace characters,
  and suspicious symbols. Splitting one digit stream into more OCR fragments
  does not increase coverage.
- Every rendered page: exact Rust classifier counts and activation evidence.
- Every OCR execution: latency, sampled process-tree RSS, exit state, output
  bound, and successful candidate-page record coverage.

The reference disagreement is a comparative acceptance metric only. It must
never be reported as CER or ground-truth accuracy.

## Winner gate

A preserve variant is eligible for later independent review only if all are
true:

- lower aggregate character disagreement than the legacy arm with the same
  language setting on pages 1–20;
- no page 1–20 regresses by more than two absolute disagreement points;
- every page 60 unaccented-pair ratio improves or remains equal;
- page 450 digit, identifier, and non-whitespace coverage do not regress;
- all 22 pages succeed with one marker each;
- exact Rust activation evidence is nonzero and bound to the measured renders;
- median latency is at most 2× the matched legacy arm;
- sampled process-tree peak RSS remains strictly below the 768 MiB worker
  memory limit for both per-record and aggregate checks.

If multiple preserve variants pass, choose the lowest character disagreement,
then the lowest word disagreement, then the lowest median latency. Equal page
60 ratios are acceptable as approved before measurement; the prior
post-measurement strict-decrease requirement was an invalid gate change and is
removed. If none passes, stop without a full rerun or production change.

## Full comparison

The 839-page run is not executed as part of the corrected calibration. A
winner only makes Task 4 eligible for independent review; Task 4 remains
blocked until that review explicitly permits it. If later permitted, the
output and timing log remain ignored artifacts. The final tracked report
records:

- exact binary, source, tessdata, configuration, classifier, and tool hashes;
- complete page-marker coverage;
- wall time and peak RSS;
- before/after proxy metrics;
- pages with the largest reference disagreement changes;
- the explicit limitation that only pages 1–20 have a private,
  non-human-verified acceptance reference and pages 60/450 have no verified
  transcription.

Both Markdown files are provided to the user outside Git for manual review.

## Failure and safety behavior

- Candidate execution uses argv arrays, never a shell.
- Time, output, image, tempfile, process-tree, and memory measurements remain
  bounded.
- A failed page or missing marker fails the candidate; partial output cannot
  win.
- No recognized text, private reference text, private-reference hash/path,
  downloaded PDF, rendered page, or full Markdown output is committed. The
  private-reference SHA-256 remains only in ignored raw provenance and runtime
  validation.
- No contextual lexicon or language model changes canonical legal text.
- No web-worker timeout, queue, sandbox, API, or deployment setting changes.

## Verification

- TDD covers classifier boundaries, unchanged non-bitonal behavior, preserved
  bitonal dimensions, deterministic output, and image bounds.
- A fixture proves the comparison labels reference disagreement rather than
  CER.
- Raw additive counts and exact Rust activation evidence regenerate the report.
- Access evidence is derived from pages actually opened/rendered and from
  diagnostic/OCR execution counters; it is not inserted as asserted literals.
- Rust formatting, locked metadata, dependency policy, architecture checks,
  full relevant tests, and tracked-artifact audits pass before push.
- OCR/native-runtime behavior requires independent review before any later
  production adoption.
