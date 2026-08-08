# Bitonal PDF OCR Improvement Spike Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Determine whether preserving 300-DPI near-bitonal PDF renders and using `tessdata_best` with a Vietnamese-only language candidate improves Thông tư 89 OCR without unsafe text correction.

**Architecture:** Add an explicit, default-off preprocessing policy to `OcrRunConfig`; the current path remains the default. A benchmark-only CLI environment selector, stripped by the web-worker sandbox, enables a deterministic near-bitonal bypass for controlled candidates. A Python harness renders only selected PDF pages, executes four Rust `fileconv` candidates serially, derives a winner from raw counts, and runs the full 839-page PDF only after the winner gate passes.

**Tech Stack:** Rust `image` and existing `fileconv-core`/CLI, PDFium through pypdfium2 for identical bounded calibration renders, Tesseract 5 `vie`/`eng`, existing Python benchmark metrics/psutil/Pillow/pytest.

## Global Constraints

- Current preprocessing remains the default; the spike is explicit opt-in.
- Web worker behavior, timeout, queue, sandbox, API, and deployment are unchanged.
- No lexicon replacement, LLM correction, table-cell OCR, or heading rewriting.
- The private 20-page reference is never committed and its disagreement metric is never called CER.
- No recognized text, reference text, source PDF, rendered page, or full Markdown output is tracked.
- Only pages 1–20, 60, and 450 may be opened before the winner is frozen.
- The full 839-page run occurs only if one candidate passes every gate.
- Every Rust push runs formatting, locked metadata, and dependency policy checks.

---

### Task 1: Explicit near-bitonal preprocessing policy

**Files:**
- Modify: `crates/core/src/image_ocr.rs:44-49,326-354`
- Modify: `crates/core/src/lib.rs:41`
- Test: inline tests in `crates/core/src/image_ocr.rs`

**Interfaces:**
- Produces: `pub enum OcrPreprocessMode { Legacy, PreserveNearBitonal }`
- Produces: `OcrRunConfig { tesseract_binary, preprocess_mode }`
- Produces: `is_effectively_bitonal(&GrayImage) -> bool`
- Produces: `preprocess_with_mode(&DynamicImage, OcrPreprocessMode) -> DynamicImage`

- [ ] **Step 1: Write failing classifier tests**

Add tests before implementation:

```rust
#[test]
fn effectively_bitonal_requires_ink_and_rejects_blank_or_gradient() {
    let blank = GrayImage::from_pixel(2500, 32, image::Luma([255]));
    assert!(!is_effectively_bitonal(&blank));

    let mut document = GrayImage::from_pixel(2500, 32, image::Luma([255]));
    for x in 100..2400 {
        document.put_pixel(x, 16, image::Luma([0]));
    }
    assert!(is_effectively_bitonal(&document));

    let gradient = GrayImage::from_fn(2500, 32, |x, _| {
        image::Luma([((x % 256) as u8)])
    });
    assert!(!is_effectively_bitonal(&gradient));
}

#[test]
fn near_bitonal_mode_preserves_large_dimensions() {
    let mut page = GrayImage::from_pixel(2455, 3523, image::Luma([255]));
    for y in (100..3400).step_by(40) {
        for x in 100..2300 {
            page.put_pixel(x, y, image::Luma([0]));
        }
    }
    let output = preprocess_with_mode(
        &DynamicImage::ImageLuma8(page),
        OcrPreprocessMode::PreserveNearBitonal,
    );
    assert_eq!(output.dimensions(), (2455, 3523));
}

#[test]
fn legacy_and_non_bitonal_behavior_remain_unchanged() {
    let photo_like = DynamicImage::ImageLuma8(GrayImage::from_fn(
        2455,
        3523,
        |x, y| image::Luma([((x + y) % 256) as u8]),
    ));
    assert_eq!(
        preprocess_with_mode(&photo_like, OcrPreprocessMode::Legacy).dimensions(),
        (1672, 2400),
    );
    assert_eq!(
        preprocess_with_mode(
            &photo_like,
            OcrPreprocessMode::PreserveNearBitonal,
        )
        .dimensions(),
        (1672, 2400),
    );
}
```

- [ ] **Step 2: Run RED**

Run:

```bash
cargo test -p fileconv-core image_ocr::tests::effectively_bitonal -- --nocapture
cargo test -p fileconv-core image_ocr::tests::near_bitonal_mode -- --nocapture
```

Expected: compile failure because the enum and functions do not exist.

- [ ] **Step 3: Implement bounded deterministic classification**

Use frozen constants:

```rust
const BITONAL_DARK_MAX: u8 = 32;
const BITONAL_LIGHT_MIN: u8 = 223;
const BITONAL_EXTREME_MIN_PER_MILLE: u64 = 985;
const BITONAL_INK_MIN_PER_MILLE: u64 = 5;
const BITONAL_INK_MAX_PER_MILLE: u64 = 400;
```

Count every grayscale pixel after existing dimension/allocation checks. A page
qualifies only when at least 98.5% of pixels are at an extreme and dark-pixel
ink coverage is between 0.5% and 40%. Full traversal is bounded by existing
50-million-pixel OCR limits.

`PreserveNearBitonal` returns the grayscale image unchanged only when the long
edge exceeds `MAX_LONG_SIDE` and classification passes. Otherwise call the
unchanged legacy implementation.

- [ ] **Step 4: Make configuration explicit and default-safe**

Implement:

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OcrPreprocessMode {
    #[default]
    Legacy,
    PreserveNearBitonal,
}

#[derive(Debug, Clone, Default)]
pub struct OcrRunConfig {
    pub tesseract_binary: Option<PathBuf>,
    pub preprocess_mode: OcrPreprocessMode,
}
```

Update existing struct literals with `..OcrRunConfig::default()` and route
`ocr_dynimage_detailed()` through `preprocess_with_mode()`.

- [ ] **Step 5: Run GREEN and full core tests**

```bash
cargo test -p fileconv-core image_ocr -- --nocapture
cargo test -p fileconv-core
```

Expected: all pass; default snapshots/behavior remain unchanged.

- [ ] **Step 6: Commit and push pre-benchmark revision**

Before pushing:

```bash
cargo fmt --all -- --check
cargo metadata --locked --format-version 1 --no-deps
python3 scripts/check-dependency-policy.py
```

Commit:

```bash
git add crates/core/src/image_ocr.rs crates/core/src/lib.rs
git commit -m "feat(ocr): add opt-in bitonal render preservation"
git push -u origin cursor/vietnamese-ocr-accuracy-e533
```

### Task 2: Benchmark-only CLI selector and bounded calibration runner

**Files:**
- Modify: `crates/cli/src/main.rs`
- Create: `bench/ocr_cpu_service/experiments/bitonal_pdf.py`
- Create: `bench/ocr_cpu_service/experiments/bitonal-configs.json`
- Create: `bench/ocr_cpu_service/tests/test_bitonal_pdf.py`

**Interfaces:**
- Consumes: `FILECONV_OCR_PREPROCESS_MODE=preserve-near-bitonal`
- Produces: `CalibrationCandidate` records for four exact variants
- Produces: ignored raw artifact `.data/bitonal-pdf/calibration.json`
- Produces: `validate_calibration_artifact(payload) -> None`

- [ ] **Step 1: Write failing CLI selector tests**

Test a pure parser in `crates/cli/src/main.rs`:

```rust
#[test]
fn parses_only_supported_preprocess_modes() {
    assert_eq!(
        parse_ocr_preprocess_mode(None).unwrap(),
        OcrPreprocessMode::Legacy,
    );
    assert_eq!(
        parse_ocr_preprocess_mode(Some("preserve-near-bitonal")).unwrap(),
        OcrPreprocessMode::PreserveNearBitonal,
    );
    assert!(parse_ocr_preprocess_mode(Some("unknown")).is_err());
}
```

Run `cargo test -p fileconv-cli parses_only_supported_preprocess_modes`.
Expected: RED because parser is absent.

- [ ] **Step 2: Implement CLI-only environment selection**

Read `FILECONV_OCR_PREPROCESS_MODE` in the CLI, construct
`OcrRunConfig`, and call `Converter::with_options_and_ocr_config()`.
Invalid non-empty values fail before source conversion. Do not add the
variable to `crates/server/src/workers/sandbox.rs`; web jobs therefore cannot
enable the spike.

- [ ] **Step 3: Write failing calibration schema/security tests**

Tests must reject:

- any page other than `1..=20`, `60`, or `450`;
- a candidate list not exactly
  `baseline-system-fast`, `baseline-best`,
  `bitonal-best-vie-eng`, `bitonal-best-vie`;
- missing source/binary/tessdata/config/tool hashes;
- recognized text, reference text, stdout/stderr, or environment values;
- duplicate/missing candidate-page records;
- unbounded dimensions, timeout, output, RSS, or process-tree settings.

Example:

```python
def test_calibration_rejects_holdout_or_unapproved_pages():
    payload = valid_artifact()
    payload["records"][0]["page_number"] = 21
    with pytest.raises(ValueError, match="approved calibration pages"):
        validate_calibration_artifact(payload)
```

- [ ] **Step 4: Implement direct-argv serial runner**

The runner:

1. checksum-validates the official PDF;
2. renders approved pages once at 300 DPI into ignored temporary storage;
3. verifies each render stays within 50 million pixels/10,000 dimensions;
4. invokes release `fileconv one <png> --lang <langs>` through the existing
   bounded candidate process contract;
5. sets only environment variable names required by each candidate;
6. samples process-tree RSS every 10 ms;
7. retains text in memory only long enough to compute additive metrics;
8. cleans rendered pages after completion.

Candidate semantics:

```json
[
  {"id":"baseline-system-fast","mode":"legacy","tessdata":"system","langs":"vie+eng"},
  {"id":"baseline-best","mode":"legacy","tessdata":"best","langs":"vie+eng"},
  {"id":"bitonal-best-vie-eng","mode":"preserve-near-bitonal","tessdata":"best","langs":"vie+eng"},
  {"id":"bitonal-best-vie","mode":"preserve-near-bitonal","tessdata":"best","langs":"vie"}
]
```

- [ ] **Step 5: Implement private-reference and diagnostic metrics**

Reference path is a required runtime argument and is never serialized.
Strip page comments, Markdown decoration, and standalone page numbers before
using additive edit counts. Store only counts.

For pages 60 and 450 store:

- digit sequence count and checksum;
- legal identifier count;
- non-whitespace character count;
- suspicious-character count;
- contextual accent-proxy counts from the supplied note.

Do not label any of these CER.

- [ ] **Step 6: Run focused and full fast tests**

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m pytest \
  bench/ocr_cpu_service/tests/test_bitonal_pdf.py -q

PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m pytest \
  bench/ocr_cpu_service/tests -q
```

- [ ] **Step 7: Commit and push**

Run Rust pre-push gates, then:

```bash
git add crates/cli/src/main.rs \
  bench/ocr_cpu_service/experiments/bitonal_pdf.py \
  bench/ocr_cpu_service/experiments/bitonal-configs.json \
  bench/ocr_cpu_service/tests/test_bitonal_pdf.py
git commit -m "bench(ocr): add bounded bitonal PDF calibration"
git push -u origin cursor/vietnamese-ocr-accuracy-e533
```

### Task 3: Execute calibration and derive the winner

**Files:**
- Create: `bench/ocr_cpu_service/reports/bitonal-pdf-calibration.md`
- Modify only if evidence finds a defect:
  `bench/ocr_cpu_service/experiments/bitonal_pdf.py`
- Test only if a defect is found:
  `bench/ocr_cpu_service/tests/test_bitonal_pdf.py`

**Interfaces:**
- Consumes: release binary, PDFium, system/best tessdata, private reference
- Produces: ignored `.data/bitonal-pdf/calibration.json`
- Produces: data-derived `winner_id: str | None`

- [ ] **Step 1: Build the measured release binary**

```bash
CC=gcc CXX=g++ cargo build --release \
  -p fileconv-cli --no-default-features
```

Record binary SHA-256 and build configuration.

- [ ] **Step 2: Run all four variants on 22 pages**

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python \
  -m experiments.bitonal_pdf calibrate \
  --pdf bench/ocr_cpu_service/.data/corpus/official-89-2026-tt-btc.signed.pdf \
  --reference /home/ubuntu/.cursor/projects/workspace/uploads/Tho_ng-tu_-89-2026-TT-BTC_opencode-qwen3.7-plus_0b58.md \
  --fileconv target/release/fileconv \
  --pdfium-lib pdfium/lib \
  --system-tessdata /usr/share/tesseract-ocr/5/tessdata \
  --best-tessdata tessdata_best \
  --output bench/ocr_cpu_service/.data/bitonal-pdf/calibration.json
```

Expected cardinality: 4 × 22 = 88 successful records.

- [ ] **Step 3: Derive winner from raw counts**

The gate implementation must:

- compare pages 1–20 aggregate and per-page disagreement;
- compare page 60 accent proxies;
- compare page 450 digit/identifier/content coverage;
- enforce marker, latency, RSS, and failure constraints;
- derive winner/ties from values rather than candidate IDs.

Add a synthetic fixture test proving an improving candidate wins and a
regressing page disqualifies it before accepting live output.

- [ ] **Step 4: Generate deterministic Markdown**

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python \
  -m experiments.bitonal_pdf report \
  --input bench/ocr_cpu_service/.data/bitonal-pdf/calibration.json \
  --output bench/ocr_cpu_service/reports/bitonal-pdf-calibration.md
```

Regenerate to a temporary path and compare byte-for-byte.

- [ ] **Step 5: Review gate**

If `winner_id` is null, stop: do not run Task 4 and report the measured
reason. If a winner exists, freeze its exact configuration hash and proceed.

- [ ] **Step 6: Commit calibration evidence**

```bash
git add bench/ocr_cpu_service/reports/bitonal-pdf-calibration.md \
  bench/ocr_cpu_service/experiments/bitonal_pdf.py \
  bench/ocr_cpu_service/tests/test_bitonal_pdf.py
git commit -m "bench(ocr): measure bitonal PDF calibration"
git push -u origin cursor/vietnamese-ocr-accuracy-e533
```

### Task 4: Conditional full-document before/after comparison

**Files:**
- Create only after a passing winner:
  `bench/ocr_cpu_service/reports/bitonal-pdf-full.md`
- Preserve ignored:
  `bench/ocr_cpu_service/.data/official-89-system-fast.md`
- Create ignored:
  `bench/ocr_cpu_service/.data/official-89-after.md`

**Interfaces:**
- Consumes: frozen winning configuration/hash
- Produces: complete 839-page ignored Markdown and tracked aggregate report

- [ ] **Step 1: Verify baseline artifact**

Require 839 unique ordered page markers, successful timing log, source hash,
and baseline configuration identity. If unavailable, rerun the exact
worker-compatible baseline before continuing.

- [ ] **Step 2: Run the winning full candidate in tmux**

Use the exact winning language/tessdata/mode. Example for a
`bitonal-best-vie` winner:

```bash
FILECONV_PDFIUM_LIB="$PWD/pdfium/lib" \
FILECONV_TESSDATA="$PWD/tessdata_best" \
FILECONV_OCR_PREPROCESS_MODE=preserve-near-bitonal \
/usr/bin/time -v ./target/release/fileconv one \
  bench/ocr_cpu_service/.data/corpus/official-89-2026-tt-btc.signed.pdf \
  --lang vie \
  > bench/ocr_cpu_service/.data/official-89-after.md
```

The actual command must use direct argv execution in the tmux workflow and a
separate ignored timing log.

- [ ] **Step 3: Validate complete output**

Require:

- exit code 0;
- exactly 839 unique ordered page markers;
- final marker 839;
- non-empty bounded output;
- sampled/recorded peak RSS below 768 MiB;
- wall time at most 2× the 21m28s baseline.

- [ ] **Step 4: Generate before/after report**

Record only aggregate counts and bounded page diagnostics:

- wall time, RSS, bytes, marker coverage;
- pages 1–20 reference disagreement before/after;
- page 60 and 450 proxy changes;
- full-document accent/code/junk proxies labeled non-CER;
- exact source/binary/tessdata/config hashes;
- limitations and manual-review requirement.

- [ ] **Step 5: Run final verification**

```bash
PYTHONPATH=bench/ocr_cpu_service \
bench/ocr_cpu_service/.venv/bin/python -m pytest \
  bench/ocr_cpu_service/tests -q
python3 scripts/check-architecture-boundaries.py
python3 scripts/check-architecture-boundaries.py --self-test
cargo fmt --all -- --check
cargo metadata --locked --format-version 1 --no-deps
python3 scripts/check-dependency-policy.py
git diff --check
```

- [ ] **Step 6: Commit tracked report and push**

```bash
git add bench/ocr_cpu_service/reports/bitonal-pdf-full.md
git commit -m "bench(ocr): compare full bitonal PDF output"
git push -u origin cursor/vietnamese-ocr-accuracy-e533
```

Keep both complete Markdown files ignored and provide them to the user as
temporary download artifacts only.
