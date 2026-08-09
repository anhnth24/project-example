# CPU Ruled-Table OCR Spike Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build and evaluate a benchmark-only CPU pipeline that recovers simple ruled tables from scanned Vietnamese PDF pages as cell matrices and Markdown.

**Architecture:** Render a frozen 12-page corpus at 300 DPI, detect rectangular grids with deterministic Pillow pixel operations, OCR bounded cell crops with the existing isolated candidate worker, and evaluate structure plus human-reviewed cell text. Raw renders, annotations, and OCR text remain ignored; only aggregate evidence and a PASS/STOP report are tracked.

**Tech Stack:** Python 3.12, Pillow, pypdfium2, Tesseract CLI with `tessdata_best` and `vie`, pytest, existing `benchmark.run` process isolation, existing `benchmark.metrics` edit counts.

## Global Constraints

- This is benchmark-only. Do not modify `fileconv-core`, CLI behavior, server workers, web APIs, desktop behavior, or production defaults.
- Do not add OpenCV, learned models, Python packages, Rust crates, or native dependencies.
- Freeze exactly six tuning table pages, three holdout table pages, and three negative pages.
- Page 450 is tuning-only. Adjacent copies of the same form template cannot occupy both tuning and holdout.
- Holdout annotations must have `review_status: "human_verified"` before the one allowed holdout run. An agent must not claim human review.
- Support at most one rectangular ruled table per page, with no merged or nested cells.
- Hard limits: 50 rows, 30 columns, 1,500 cells, one table region, 50,000,000 decoded pixels, 10,000 pixels per dimension, 20 seconds per page, 1,048,576 output bytes per page, and sampled process-tree RSS strictly below 805,306,368 bytes (768 MiB).
- A predicted cell matches a reference cell only at IoU >= 0.80 with one-to-one matching.
- Holdout gate: exact row and column counts on every table page, cell F1 >= 0.95, additive cell CER <= 0.05, empty-cell accuracy >= 0.98, all three negatives `not_detected`, and no failure, timeout, crash, or resource violation.
- Unsupported, ambiguous, or excessive geometry returns `not_detected`, `unsupported`, or `invalid_grid`; never guess a table.
- Ground truth, renders, crops, OCR text, overlays, and raw records remain under `bench/ocr_cpu_service/.data/ruled-table/`.
- Tracked reports contain aggregate counts, metrics, public source/config/tool hashes, access counts, and decisions only. They contain no recognized/reference text, private path, environment value, or private-reference hash.
- Commands use direct argv and the existing sanitized candidate environment. Every worker is closed on success and every error path.
- Run the holdout only once after a tuning winner is frozen. If any gate fails, record `STOP`; do not port to Rust or run all 839 pages.

---

## File map

- `bench/ocr_cpu_service/experiments/ruled-table-config.json` — canonical immutable detector candidates, render limits, process bounds, metric thresholds, and gate.
- `bench/ocr_cpu_service/experiments/table_lines.py` — pure Pillow geometry: mask, bounded deskew, line runs, clustering, intersections, grid and cell boxes.
- `bench/ocr_cpu_service/experiments/table_cells.py` — safe cell crops, border suppression, Tesseract candidate construction, OCR orchestration, normalization, and Markdown serialization.
- `bench/ocr_cpu_service/experiments/ruled_table.py` — corpus/annotation schemas, rendering, tuning and holdout orchestration, provenance, metrics, gate, CLI, and deterministic report.
- `bench/ocr_cpu_service/tests/test_table_lines.py` — synthetic detector and geometry tests.
- `bench/ocr_cpu_service/tests/test_table_cells.py` — crop, OCR contract, limits, and Markdown tests.
- `bench/ocr_cpu_service/tests/test_ruled_table.py` — corpus, schema, access, metrics, provenance, gate, cleanup, and report tests.
- `bench/ocr_cpu_service/reports/ruled-table-corpus.md` — tracked corpus counts, public source identity, split/template policy, review status counts, and hashes without annotation text.
- `bench/ocr_cpu_service/reports/ruled-table-spike.md` — tracked aggregate tuning/holdout result and PASS/STOP decision.
- `bench/ocr_cpu_service/.data/ruled-table/manifest.json` — ignored exact page manifest.
- `bench/ocr_cpu_service/.data/ruled-table/annotations/*.json` — ignored human-reviewed cell references.
- `bench/ocr_cpu_service/.data/ruled-table/renders/` — ignored exact 300-DPI inputs.
- `bench/ocr_cpu_service/.data/ruled-table/raw/` — ignored candidate records and sidecar Markdown.

---

### Task 1: Freeze corpus and annotation contracts

**Files:**
- Create: `bench/ocr_cpu_service/experiments/ruled-table-config.json`
- Create: `bench/ocr_cpu_service/experiments/ruled_table.py`
- Create: `bench/ocr_cpu_service/tests/test_ruled_table.py`
- Create: `bench/ocr_cpu_service/reports/ruled-table-corpus.md`
- Runtime only: `bench/ocr_cpu_service/.data/ruled-table/manifest.json`
- Runtime only: `bench/ocr_cpu_service/.data/ruled-table/annotations/*.json`

**Interfaces:**
- Produces: `load_config(path: Path) -> dict[str, Any]`
- Produces: `load_manifest(path: Path, *, mode: Literal["tuning", "holdout"]) -> CorpusManifest`
- Produces: `load_annotation(path: Path, *, expected_render_sha256: str) -> PageAnnotation`
- Produces: `render_frozen_pages(...) -> tuple[RenderedPage, ...]`
- Produces: `write_corpus_report(manifest: CorpusManifest, output: Path) -> None`
- `CorpusManifest` exposes `source_sha256`, `tuning`, `holdout`, and `negative` tuples.
- `PageAnnotation` exposes `page_number`, `split`, `negative`, `table`, and verified review metadata.

- [ ] **Step 1: Write failing closed-schema and access tests**

Add exact fixtures to `tests/test_ruled_table.py`:

```python
def valid_annotation(*, page_number: int = 450, split: str = "tuning") -> dict:
    return {
        "schema_version": 1,
        "source_sha256": "a" * 64,
        "render_sha256": "b" * 64,
        "page_number": page_number,
        "split": split,
        "negative": False,
        "review": {
            "review_status": "human_verified",
            "reviewer": "reviewer@example.invalid",
            "revision": 1,
            "reviewed_at": "2026-08-09T00:00:00Z",
        },
        "table": {
            "bbox": [10, 20, 210, 120],
            "rows": 2,
            "columns": 2,
            "cells": [
                {"row": 0, "column": 0, "bbox": [10, 20, 110, 70],
                 "text": "Mã", "blank": False},
                {"row": 0, "column": 1, "bbox": [110, 20, 210, 70],
                 "text": "Giá trị", "blank": False},
                {"row": 1, "column": 0, "bbox": [10, 70, 110, 120],
                 "text": "01", "blank": False},
                {"row": 1, "column": 1, "bbox": [110, 70, 210, 120],
                 "text": "", "blank": True},
            ],
        },
    }


def test_annotation_requires_complete_non_overlapping_matrix(tmp_path):
    annotation = valid_annotation()
    annotation["table"]["cells"].pop()
    path = tmp_path / "annotation.json"
    path.write_text(json.dumps(annotation), encoding="utf-8")
    with pytest.raises(ValueError, match="complete rectangular matrix"):
        load_annotation(path, expected_render_sha256="b" * 64)


def test_holdout_rejects_non_human_review(tmp_path):
    annotation = valid_annotation(split="holdout")
    annotation["review"]["review_status"] = "draft"
    path = tmp_path / "annotation.json"
    path.write_text(json.dumps(annotation), encoding="utf-8")
    with pytest.raises(ValueError, match="human_verified"):
        load_annotation(path, expected_render_sha256="b" * 64)


def test_tuning_mode_cannot_open_holdout_annotations(tmp_path):
    manifest = write_manifest_fixture(tmp_path, tuning=6, holdout=3, negative=3)
    with pytest.raises(PermissionError, match="holdout access denied"):
        load_manifest(manifest, mode="tuning").open_page(700)
```

Also test unknown keys, wrong scalar types, malformed SHA-256, duplicate page
numbers, page 450 outside tuning, adjacent-template leakage, wrong 6/3/3
cardinality, overlapping cells, boxes outside table bounds, nonblank text in a
blank cell, empty reviewer, and an annotation hash mismatch.

- [ ] **Step 2: Run RED**

Run:

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m pytest \
  bench/ocr_cpu_service/tests/test_ruled_table.py -q
```

Expected: collection fails because `experiments.ruled_table` does not exist.

- [ ] **Step 3: Add the canonical configuration**

Create `experiments/ruled-table-config.json` with these exact values:

```json
{
  "schema_version": 1,
  "source": {
    "id": "official-89-2026-tt-btc",
    "expected_sha256": "952c45ffc0f10bfc176bd9ae6b3d204fd3a034294ee270278957b9c11e1471dc",
    "max_bytes": 17281751
  },
  "render": {"dpi": 300, "max_pixels": 50000000, "max_dimension": 10000},
  "geometry_limits": {
    "max_rows": 50,
    "max_columns": 30,
    "max_cells": 1500,
    "max_table_regions": 1,
    "cell_match_iou": 0.8
  },
  "process_limits": {
    "cpu_threads": 1,
    "page_timeout_seconds": 20,
    "cell_timeout_seconds": 10,
    "max_output_bytes_per_cell": 65536,
    "max_output_bytes_per_page": 1048576,
    "max_rss_bytes": 805306368,
    "sample_interval_ms": 10
  },
  "detector_candidates": [
    {
      "id": "strict-psm6",
      "dark_max": 128,
      "min_horizontal_fraction": 0.25,
      "min_vertical_fraction": 0.10,
      "max_gap_pixels": 8,
      "cluster_tolerance_pixels": 4,
      "intersection_tolerance_pixels": 4,
      "deskew_angles_degrees": [-1.5, -1.0, -0.5, 0.0, 0.5, 1.0, 1.5],
      "cell_inset_pixels": 4,
      "psm": 6
    },
    {
      "id": "balanced-psm6",
      "dark_max": 160,
      "min_horizontal_fraction": 0.20,
      "min_vertical_fraction": 0.08,
      "max_gap_pixels": 12,
      "cluster_tolerance_pixels": 5,
      "intersection_tolerance_pixels": 5,
      "deskew_angles_degrees": [-1.5, -1.0, -0.5, 0.0, 0.5, 1.0, 1.5],
      "cell_inset_pixels": 4,
      "psm": 6
    },
    {
      "id": "balanced-psm7",
      "dark_max": 160,
      "min_horizontal_fraction": 0.20,
      "min_vertical_fraction": 0.08,
      "max_gap_pixels": 12,
      "cluster_tolerance_pixels": 5,
      "intersection_tolerance_pixels": 5,
      "deskew_angles_degrees": [-1.5, -1.0, -0.5, 0.0, 0.5, 1.0, 1.5],
      "cell_inset_pixels": 4,
      "psm": 7
    }
  ],
  "gate": {
    "exact_grid_required": true,
    "minimum_cell_f1": 0.95,
    "maximum_cell_cer": 0.05,
    "minimum_empty_cell_accuracy": 0.98,
    "maximum_negative_false_positives": 0
  }
}
```

Validate the exact candidate IDs and semantics, all numeric ranges, the
cross-field `rows * columns <= max_cells` relationship, fixed source identity,
and exact config SHA-256 against the checked-in bytes.

- [ ] **Step 4: Implement immutable schemas and bounded rendering**

In `ruled_table.py`, add frozen dataclasses:

```python
@dataclass(frozen=True, slots=True)
class CellReference:
    row: int
    column: int
    bbox: tuple[int, int, int, int]
    text: str
    blank: bool


@dataclass(frozen=True, slots=True)
class TableReference:
    bbox: tuple[int, int, int, int]
    rows: int
    columns: int
    cells: tuple[CellReference, ...]


@dataclass(frozen=True, slots=True)
class PageAnnotation:
    page_number: int
    split: str
    negative: bool
    source_sha256: str
    render_sha256: str
    review_status: str
    reviewer: str
    revision: int
    table: TableReference | None
```

Use exact-key validation before indexing fields so missing fields raise
`ValueError`, not `KeyError`. Read the source only after checking
`stat().st_size <= 17_281_751`, verify its canonical SHA-256, and render via
`benchmark.render.open_pdf` plus `render_page` using the canonical limits.
Write each render atomically to the ignored render directory, then bind its
SHA-256 into the manifest.

The manifest loader accepts a mode and exposes pages through an access-counting
method. In tuning mode, any holdout access raises before opening the render or
annotation.

- [ ] **Step 5: Freeze the 12-page manifest and draft annotations**

Inspect the public source and select six distinct tuning tables including page
450, three nonadjacent holdout table templates, and three verified negative
prose pages. Render and hash them before detector work.

Create complete draft annotations under `.data/ruled-table/annotations/`.
Do not set `human_verified` yourself. Generate ignored overlay PNGs that draw
the annotated table and cell boxes for human review.

The implementer may continue synthetic detector work while annotations are
draft. The live holdout task remains blocked until a human changes the review
status and signs reviewer/revision metadata.

- [ ] **Step 6: Generate the corpus report**

`write_corpus_report` must emit only:

```markdown
# Ruled-table OCR corpus

- Source id: `official-89-2026-tt-btc`
- Source SHA-256: `<public source hash>`
- Manifest SHA-256: `<manifest hash>`
- Tuning table pages: 6
- Holdout table pages: 3
- Negative pages: 3
- Human-verified annotations: <count>/12
- Distinct template families: <count>
- Ground-truth text tracked: no
```

Add a regression test that inserts a unique annotation phrase, private path,
and annotation SHA into fixture input and asserts none appears in the report.

- [ ] **Step 7: Run GREEN and full benchmark tests**

Run:

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m pytest \
  bench/ocr_cpu_service/tests/test_ruled_table.py -q

PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m pytest \
  bench/ocr_cpu_service/tests -q
```

Expected: all tests pass. Real corpus report may state fewer than 12 verified
annotations; that is a documented holdout blocker, not a reason to falsify it.

- [ ] **Step 8: Commit and push**

```bash
git add \
  bench/ocr_cpu_service/experiments/ruled-table-config.json \
  bench/ocr_cpu_service/experiments/ruled_table.py \
  bench/ocr_cpu_service/tests/test_ruled_table.py \
  bench/ocr_cpu_service/reports/ruled-table-corpus.md
git commit -m "bench(ocr): freeze ruled-table corpus contracts"
git push -u origin cursor/vietnamese-ocr-accuracy-e533
```

Verify `git status --short --ignored` shows all raw annotations, renders, and
overlays ignored and none staged.

---

### Task 2: Detect rectangular ruled grids

**Files:**
- Create: `bench/ocr_cpu_service/experiments/table_lines.py`
- Create: `bench/ocr_cpu_service/tests/test_table_lines.py`
- Modify: `bench/ocr_cpu_service/experiments/ruled_table.py`

**Interfaces:**
- Consumes: one Pillow render and one canonical detector candidate.
- Produces: `detect_ruled_table(image: Image.Image, config: DetectorConfig) -> DetectionResult`
- Produces: `prepare_working_image(image: Image.Image, angle_degrees: float) -> Image.Image`
- `DetectionResult.status` is exactly `detected`, `not_detected`, `unsupported`, or `invalid_grid`.
- A detected result carries one `Grid`, the deskew angle, transformed working
  image size, and original-coordinate cell boxes. It does not own a Pillow
  image; the orchestrator recreates the working image with
  `prepare_working_image` and closes it after cell OCR.
- Later tasks consume `Grid.rows`, `Grid.columns`, `Grid.cells`,
  `Grid.working_table_box`, and `Grid.original_table_box`.

- [ ] **Step 1: Write synthetic RED tests**

Create a fixture helper and exact assertions:

```python
def grid_image(
    *,
    width: int = 240,
    height: int = 160,
    xs: tuple[int, ...] = (20, 120, 220),
    ys: tuple[int, ...] = (20, 80, 140),
) -> Image.Image:
    image = Image.new("L", (width, height), 255)
    draw = ImageDraw.Draw(image)
    for x in xs:
        draw.line((x, ys[0], x, ys[-1]), fill=0, width=2)
    for y in ys:
        draw.line((xs[0], y, xs[-1], y), fill=0, width=2)
    return image


def test_detects_exact_two_by_two_grid():
    result = detect_ruled_table(grid_image(), balanced_config())
    assert result.status == "detected"
    assert (result.grid.rows, result.grid.columns) == (2, 2)
    assert [cell.coordinate for cell in result.grid.cells] == [
        (0, 0), (0, 1), (1, 0), (1, 1)
    ]


def test_dotted_leader_without_vertical_intersections_is_not_a_table():
    image = dotted_leader_fixture()
    assert detect_ruled_table(image, balanced_config()).status == "not_detected"


def test_two_valid_grids_are_unsupported():
    image = two_grid_fixture()
    assert detect_ruled_table(image, balanced_config()).status == "unsupported"
```

Add fixtures for an 8-pixel gap, a gap beyond the configured maximum, isolated
noise, ±1.5-degree rotation, blank cells, incomplete intersections,
merged-cell-like missing rules, 51 rows, 31 columns, 1,501 cells, zero-size
images, and pixel/dimension overflow.

- [ ] **Step 2: Run RED**

Run:

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m pytest \
  bench/ocr_cpu_service/tests/test_table_lines.py -q
```

Expected: collection fails because `experiments.table_lines` does not exist.

- [ ] **Step 3: Define immutable geometry**

Implement:

```python
@dataclass(frozen=True, slots=True)
class Box:
    left: int
    top: int
    right: int
    bottom: int

    @property
    def area(self) -> int:
        return max(0, self.right - self.left) * max(0, self.bottom - self.top)


@dataclass(frozen=True, slots=True)
class GridCell:
    row: int
    column: int
    working_box: Box
    original_box: Box

    @property
    def coordinate(self) -> tuple[int, int]:
        return self.row, self.column


@dataclass(frozen=True, slots=True)
class Grid:
    rows: int
    columns: int
    working_table_box: Box
    original_table_box: Box
    cells: tuple[GridCell, ...]


@dataclass(frozen=True, slots=True)
class DetectionResult:
    status: Literal["detected", "not_detected", "unsupported", "invalid_grid"]
    deskew_angle_degrees: float
    working_size: tuple[int, int]
    grid: Grid | None
    diagnostics: Mapping[str, int | float | str]
```

Validate positive box areas, unique complete coordinates, row-major ordering,
strictly increasing line coordinates, and all configured limits in constructors.

- [ ] **Step 4: Implement bounded deskew**

Downsample only for angle scoring so its longest side is at most 1,000 pixels.
For each configured angle, rotate with Pillow using `expand=False`, white fill,
and bicubic resampling. Score the top horizontal and vertical dark-run lengths.
Choose the highest score, then lowest absolute angle, then lowest signed angle.
Rotate the full image once at the winner.

Keep forward and inverse affine coefficients. Grid extraction occurs in working
coordinates; map all four cell corners through the inverse transform and use
their clamped axis-aligned envelope for the original-coordinate box.
`prepare_working_image` applies the same full-resolution rotation and returns a
new owned image whose size must equal `DetectionResult.working_size`.

Tests must prove the unrotated fixture chooses 0.0, ±1.5-degree fixtures recover
a 2x2 grid, and mapped original boxes reach IoU >= 0.80 against fixture boxes.

- [ ] **Step 5: Implement line and grid detection**

Use a binary mask where `pixel <= dark_max`. For each row and column:

1. find dark runs while bridging no more than `max_gap_pixels`;
2. retain horizontal runs at least
   `ceil(width * min_horizontal_fraction)` and vertical runs at least
   `ceil(height * min_vertical_fraction)`;
3. merge overlapping collinear runs;
4. cluster line coordinates within `cluster_tolerance_pixels` using the rounded
   median coordinate;
5. connect one horizontal and one vertical line only when both segments cover
   the intersection within `intersection_tolerance_pixels`;
6. form connected rectangular regions with at least three horizontal and three
   vertical canonical lines;
7. require every row/column line pair in a region to intersect;
8. derive adjacent-line cells and enforce all geometry limits.

No region returns `not_detected`; multiple complete regions returns
`unsupported`; one incomplete/merged-like candidate returns `invalid_grid`.
Diagnostics contain counts and geometry only, never pixels or OCR text.

- [ ] **Step 6: Run GREEN and fuzz bounded geometry**

Run:

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m pytest \
  bench/ocr_cpu_service/tests/test_table_lines.py -q
```

Then add a deterministic 100-seed test that generates bounded random grayscale
images up to 256x256 and asserts the detector returns a typed status without
exception or allocation outside the input size.

- [ ] **Step 7: Commit and push**

```bash
git add \
  bench/ocr_cpu_service/experiments/table_lines.py \
  bench/ocr_cpu_service/experiments/ruled_table.py \
  bench/ocr_cpu_service/tests/test_table_lines.py
git commit -m "bench(ocr): detect bounded ruled-table grids"
git push -u origin cursor/vietnamese-ocr-accuracy-e533
```

---

### Task 3: OCR cells and serialize Markdown

**Files:**
- Create: `bench/ocr_cpu_service/experiments/table_cells.py`
- Create: `bench/ocr_cpu_service/tests/test_table_cells.py`
- Modify: `bench/ocr_cpu_service/experiments/ruled_table.py`

**Interfaces:**
- Consumes: one detected `Grid`, its deskewed working image, canonical
  `cell_inset_pixels`, `psm`, tessdata path, and process limits.
- Produces: `recognize_grid(...) -> GridRecognition`
- Produces: `serialize_markdown(recognition: GridRecognition) -> str`
- `GridRecognition.cells` is complete, unique, row-major, and contains text,
  elapsed seconds, and resource measurements only in ignored memory/artifacts.
- The orchestrator receives aggregate counts and a SHA-256 of the sidecar, not
  text for tracked output.

- [ ] **Step 1: Write crop and Markdown RED tests**

```python
def test_crop_insets_and_erases_residual_border():
    image, grid = bordered_cell_fixture()
    crop = prepare_cell_crop(image, grid.cells[0], inset_pixels=4)
    assert crop.size == (92, 42)
    assert min(crop.getpixel((x, 0)) for x in range(crop.width)) == 255


def test_markdown_escapes_pipes_and_preserves_multiline_cells():
    recognition = grid_recognition(
        rows=2,
        columns=2,
        texts=("Mã | code", "Giá trị", "01\nA", ""),
    )
    assert serialize_markdown(recognition) == (
        "| Mã \\| code | Giá trị |\n"
        "|---|---|\n"
        "| 01<br>A |  |\n"
    )


def test_output_budget_is_additive_across_cells():
    with pytest.raises(PageOutputLimitError):
        enforce_page_output_budget([40_000, 40_000], maximum=65_536)
```

Also test blank crops, zero-area insets, PSM allowlist `{6, 7}`, complete
row-major coordinates, 1,500-cell cap, per-cell and page output overflow,
timeout mapping, sanitized failures, worker cleanup, and no shell invocation.

- [ ] **Step 2: Run RED**

Run:

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m pytest \
  bench/ocr_cpu_service/tests/test_table_cells.py -q
```

Expected: collection fails because `experiments.table_cells` does not exist.

- [ ] **Step 3: Implement bounded crop preparation**

`prepare_cell_crop` must:

- require a detected cell inside the working image;
- inset all sides by exactly `cell_inset_pixels`;
- reject a nonpositive interior;
- convert to grayscale;
- whiten a two-pixel strip around the crop after inset;
- return a detached Pillow image;
- close every temporary image owned by the function.

Skip OCR only when at least 99.5% of crop pixels are >= 223. Record that cell
as blank with zero subprocess time.

- [ ] **Step 4: Build exact Tesseract candidates**

Construct one candidate per detector config:

```python
def build_cell_candidate(
    *,
    candidate_id: str,
    psm: int,
    tessdata: Path,
    limits: ProcessLimits,
) -> CommandCandidateSpec:
    if psm not in {6, 7}:
        raise ValueError("cell PSM must be 6 or 7")
    environment = sanitized_candidate_environment(
        cpu_threads=limits.cpu_threads
    )
    environment["TESSDATA_PREFIX"] = str(tessdata)
    return CommandCandidateSpec(
        id=candidate_id,
        label=f"Tesseract vie PSM {psm}",
        argv=(
            "tesseract", "{input}", "stdout",
            "-l", "vie", "--psm", str(psm),
        ),
        environment=environment,
        provenance={
            "engine": "tesseract-cli",
            "langs": "vie",
            "psm": psm,
            "tessdata_sha256": hash_vie_traineddata(tessdata),
        },
    )
```

Require an HTTPS-free local path, exact `vie.traineddata` hash, and no inherited
environment values outside the sanitized mapping.

- [ ] **Step 5: OCR a grid with one bounded persistent worker**

Allocate a runner-owned temporary page directory. Save cell crops by numeric
coordinate, instantiate `_isolated_worker` once, recognize cells row-major,
and close the worker in `finally`.

Enforce:

- the 20-second page deadline before each request and after each response;
- 65,536 bytes per cell and 1,048,576 bytes cumulatively;
- sampled process-tree RSS strictly below 805,306,368 bytes;
- a sanitized typed failure for timeout, output limit, candidate failure, and
  resource violation;
- deletion of every crop and the owned directory on all paths.

Normalize only NFC and whitespace. Convert internal line breaks to `\n`; do not
change recognized words, punctuation, or accents.

- [ ] **Step 6: Implement deterministic Markdown**

Use the first matrix row as the Markdown header because simple ruled tables are
the approved scope. Escape backslashes before pipes, replace newlines with
`<br>`, trim cell edges, and always emit one separator row with exactly the
detected column count.

If the grid has fewer than two rows or two columns, return `invalid_grid`
before serialization.

- [ ] **Step 7: Run GREEN and full benchmark tests**

Run:

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m pytest \
  bench/ocr_cpu_service/tests/test_table_cells.py -q

PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m pytest \
  bench/ocr_cpu_service/tests -q
```

Expected: all tests pass.

- [ ] **Step 8: Commit and push**

```bash
git add \
  bench/ocr_cpu_service/experiments/table_cells.py \
  bench/ocr_cpu_service/experiments/ruled_table.py \
  bench/ocr_cpu_service/tests/test_table_cells.py
git commit -m "bench(ocr): recognize ruled-table cells"
git push -u origin cursor/vietnamese-ocr-accuracy-e533
```

---

### Task 4: Add metrics, gate, and deterministic runner

**Files:**
- Modify: `bench/ocr_cpu_service/experiments/ruled_table.py`
- Modify: `bench/ocr_cpu_service/tests/test_ruled_table.py`
- Create: `bench/ocr_cpu_service/reports/ruled-table-spike.md`

**Interfaces:**
- Consumes: frozen manifest, verified annotations, canonical config, detector,
  cell recognizer, and raw ignored records.
- Produces: `run_split(split: Literal["tuning", "holdout"], ...) -> dict[str, Any]`
- Produces: `validate_artifact(payload: Mapping[str, Any], *, split: str) -> None`
- Produces: `derive_tuning_winner(payload: Mapping[str, Any]) -> str | None`
- Produces: `derive_holdout_decision(payload: Mapping[str, Any]) -> Literal["PASS", "STOP"]`
- Produces: `render_report(payload: Mapping[str, Any]) -> str`

- [ ] **Step 1: Write metric and gate RED tests**

```python
def test_iou_matching_is_one_to_one_at_exact_threshold():
    references = [Box(0, 0, 100, 100), Box(100, 0, 200, 100)]
    predictions = [Box(0, 0, 80, 100), Box(0, 0, 100, 100)]
    counts = match_cells(references, predictions, threshold=0.80)
    assert counts == CellMatchCounts(tp=1, fp=1, fn=1)


def test_holdout_gate_requires_every_condition():
    payload = passing_holdout_artifact()
    assert derive_holdout_decision(payload) == "PASS"
    payload["aggregate"]["cell_cer"] = 0.050001
    assert derive_holdout_decision(payload) == "STOP"


def test_negative_false_positive_forces_stop():
    payload = passing_holdout_artifact()
    payload["records"][-1]["status"] = "detected"
    assert derive_holdout_decision(payload) == "STOP"
```

Test every boundary: F1 `0.95` passes and `0.949999` stops; CER `0.05` passes;
empty accuracy `0.98` passes; RSS exactly `805_306_368` stops because it must be
strictly below; exactly 20 seconds passes because the contract says “must not
exceed,” and `20.000001` stops. Test one missing record,
duplicate page/candidate records, stale aggregates, unknown nested schema keys,
wrong hashes, text aliases (`recognized_text`, `reference`, `markdown`,
`environment`), and tuning access to holdout.

- [ ] **Step 2: Run RED**

Run:

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m pytest \
  bench/ocr_cpu_service/tests/test_ruled_table.py -q
```

Expected: fails because metric and gate functions are absent.

- [ ] **Step 3: Implement cell matching and additive content counts**

For all box pairs with IoU >= 0.80, sort candidates by:

1. descending IoU;
2. reference row and column;
3. predicted row and column.

Greedily accept only pairs whose reference and prediction are both unused.
Compute integer TP/FP/FN and derive precision, recall, and F1 from aggregate
counts, with an empty/empty set scoring 1.0.

Content counts use `benchmark.metrics.error_counts` on coordinate-aligned cells
only after exact rows and columns are confirmed. Missing cells count their full
reference as deletions; extra cells count their normalized hypothesis length
as insertions. Aggregate integer edits and reference units before division.
Empty-cell accuracy counts every annotated coordinate; a missing prediction is
incorrect.

- [ ] **Step 4: Define a closed raw artifact**

The raw artifact has exact top-level keys:

```python
{
    "schema_version": 1,
    "split": "tuning",
    "source": {...},
    "config_sha256": "...",
    "manifest_sha256": "...",
    "host": {...},
    "toolchain": {...},
    "access": {
        "tuning_pages_opened": 6,
        "holdout_pages_opened": 0,
        "negative_pages_opened": 0,
    },
    "candidates": [...],
    "records": [...],
    "aggregates": [...],
    "winner_id": None,
    "decision": None,
}
```

Records contain IDs, public page number, split, typed status, rows, columns,
predicted/reference boxes, per-coordinate integer edit/reference counts and
blank flags, elapsed seconds, peak RSS, resource flags, and provenance hashes.
The raw boxes and per-cell counts are ignored evidence used to recompute
TP/FP/FN, CER/WER, and empty-cell accuracy. Records do not contain cell text,
Markdown, image bytes, filesystem paths, command environment values, or error
details.

Recursively validate exact keys and scalar types. Recompute every aggregate,
access count, rate, winner, and decision from records. Validate config,
manifest, source, render, Tesseract, tessdata, host, and toolchain hashes
against canonical inputs rather than internal agreement alone.

- [ ] **Step 5: Implement tuning winner and holdout decision**

Disqualify a tuning candidate with any failure, timeout, resource violation,
false positive, wrong grid, or missing record. Rank remaining candidates by:

1. more exact-grid table pages;
2. higher additive cell F1;
3. lower additive cell CER;
4. higher empty-cell accuracy;
5. lower median page latency;
6. lexicographically smaller candidate ID.

Freeze the exact winner ID and configuration SHA-256 before holdout.

For holdout, apply the Global Constraints gate exactly. Do not compare
candidate IDs or retune. `PASS` is possible only for the frozen tuning winner
and only when all 3 table plus 3 negative records succeed under bounds.

- [ ] **Step 6: Implement CLI and deterministic report**

Add subcommands:

```text
python -m experiments.ruled_table inventory ...
python -m experiments.ruled_table tune ...
python -m experiments.ruled_table holdout ...
python -m experiments.ruled_table validate ...
python -m experiments.ruled_table report ...
```

`inventory` cannot OCR. `tune` cannot open holdout. `holdout` requires a frozen
winner artifact and all holdout/negative annotations human-verified.

The tracked report includes:

- public provenance and non-sensitive hashes;
- exact split and access counts;
- bounds;
- per-candidate aggregate integer counts and rates;
- tuning winner and frozen config hash;
- holdout gate rows with measured value, threshold, and pass/fail;
- explicit `PASS` or `STOP`;
- limitations covering one table/page, visible rules, no merged cells, tiny
  corpus, and no production/full-document authorization.

Add a fixture with unique OCR/reference/path/environment secrets and assert
none occurs in the report. Generate twice and compare bytes.

- [ ] **Step 7: Run GREEN and full tests**

Run:

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m pytest \
  bench/ocr_cpu_service/tests/test_ruled_table.py -q

PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m pytest \
  bench/ocr_cpu_service/tests -q
```

Expected: all tests pass.

- [ ] **Step 8: Commit and push**

```bash
git add \
  bench/ocr_cpu_service/experiments/ruled_table.py \
  bench/ocr_cpu_service/tests/test_ruled_table.py \
  bench/ocr_cpu_service/reports/ruled-table-spike.md
git commit -m "bench(ocr): gate ruled-table recovery evidence"
git push -u origin cursor/vietnamese-ocr-accuracy-e533
```

---

### Task 5: Run tuning, one-time holdout, and independent handoff

**Files:**
- Modify: `bench/ocr_cpu_service/reports/ruled-table-corpus.md`
- Modify: `bench/ocr_cpu_service/reports/ruled-table-spike.md`
- Runtime only: `bench/ocr_cpu_service/.data/ruled-table/raw/*.json`
- Runtime only: `bench/ocr_cpu_service/.data/ruled-table/sidecars/*.md`

**Interfaces:**
- Consumes all reviewed Task 1–4 interfaces.
- Produces the final tracked aggregate report and an explicit `PASS` or `STOP`.
- Does not produce a Rust patch or full-document output.

- [ ] **Step 1: Verify annotation readiness**

Run:

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python \
  -m experiments.ruled_table inventory \
  --manifest bench/ocr_cpu_service/.data/ruled-table/manifest.json \
  --annotations bench/ocr_cpu_service/.data/ruled-table/annotations \
  --output bench/ocr_cpu_service/reports/ruled-table-corpus.md \
  --require-human-review
```

Expected: exactly 12/12 human-verified annotations, 6 tuning table pages, 3
holdout table pages, 3 negatives, no template leakage, and zero holdout opens.

If this fails because human review is missing, report `BLOCKED` and stop. Do not
change review metadata or continue to holdout.

- [ ] **Step 2: Run all three candidates on tuning only**

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python \
  -m experiments.ruled_table tune \
  --config bench/ocr_cpu_service/experiments/ruled-table-config.json \
  --manifest bench/ocr_cpu_service/.data/ruled-table/manifest.json \
  --annotations bench/ocr_cpu_service/.data/ruled-table/annotations \
  --pdf bench/ocr_cpu_service/.data/corpus/official-89-2026-tt-btc.signed.pdf \
  --tessdata tessdata_best \
  --output bench/ocr_cpu_service/.data/ruled-table/raw/tuning.json
```

Expected cardinality: 3 candidates x 6 tuning pages = 18 records. Access
evidence must show 6 tuning, 0 holdout, and 0 negative pages opened.

- [ ] **Step 3: Validate and freeze the tuning winner**

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python \
  -m experiments.ruled_table validate \
  --input bench/ocr_cpu_service/.data/ruled-table/raw/tuning.json \
  --split tuning
```

If no candidate remains after disqualification, generate a tuning `STOP`
report and do not run holdout.

If a winner exists, record its exact ID and configuration SHA-256 in an ignored
freeze file and the tracked report. Do not edit detector values afterward.

- [ ] **Step 4: Run the one allowed holdout**

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python \
  -m experiments.ruled_table holdout \
  --config bench/ocr_cpu_service/experiments/ruled-table-config.json \
  --frozen-winner bench/ocr_cpu_service/.data/ruled-table/raw/winner.json \
  --manifest bench/ocr_cpu_service/.data/ruled-table/manifest.json \
  --annotations bench/ocr_cpu_service/.data/ruled-table/annotations \
  --pdf bench/ocr_cpu_service/.data/corpus/official-89-2026-tt-btc.signed.pdf \
  --tessdata tessdata_best \
  --output bench/ocr_cpu_service/.data/ruled-table/raw/holdout.json
```

Expected cardinality: 1 candidate x (3 holdout + 3 negative pages) = 6 records.
Write an immutable holdout-run marker before opening the first holdout page;
future attempts must fail closed.

- [ ] **Step 5: Validate, report, and prove determinism**

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python \
  -m experiments.ruled_table validate \
  --input bench/ocr_cpu_service/.data/ruled-table/raw/holdout.json \
  --split holdout

PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python \
  -m experiments.ruled_table report \
  --tuning bench/ocr_cpu_service/.data/ruled-table/raw/tuning.json \
  --holdout bench/ocr_cpu_service/.data/ruled-table/raw/holdout.json \
  --output bench/ocr_cpu_service/reports/ruled-table-spike.md

PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python \
  -m experiments.ruled_table report \
  --tuning bench/ocr_cpu_service/.data/ruled-table/raw/tuning.json \
  --holdout bench/ocr_cpu_service/.data/ruled-table/raw/holdout.json \
  --output /tmp/ruled-table-spike.md

cmp bench/ocr_cpu_service/reports/ruled-table-spike.md \
  /tmp/ruled-table-spike.md
```

Expected: byte-identical reports and an explicit measured `PASS` or `STOP`.

- [ ] **Step 6: Run final verification**

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m pytest \
  bench/ocr_cpu_service/tests -q

cargo fmt --all -- --check
cargo metadata --locked --format-version 1 --no-deps
python3 scripts/check-dependency-policy.py
python3 scripts/check-architecture-boundaries.py
git diff --check
```

Expected: all commands exit 0.

Verify:

```bash
git status --short --ignored
git ls-files bench/ocr_cpu_service/.data
```

Expected: all raw data appears ignored and `git ls-files` prints nothing.

- [ ] **Step 7: Independent evidence review**

Give the reviewer the design, this plan, Task 1 annotation-review evidence,
tracked reports, ignored raw artifact paths, and the complete branch diff.
Require independent recomputation of:

- exact record cardinality and access counts;
- TP/FP/FN, F1, character/word edits, empty-cell accuracy;
- latency and RSS aggregates;
- source/render/config/tessdata/toolchain bindings;
- PASS/STOP gate;
- tracked-data privacy.

Any Critical or Important finding is fixed and re-reviewed before completion.

- [ ] **Step 8: Commit and push the measured result**

```bash
git add \
  bench/ocr_cpu_service/reports/ruled-table-corpus.md \
  bench/ocr_cpu_service/reports/ruled-table-spike.md
git commit -m "bench(ocr): report ruled-table recovery result"
git push -u origin cursor/vietnamese-ocr-accuracy-e533
```

If the decision is `STOP`, state that no Rust or 839-page work is authorized.
If it is `PASS`, create a separate production-integration design; do not add
production code in this plan.
