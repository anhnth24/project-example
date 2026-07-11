# Markhand Document Intelligence Suite — implementation plan

## Goal

Turn Markhand from a file converter into a local-first Document QA and BA/PM
handoff workbench. The existing convert/edit/compare flow remains unchanged;
all intelligence features are additive and operate on Markdown sidecars inside
the selected DATA root.

The primary workflow is:

1. Select converted source documents.
2. Generate a citation-backed BRD/PRD handoff pack.
3. Review quality, assumptions, PII, traceability and test coverage.
4. Edit/approve the generated Markdown.
5. Export a portable ZIP knowledge pack.

## Product invariants

- Filesystem remains the source of truth.
- `Converter::convert_path()` and existing CLI/MCP contracts remain compatible.
- Deterministic offline output always works without API keys.
- LLM enhancement is optional and uses existing `FILECONV_LLM_*` configuration.
- Generated factual requirements must cite source excerpts; missing information
  becomes an assumption or open question rather than invented content.
- All filesystem paths remain confined by the existing Tauri path jail.
- Heavy operations run in `spawn_blocking`; no fake progress percentages.

## Core modules

Add `fileconv_core::intelligence` with these sub-capabilities:

### Corpus and citations

- Aggregate one or more Markdown documents.
- Split by existing heading-aware chunks.
- Assign stable citation IDs, source paths, heading paths and byte offsets.
- Preserve Vietnamese NFC.

### BRD/PRD handoff pack

Generate:

- `00-README.md`
- `01-BRD.md`
- `02-PRD.md`
- `03-USER-STORIES.md`
- `04-ACCEPTANCE-CRITERIA.md`
- `05-GLOSSARY.md`
- `06-TEST-CASES.md`
- `07-TRACEABILITY.md`
- `08-ASSUMPTIONS-QUESTIONS.md`
- `manifest.json`
- `validation.json`

Deterministic extraction recognizes:

- requirements: `phải`, `cần`, `bắt buộc`, `không được`, `yêu cầu`;
- explicit user stories;
- Given/When/Then and Vietnamese equivalents;
- assumptions, TBDs and open questions;
- heading terms and definition patterns for glossary;
- Markdown tables and field-like `label: value` lines.

Stable IDs use ordered prefixes (`BR-001`, `FR-001`, `US-001`, `AC-001`,
`TC-001`) and every row carries citation IDs.

Optional LLM mode enhances the deterministic draft with a strict prompt:
no uncited facts, Vietnamese output, preserve IDs/citations, and return
Markdown only. If LLM configuration is absent or fails, the deterministic pack
is retained.

### Quality report

Per document/block:

- empty or very short content;
- OCR markers and low-confidence OCR;
- replacement/private-use characters;
- long repeated character runs;
- malformed Markdown tables;
- recurring headers;
- source/Markdown size and quality score.

Each issue includes a severity, citation/offset and recommended reprocess action.

### Search and cited Q&A

- Persist heading chunks in SQLite FTS5 with vector metadata.
- Accent-insensitive Vietnamese tokenization plus local hash fallback.
- Optional neural embeddings through local/cloud providers; model signature and
  dimensions are hard invariants.
- Rank lexical/vector legs with Reciprocal Rank Fusion, token and heading match.
- Return snippets and citations.
- Offline Q&A is extractive: top cited passages form the answer.
- Optional LLM may summarize only the retrieved passages.

### Versions, diff and merge

- Persist snapshots under `DATA/.markhand/versions/<document-key>/`.
- List/read versions and produce line-level diff hunks.
- Three-way merge preserves unchanged user edits and reports conflicts.
- Source hash/mtime create a changelog entry.

### Tables and schema

- Parse Markdown tables into editable 2D arrays with byte spans.
- Patch edited rows back into Markdown.
- Export CSV.
- Infer field types (`string`, `number`, `date`, `boolean`) and form-like fields.

### PII

Detect and optionally redact:

- email;
- Vietnamese phone numbers;
- CCCD/CMND near identity keywords;
- bank account-like values near banking keywords.

Redaction writes a new artifact and never mutates the source or canonical
Markdown without explicit approval.

### Automation and hard OCR

- Persist watch rules under DATA `.markhand/watch-rules.json`.
- Manual/polling scan reports new/modified matching files; the frontend can add
  them to the existing serial conversion queue.
- Per-block reprocess hooks expose native reconvert, Tesseract OCR and optional
  vision-LLM (`ocr_hard`) actions. The deterministic release records unsupported
  hooks rather than pretending they ran.

### Knowledge-pack export

Use the existing `zip` crate to export Markdown, citations, quality, schema,
PII report, versions and handoff artifacts with a JSON manifest.

## Persistence

```
DATA/.markhand/
├── handoff/<pack-id>/
├── versions/<document-key>/
├── knowledge.sqlite
├── watch-rules.json
└── watch-state.json
```

Writes use temp-file + rename semantics. Metadata paths derive only from
validated relative DATA paths.

## Tauri commands

- `generate_handoff_pack`
- `read_handoff_artifact`
- `search_intelligence`
- `ask_intelligence`
- `run_quality_report`
- `scan_pii`
- `redact_pii`
- `extract_document_schema`
- `list_markdown_tables`
- `update_markdown_table`
- `list_document_versions`
- `snapshot_document_version`
- `diff_document_versions`
- `merge_document_versions`
- `get_watch_rules`
- `set_watch_rules`
- `scan_watch_rules`
- `export_knowledge_pack`
- `rebuild_knowledge_index` / `knowledge_index_stats`
- `hybrid_search` / `hybrid_ask`
- subscription CLI status/login and embedding presets/test

## Desktop UI

Add a fourth top-level `intelligence` view to the icon rail.

The workspace uses progressive-disclosure pills:

1. Bàn giao
2. Chất lượng
3. Hỏi đáp
4. Phiên bản
5. Bảng
6. PII
7. Xuất
8. Theo dõi

BRD/PRD is the default. It has:

- product name and output mode;
- corpus checklist;
- generate action;
- validation summary;
- artifact tabs;
- editable Markdown preview;
- save/open/export actions.

Other tools reuse the same selected corpus and show inline empty/loading/error
states. The existing Workbench remains the place for source comparison.

## Delivery slices

1. Core types, corpus, citations, quality, search and tests.
2. Handoff generation, validation and ZIP export.
3. PII, schema, tables, versions/diff/merge and watch rules.
4. Tauri persistence and commands.
5. Intelligence React workspace and store wiring.
6. Optional LLM enhancement and hard-OCR hooks.
7. Unit tests, frontend build, Tauri sandbox workflow and reports.

## Acceptance

- Generate a complete deterministic BRD/PRD pack from at least one converted
  document without network access.
- Every extracted requirement links to a valid citation.
- Empty input creates open questions, not fabricated requirements.
- Search answers contain clickable source citations.
- PII redaction never changes canonical Markdown.
- Table edits round-trip without affecting surrounding text.
- Version diff and merge preserve unrelated edits.
- Watch scans never import outside configured paths.
- Exported ZIP contains manifest plus all selected artifacts.
- Existing convert, compare, save, queue, CLI and MCP tests continue to pass.
