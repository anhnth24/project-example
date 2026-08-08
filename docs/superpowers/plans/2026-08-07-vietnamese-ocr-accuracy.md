# Vietnamese OCR Accuracy Improvement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce Vietnamese real-scan holdout CER by at least 20% through measured, non-generative improvements to Markhand's existing Tesseract pipeline.

**Architecture:** Expand and freeze a source-separated real-scan corpus, run one-variable CPU experiments through the generic benchmark, derive a runtime-observable selection policy, verify it on untouched holdout pages, then port only the winning bounded behavior to Rust.

**Tech Stack:** Existing Rust `fileconv-core`, Tesseract `vie+eng`, `tessdata_best`, PDFium, Python benchmark utilities, Pillow/OpenCV only in experiment tooling if license-approved.

## Global Constraints

- This plan is not executed as part of the PP-OCRv6 cleanup.
- Real-scan evidence, not synthetic fixtures, decides adoption.
- Ground truth is never available to runtime selection logic.
- No dictionary, language model, LLM, or generative post-correction may rewrite OCR text.
- Downloaded documents/images remain ignored; only license metadata, checksums, permitted ground truth, aggregate metrics, and bounded diagnostics are tracked.
- Change one experimental variable at a time before evaluating a combined policy.
- Production Rust changes begin only after a holdout winner exists.
- Before every Rust push run formatting, locked metadata, and dependency policy gates.

---

### Task 1: Expand and freeze the real-scan corpus

**Files:**
- Create: `bench/ocr_cpu_service/corpus/accuracy-sources.json`
- Create: `bench/ocr_cpu_service/corpus/accuracy-annotations.jsonl`
- Create: `bench/ocr_cpu_service/corpus/split.py`
- Create: `bench/ocr_cpu_service/tests/test_accuracy_corpus.py`
- Create: `bench/ocr_cpu_service/reports/accuracy-corpus.md`

**Interfaces:**
- Produces: at least 50 human-transcribed real scan pages
- Produces: source-document-separated `tuning` and `holdout` assignments
- Produces: named difficulty strata and immutable source/page checksums

- [ ] **Step 1: Write failing schema and leakage tests**

```python
def test_source_document_never_crosses_tuning_and_holdout():
    rows = load_accuracy_annotations(FIXTURE)
    assert no_source_overlap(rows)


def test_real_pages_have_reviewed_text_and_license():
    for row in load_accuracy_annotations(FIXTURE):
        assert row.review_status == "human-verified"
        assert row.license
        assert len(row.sha256) == 64
```

- [ ] **Step 2: Verify RED, implement schema/split validation, and verify GREEN**

Use a deterministic split keyed by source ID. Fail closed on missing transcription,
license, checksum, stratum, or duplicate source/page.

- [ ] **Step 3: Curate public sources**

Cover clean official scans, skew, low contrast, stamps/watermarks, dense forms, small
text, and old print. Keep source documents local/ignored. Have a second human review
holdout transcriptions before freezing them.

- [ ] **Step 4: Record corpus evidence**

Report counts by split, source, document type, and difficulty without publishing full
documents.

- [ ] **Step 5: Commit**

```bash
git add bench/ocr_cpu_service/corpus bench/ocr_cpu_service/tests bench/ocr_cpu_service/reports/accuracy-corpus.md
git commit -m "bench(ocr): freeze expanded Vietnamese scan corpus"
```

### Task 2: Lock the expanded baseline

**Files:**
- Create: `bench/ocr_cpu_service/experiments/baseline.json`
- Create: `bench/ocr_cpu_service/experiments/run_matrix.py`
- Create: `bench/ocr_cpu_service/tests/test_experiment_matrix.py`
- Create: `bench/ocr_cpu_service/reports/accuracy-baseline.md`

**Interfaces:**
- Consumes: frozen tuning/holdout annotations and generic candidates
- Produces: immutable baseline configuration and per-stratum aggregate metrics

- [ ] **Step 1: Write failing experiment-provenance tests**

Require source, binary, tessdata, configuration, host, and split checksums in every run.

- [ ] **Step 2: Implement baseline runner**

Run current Markhand default and `tessdata_best` serially. Store only edit counts,
timing, RSS, failures, and bounded diagnostics.

- [ ] **Step 3: Run baseline twice**

Reject the baseline if repeated aggregate CER differs beyond a documented deterministic
tolerance or candidate provenance changes.

- [ ] **Step 4: Commit**

```bash
git add bench/ocr_cpu_service/experiments bench/ocr_cpu_service/tests bench/ocr_cpu_service/reports/accuracy-baseline.md
git commit -m "bench(ocr): lock expanded real-scan baseline"
```

### Task 3: One-variable preprocessing and Tesseract experiments

**Files:**
- Create: `bench/ocr_cpu_service/experiments/preprocess.py`
- Create: `bench/ocr_cpu_service/experiments/configs.json`
- Create: `bench/ocr_cpu_service/tests/test_preprocess.py`
- Create: `bench/ocr_cpu_service/reports/accuracy-matrix.md`

**Interfaces:**
- Produces: deterministic transformed page images in ignored work storage
- Produces: one-factor configurations for DPI, deskew, threshold, denoise, PSM, tessdata

- [ ] **Step 1: Write image-invariant tests**

Test dimension/pixel bounds, deterministic output checksum, maximum rotation, and
cleanup. Ensure transforms never crop text-bearing bounds without explicit padding.

- [ ] **Step 2: Implement one transform at a time**

Order:

1. 300 vs 400 DPI;
2. bounded deskew;
3. grayscale normalization;
4. Otsu vs adaptive threshold;
5. conservative denoise/watermark suppression;
6. PSM 3/4/6/11;
7. system vs best tessdata.

- [ ] **Step 3: Run tuning matrix**

Report CER/WER and cost by stratum. Do not inspect holdout results.

- [ ] **Step 4: Commit**

```bash
git add bench/ocr_cpu_service/experiments bench/ocr_cpu_service/tests bench/ocr_cpu_service/reports/accuracy-matrix.md
git commit -m "bench(ocr): measure Tesseract preprocessing matrix"
```

### Task 4: Derive an observable retry policy

**Files:**
- Create: `bench/ocr_cpu_service/experiments/policy.py`
- Create: `bench/ocr_cpu_service/tests/test_policy.py`
- Create: `bench/ocr_cpu_service/reports/accuracy-policy.md`

**Interfaces:**
- Consumes only runtime-observable image/layout/confidence features
- Produces: bounded primary configuration plus at most one retry

- [ ] **Step 1: Write leakage and retry-bound tests**

```python
def test_policy_input_has_no_reference_text_or_error_metric():
    assert forbidden_fields(PolicyInput) == set()


def test_policy_runs_at_most_one_retry():
    assert decide_retry(low_confidence_case).attempts <= 2
```

- [ ] **Step 2: Implement the smallest explainable policy**

Prefer a single global winner. Add conditional retry only when tuning evidence shows a
stable stratum benefit and the trigger is available before ground-truth evaluation.

- [ ] **Step 3: Freeze policy before holdout**

Write its exact thresholds and configuration checksum. Do not adjust after viewing
holdout results.

- [ ] **Step 4: Commit**

```bash
git add bench/ocr_cpu_service/experiments bench/ocr_cpu_service/tests bench/ocr_cpu_service/reports/accuracy-policy.md
git commit -m "bench(ocr): freeze observable OCR retry policy"
```

### Task 5: Holdout gate

**Files:**
- Create: `bench/ocr_cpu_service/reports/accuracy-holdout.json`
- Create: `bench/ocr_cpu_service/reports/accuracy-holdout.md`
- Modify: `bench/ocr_cpu_service/benchmark/report.py`
- Modify: `bench/ocr_cpu_service/tests/test_report.py`

**Interfaces:**
- Produces: raw-derived PASS/STOP against frozen baseline and policy

- [ ] **Step 1: Add gate tests**

Verify 20% relative CER improvement, 9.5% comparable absolute target, maximum two-point
stratum regression, 2x latency bounds, and zero new failures.

- [ ] **Step 2: Run holdout once**

Run baseline and frozen policy on identical pages and host. Regenerate Markdown only
from raw additive counts.

- [ ] **Step 3: Stop or proceed**

On STOP, retain evidence and do not modify Rust. On PASS, proceed to Task 6.

- [ ] **Step 4: Commit**

```bash
git add bench/ocr_cpu_service/reports bench/ocr_cpu_service/benchmark bench/ocr_cpu_service/tests
git commit -m "bench(ocr): evaluate frozen Vietnamese OCR holdout"
```

### Task 6: Port a passing policy to Rust

**Files:**
- Modify only after PASS: `crates/core/src/image_ocr.rs`
- Test: existing inline tests in `crates/core/src/image_ocr.rs`
- Modify: `bench/REPORT_ACCURACY.md`

**Interfaces:**
- Consumes: exact frozen policy thresholds/configuration
- Produces: bounded Rust preprocessing/retry behavior used by image and PDF OCR

- [ ] **Step 1: Write failing Rust regression tests**

Add tests for each observable policy branch, image bounds, maximum attempts, and
deterministic preprocessing.

- [ ] **Step 2: Run RED**

Run the narrow `fileconv-core` OCR tests and confirm failures are due to missing policy.

- [ ] **Step 3: Implement the minimal passing behavior**

Do not port unused experiment options or Python-specific abstractions.

- [ ] **Step 4: Run Rust and corpus verification**

Run focused core tests, full matching Rust gates, then rerun the frozen holdout through
the release binary. The Rust result must meet the same gate as the experimental winner.

- [ ] **Step 5: Commit and request required review**

```bash
git add crates/core/src/image_ocr.rs bench/REPORT_ACCURACY.md
git commit -m "feat(ocr): improve Vietnamese scan recognition"
```

The change affects OCR/native-runtime behavior and requires dependency/native review if
any runtime or model dependency changes.
