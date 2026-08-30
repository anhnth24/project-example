# CPU ruled-table OCR spike design

## Status

Approved in conversation on 2026-08-09. This document defines a
benchmark-only experiment. It does not authorize a production pipeline change.

## Objective

Determine whether a bounded CPU-only pipeline can recover simple ruled tables
from scanned Vietnamese PDFs as cell matrices and serialize them to Markdown
with useful cell-level accuracy.

The first target is the ruled form/table class present in the public Thông tư
89/2026/TT-BTC scan. The spike does not attempt to solve borderless tables,
nested tables, or arbitrary document layout.

## Verified starting point

- The 839-page document is image-only for the relevant PDF path, so
  `pdf-inspector` cannot recover glyph-based table structure.
- Current recovery renders each page with PDFium at 300 DPI and sends the whole
  image to Tesseract. OCR output is wrapped as flat page text.
- The latest full run with legacy preprocessing, `tessdata_best`, and `vie`
  produced all 839 page markers and improved Vietnamese text disagreement on
  the 20 reference pages, but emitted no valid Markdown tables.
- Page 450 demonstrates the failure mode: labels and values are more readable,
  while rows, columns, empty cells, and field associations are flattened.
- Existing Python benchmark dependencies already include Pillow and
  pypdfium2. The spike must not add OpenCV, a learned table model, or another
  heavy runtime.

## Scope

### Included

- Simple tables with visible horizontal and vertical rules.
- At most one detected table region per benchmark page.
- Rectangular grids without merged or nested cells.
- Multiline cell text represented with `<br>` in Markdown.
- Empty-cell detection.
- Cell-level human annotations and structural evaluation.
- A benchmark-only Python implementation over saved 300-DPI PDFium renders.

### Excluded

- Borderless or weakly implied tables.
- Merged cells, nested tables, spanning headers, and multiple tables per page.
- Semantic field extraction or form filling.
- Lexicon or LLM correction.
- Changes to `fileconv-core`, server workers, web APIs, desktop behavior, or
  production defaults.
- A full 839-page table conversion.

Unsupported inputs must return a typed `not_detected`, `unsupported`, or
`invalid_grid` result. They must never be forced into a guessed table.

## Alternatives considered

### Selected: ruled-line detection and per-cell OCR

Use deterministic pixel projections and run-length evidence to locate long
horizontal and vertical rules, repair small gaps, infer intersections, crop
cells, remove borders, OCR each cell, and serialize the matrix.

This has the smallest dependency and deployment surface, directly targets the
approved document class, and supports objective cell-level scoring.

### Rejected for this spike: Tesseract TSV word clustering

Word bounding boxes could support borderless layouts, but assigning words to
rows and columns is ambiguous around empty cells and multiline content. It is
better evaluated after a ruled-grid baseline exists.

### Rejected for this spike: OpenCV or learned table models

These approaches cover more layouts but add heavy dependencies, increase CPU
and memory cost, and expand native/dependency review scope before the simplest
case has been measured.

## Corpus and split

The benchmark freezes exactly:

- six tuning pages with ruled tables;
- three holdout pages with ruled tables;
- three negative pages verified to contain no ruled table.

Page 450 is included in tuning because it is already inspected and cannot be a
holdout. The remaining pages must represent distinct table/form templates, not
adjacent copies of one template. Selection, source SHA-256, page numbers,
render hashes, split, reviewer identity, and annotation revision are frozen in
an ignored manifest before tuning begins.

Holdout images and annotations are not opened during parameter tuning. The
runner records actual page access and fails if a holdout page is opened by a
tuning command.

Ground-truth text, document crops, renders, and OCR output are corpus artifacts
and remain under the ignored benchmark data directory. Tracked reports contain
only aggregate counts, metrics, non-sensitive hashes, configuration, and the
decision.

## Ground-truth schema

Each annotated table records:

- source and render hashes;
- page number and split;
- table bounding box;
- exact row and column counts;
- one entry for every `(row, column)` coordinate;
- cell bounding box;
- normalized reference text;
- whether the cell is intentionally blank;
- reviewer and revision metadata.

Annotations must form one complete rectangular matrix with unique coordinates,
non-overlapping positive-area boxes inside the table bounds, and no missing
cells. Validation fails closed before any experiment runs.

## Detection pipeline

1. Load a bounded 300-DPI render produced by the existing PDFium helper.
2. Convert to grayscale and create a deterministic binary mask.
3. Calculate row and column ink projections.
4. Detect sufficiently long horizontal and vertical runs.
5. Merge collinear segments separated only by a bounded gap.
6. Cluster nearby line coordinates into one canonical line.
7. Find intersections and connected rectangular table regions.
8. Reject regions that cannot form a complete rectangular grid.
9. Select at most one valid region; multiple valid regions are `unsupported`.
10. Derive cell rectangles from adjacent line coordinates.
11. Apply a bounded inset, remove residual border pixels, and OCR each cell
    with Tesseract `tessdata_best`, language `vie`, using a cell-appropriate
    page segmentation mode.
12. Normalize cell whitespace only; do not correct recognized words.
13. Serialize the matrix as a Markdown pipe table, using `<br>` for multiline
    content and escaping literal pipes.

The whole-page OCR remains unchanged. During the spike, the table result is a
sidecar artifact and does not replace any portion of the page Markdown.

## Components

### `experiments/ruled_table.py`

Owns immutable configuration, bounded render loading, detector orchestration,
candidate execution, schema validation, metric aggregation, gate derivation,
and deterministic report generation.

### `experiments/table_lines.py`

Owns binary masks, projections, run detection, gap repair, line clustering,
intersections, grid validation, and cell rectangle derivation. It has no OCR,
filesystem, subprocess, or Markdown responsibility.

### `experiments/table_cells.py`

Owns safe cell crops, border removal, bounded Tesseract invocation, whitespace
normalization, and Markdown escaping/serialization. It uses the existing
bounded candidate process contract rather than an ad hoc subprocess path.

### Corpus artifacts

An ignored manifest and annotation file bind private/raw data to tracked
aggregate evidence. A tracked metadata file may list only public source
identity, page counts, split counts, schema version, and hashes that do not
identify a private reference.

## Resource and safety bounds

- Existing benchmark PDF and render byte limits remain in force.
- Maximum render dimensions and decoded pixel count are validated before
  allocation.
- Maximum grid size is 50 rows by 30 columns.
- Maximum cells per page is 1,500.
- Maximum one table region per page.
- Maximum Tesseract output per cell and per page is byte-bounded.
- Each subprocess has a timeout and process-group cleanup on success, timeout,
  output overflow, and error.
- Total page wall time must not exceed 20 seconds for the accepted gate.
- Sampled process-tree peak RSS must remain strictly below 768 MiB.
- Temporary directories are runner-owned and removed on every exit path.
- Commands use direct argv, a sanitized environment allowlist, and no shell.
- Reports never contain recognized text, reference text, environment values,
  private paths, or private-reference hashes.

## Metrics

Metrics are computed per page and additively across a split.

### Structure

- Exact row-count match.
- Exact column-count match.
- Cell detection precision, recall, and F1.
- A predicted cell matches one reference cell only when intersection-over-union
  is at least 0.80; matching is one-to-one.

### Content

- Additive character edits divided by additive reference characters, labeled
  cell CER because references are human-reviewed.
- Additive word edits divided by additive reference words.
- Empty-cell accuracy over all annotated cells.

### Safety and performance

- False-positive tables on negative pages.
- Success, typed failure, timeout, and resource-limit counts.
- Median and p95 page latency.
- Sampled process-tree peak RSS.
- Complete source, render, configuration, binary, Tesseract, tessdata, host,
  and toolchain provenance.

## Acceptance gate

The holdout passes only when every condition holds:

- every table page has exact row and column counts;
- cell detection F1 is at least 95%;
- additive cell CER is at most 5%;
- empty-cell accuracy is at least 98%;
- all three negative pages return `not_detected`;
- no page fails, times out, crashes, or violates a resource bound;
- every page completes within 20 seconds;
- sampled process-tree peak RSS remains strictly below 768 MiB;
- all artifact cardinality, schema, access, and provenance checks pass.

Tuning results cannot satisfy the gate. Holdout metrics are evaluated once
after the configuration is frozen. A failure produces a `STOP` decision and
does not authorize a Rust port or full-document run.

## Testing

Unit fixtures cover:

- clean rectangular grids;
- small gaps in otherwise continuous rules;
- isolated noise and dotted leaders;
- slight skew within the frozen tolerance;
- blank cells;
- multiline cell text;
- literal Markdown pipe characters;
- incomplete intersections;
- merged-cell-like geometry;
- excessive rows, columns, pixels, and output;
- multiple table regions;
- negative prose pages;
- timeout, process-tree cleanup, and temporary-directory cleanup;
- closed schemas and deterministic reports.

Real-page tuning and holdout evidence is reviewed independently. The reviewer
must be able to recompute all aggregate metrics from ignored raw records
without reading recognized or reference text into the tracked report.

## Decision after the spike

- `PASS`: preserve the exact configuration and evidence, then design a
  separate production integration with required native/public-contract review.
- `STOP`: retain the benchmark report and failure analysis; do not modify the
  Rust PDF path.

Passing this spike is evidence only for simple ruled tables similar to the
frozen corpus. It is not evidence for borderless, merged, nested, or arbitrary
tables.
