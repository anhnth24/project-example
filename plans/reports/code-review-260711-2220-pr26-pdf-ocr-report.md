# Code Review — PR #26 "Improve PDF OCR fallback quality" (merge 3073fd1)

Reviewer: staff-eng code review. Scope: `crates/core/src/conv/pdf.rs` (+983), `image_ocr.rs`,
`Cargo.toml`, docs. Focus per request: correctness, panics on untrusted PDFs, per-page perf,
resource/concurrency safety, silent-failure/wrong-or-empty output.

Reality-filter note: code-level facts below are grep/source-verified. Runtime crash/UB
predictions are labelled `[Inference]` (based on reading pdfium-render 0.9.2 source + pdfium's
documented single-thread contract), not reproduced here.

## Overall Assessment

The new fallback logic is defensively written and conservative: nearly every uncertain branch
returns `None` and falls through to the slower OCR-capable path rather than emitting guessed
content. Page attribution in the fast path is sound (per-page markers + single-page recovery,
each single-page extraction being unambiguous). Panic surface on untrusted input is well
contained (`catch_unwind` around every `pdf_inspector` entry, `?`/`ok()` on joins, no unguarded
indexing). The main risks are (1) a concurrency hazard that PR26 amplifies from rare to common,
(2) a document-wide fallback regression triggered by a single unresolvable page, and
(3) content-loss potential in the new header/footer stripper given the project's
"Vietnamese content accuracy > format" priority.

No blocking data-corruption bug found in single-threaded operation. Recommend addressing the
concurrency item before relying on parallel desktop conversions.

## Critical Issues

None that manifest in single-threaded conversion.

## High Priority

### H1 — Concurrent PDFium access is now hit on *every* PDF conversion (amplified UB risk)
`pdf.rs:364-372` (`via_pdf_inspector` now always calls `native_text_for_requested_pages`),
`pdf.rs:503-532`, `pdf.rs:849` (`via_pdfium`), thread-local at `pdf.rs:23-25`.

Before PR26, PDFium was opened only when `need_pdfium` (OCR actually required). PR26 makes
**every** PDF conversion extract native text for every requested page through PDFium
(`native_text_for_requested_pages` runs unconditionally, concurrently with the inspector).

pdfium-render 0.9.2 is compiled with the default `thread_safe` feature, but for native targets
that feature only makes the bindings `Send + Sync` via a global `OnceCell` (`pdfium.rs:50-60`);
it does **not** wrap FPDF calls in a lock (per-call `Mutex`/`RwLock` exist only in
`wasm_bindings.rs`). The underlying libpdfium is single-threaded by contract.

Concrete failure scenario: the desktop app converts through `spawn_blocking`
(`app/src-tauri/src/lib.rs:790`) on tokio's multi-threaded blocking pool. The `watch` worker is
a single thread (`watch.rs:60`, sequential loop) so watch alone is safe — but a user manually
converting a PDF while the watcher is converting a dropped PDF puts two threads inside libpdfium
document/text operations at the same time. Each thread has its own `thread_local` `Pdfium`
reusing the same global bindings, so calls into `FPDF_LoadDocument`/`FPDFText_*` race with no
serialization. `[Inference]` Expected result: process crash (loses all in-flight conversions) or
silently corrupted extracted text — the latter directly violating the Vietnamese-accuracy
priority.

This is a pre-existing latent hazard, but PR26 widens the window from "only OCR jobs" to "all
PDF jobs," making it materially more likely.

Recommendation: guard PDFium document/render/text operations with a process-global `Mutex`
(a coarse lock around the `PDFIUM.with(...)` critical sections in `native_text_for_requested_pages`,
`via_pdf_inspector`'s render loop, and `via_pdfium`), or confirm/enforce that the desktop never
runs two conversions concurrently. Note the intra-call design is already single-threaded for
PDFium (workers use only `pdf_inspector`), so a cross-call lock is sufficient.

## Medium Priority

### M1 — One unresolvable page discards the entire structured extraction (regression)
`pdf.rs:426`, `pdf.rs:469-471`.

In `via_pdf_inspector`, if a `needs_ocr` page has no trustworthy native text, OCR is
disabled/unavailable, and it has no text-layer (`has_text == false`), the code sets
`unresolved_page = true` and returns `None` for the whole document. `to_markdown` then falls to
`via_pdfium` and ultimately `extract_with_pdf_extract` (whole-doc, no page filter, no structure).

Concrete scenario: a 40-page native Word-export PDF with one embedded full-page scan, run with
`ocr_enabled == false` (or libpdfium/Tesseract absent). Old code kept the 39 good structured
pages and skipped the scan. New code throws away all 39 structured pages and re-runs `pdf-extract`
over the whole file, losing pdf-inspector's headings/tables for every page. For a page-filtered
request this instead returns a hard `Err` (`pdf.rs:84-88`).

This may be an intentional "fail loud vs silently drop a page" choice, but it degrades
otherwise-good documents. Consider emitting a placeholder for the unresolved page and keeping the
structured content for the rest.

### M2 — `strip_repeated_marginal_lines` can delete real Vietnamese content
`pdf.rs:774-837`, substring logic at `pdf.rs:826-830`.

Margin detection scans the first 5 and last 3 non-empty lines of each page (`margin_indices`,
`pdf.rs:757-767`). On text-dense pages those "first 5 lines" are real body content, not just a
header. A line is stripped if it equals a repeated candidate **or** (length ≥ 12) is a substring
relation either direction. So a body line that *contains* a repeated header substring is removed
in full, not just the header portion.

Concrete scenario: header "PHƯƠNG PHÁP LUẬN FPT CASAN" repeats on ≥60% of pages (threshold
`pdf.rs:798`). A body sentence in another page's top-5 lines reads
"Theo PHƯƠNG PHÁP LUẬN FPT CASAN, doanh nghiệp phải…". Because `normalized.contains(candidate)`
with `candidate.len ≥ 12`, the whole sentence line is dropped, losing "Theo … doanh nghiệp
phải…". Gated to ≥4 pages and 60% recurrence, so impact is bounded, but it is silent content
loss on the project's highest-priority axis. Recommend stripping only the matched header segment,
or restricting removal to exact-normalized-equality matches.

### M3 — Per-page re-extraction fan-out on marker/table failures (perf)
`pdf.rs:157-168` (`extract_fast_pages` recover loop), `pdf.rs:140` (`extract_fast_pages_once`).

When page markers are missing or a page looks malformed, recovery calls
`extract_fast_pages_once(bytes, &[page])` once **per** affected page. If pdf-inspector 0.1.3 emits
markers in an unexpected form for a large filtered request, this becomes N separate full
`process_pdf_mem_with_options` calls (one per selected page). Correctness is preserved
(single-page extraction is unambiguous), but a 50-page filtered request could silently do ~50
sequential extractions. Bounded by `selected.len()`, so not unbounded, but a latent latency cliff.

### M4 — Predecessor page downgraded to flat native text on a missing successor
`pdf.rs:311-323` (parallel path).

When a page is missing and filled from native text, the *previous* page's already-good structured
content is also replaced with flat native text (to counter pdf-inspector appending an unmarked
table page to the preceding chunk). This can strip headings/tables from a correctly-structured
predecessor that never had an appended table. Content is preserved (guarded by
`native_text_is_high_confidence`), only structure/format is lost. Acceptable under
content>format, but noted.

## Low Priority

- **L1 — Fast vs slow path header handling diverges.** Fast path sets
  `strip_headers_footers = false` (`pdf.rs:114`) and relies on `strip_repeated_marginal_lines`,
  which early-returns for <4 pages (`pdf.rs:775`). A 2–3 page filtered request therefore retains
  repeated headers that the slow path would remove. Cosmetic inconsistency, not correctness.

- **L2 — `native_text_covers_markdown` token cap of 2** (`pdf.rs:635-651`) can mask loss when
  markdown legitimately repeats a token >2×; the 90% overlap gate hides the missing repeats.
  Narrow edge case.

- **L3 — `Pdfium::default()` fallback** at `pdf.rs:982`/`989` is reached only after a
  `PdfiumLibraryBindingsAlreadyInitialized` error, which guarantees the global `BINDINGS`
  `OnceCell` is already set; `Pdfium::default()` (`pdfium.rs:528-551`) then short-circuits on the
  same `AlreadyInitialized` arm and returns a reuse struct without dlopen. Verified sound — no
  panic in this path. Flagging only because the general `Pdfium::default()` contract *can* panic
  (`bind_to_system_library().unwrap()`) if reached without prior init; not reachable here.

- **L4 — `load_pdfium`/`tessdata_dir` now walk up to 4 ancestor dirs** of cwd/exe/manifest
  (`pdf.rs:939-992`, `image_ocr.rs:98-124`). More robust discovery, but a stray
  `pdfium/`/`tessdata_best/` in an ancestor directory could be picked up unexpectedly. Low
  supply-chain/ambiguity risk in dev trees.

## Verified Non-Issues (checked, not defects)

- **Page-index attribution** (`native_pages.get(&(pm.page + 1))`, `pdf.rs:400`,`436`): `pm.page`
  is the absolute 0-based document index (old code already rendered/OCR'd via `ocr_page_at(d,
  pm.page)`), so the `+1` keyed lookup into the 1-based native map is correct.
- **Unsorted/duplicate page input**: `via_pdf_inspector_filtered_fast` bails to the slow path if
  pages aren't strictly ascending unique (`pdf.rs:248`). No misordering.
- **Scanned pages in a fast-path selection**: `finalize_fast_pages` returns `None` when any
  selected `pages_needing_ocr` page lacks high-confidence native text (`pdf.rs:176-185`), so OCR
  is not skipped — control falls to the OCR-capable slow path.
- **Panic surface on untrusted PDFs**: every `pdf_inspector` call is `catch_unwind`-wrapped;
  `selected[0]` indexing at `pdf.rs:149` is guarded by `selected.len() == 1`; thread joins use
  `.ok()`. No new unguarded panic path found.
- **`PDFIUM_INIT` mutex** (`pdf.rs:28`,`940-942`): serializes cross-thread library init, poison
  handled via `into_inner`. No deadlock (lock released before PDFium use).
- **Intra-call threading**: workers in `via_pdf_inspector_parallel_full` and the inspector thread
  in `via_pdf_inspector` touch only `pdf_inspector` (lopdf); PDFium runs on the caller thread
  only. No concurrent PDFium *within* a single conversion.

## Recommended Actions (priority order)

1. **H1** — Add a process-global mutex around PDFium document/text/render operations, or prove
   conversions never overlap across threads. This is the one item with production crash/corruption
   potential and it is now hit on every PDF, not just OCR jobs.
2. **M1** — Don't discard the whole document for one unresolvable page; keep structured output for
   resolvable pages and placeholder the rest.
3. **M2** — Tighten `strip_repeated_marginal_lines` to strip only the matched header segment (or
   exact-match only) to avoid dropping body lines that contain a header substring.
4. **M3** — Cap or batch the per-page recovery fan-out to avoid an N-extraction latency cliff.

## Metrics
- New Rust LOC reviewed: ~950 in pdf.rs (+ ~24 image_ocr.rs, +11 Cargo.toml).
- New tests: 10 unit tests added (`pdf.rs:1015-1099`) covering trust gates, malformed-table
  detection, header stripping, marker parsing, and cross-thread binding reuse. They exercise the
  pure heuristics well; none exercise the concurrency path (H1) or the whole-doc discard (M1).
- Type coverage / lint: not run (review-only, no build executed).

## Unresolved Questions
- Does the desktop frontend ever issue two convert commands (or convert + watch) concurrently in
  practice? This determines whether H1 is "will happen" vs "can happen." Needs product/UI
  confirmation.
- Is M1 (whole-doc discard on one unresolvable page) an intentional fail-loud choice, or an
  oversight? Behavior differs from pre-PR26.
