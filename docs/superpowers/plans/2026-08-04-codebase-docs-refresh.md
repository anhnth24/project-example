# Codebase Documentation Refresh Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refresh the five living codebase documents so they accurately describe the current workspace, architecture, product surfaces, conventions, and local Web workflow.

**Architecture:** Treat manifests, source registries, OpenAPI, Make targets, scripts, and tests as evidence. Update the codebase map and architecture first, then align product/convention wording, and finally repair the local-development flow. Preserve historical records and avoid volatile LOC/file counts.

**Tech Stack:** Markdown, Rust workspace manifests and source, Tauri 2, React/Vite, Axum HTTP/SSE, OpenAPI, PostgreSQL/Qdrant/MinIO development stack.

## Global Constraints

- Modify only `docs/codebase-summary.md`, `docs/system-architecture.md`, `docs/project-overview-pdr.md`, `docs/code-standards.md`, and `docs/runbooks/local-development.md`.
- Do not modify roadmap, issue catalogs, ADRs, journals, historical specs/plans, benchmark reports, generated roadmap, or PostgreSQL ERD.
- Preserve project-owned converter boundaries, intentional pins/fallbacks, GNU native linking, thread-local PDFium, and bounded process-wide Whisper caches.
- Distinguish code present on `master`, runtime dependencies, and production/release evidence.
- Do not add LOC or tracked-file counts.
- Do not introduce new benchmark, production-readiness, or release claims.

---

### Task 1: Refresh the codebase map and system architecture

**Files:**
- Modify: `docs/codebase-summary.md`
- Modify: `docs/system-architecture.md`

**Interfaces:**
- Consumes: workspace membership from `Cargo.toml`; converter contracts from `crates/core/src/lib.rs`; CLI registry from `crates/cli/src/main.rs::registered_commands`; MCP tool handlers from `crates/mcp/src/main.rs`; Tauri handler registry from `app/src-tauri/src/lib.rs`; server routes from `crates/server/src/http.rs`; Web pages from `web/src/pages/`.
- Produces: the canonical navigation map and high-level architecture used by Tasks 2 and 3.

- [ ] **Step 1: Record the stale inventory before editing**

Run:

```bash
rg -n 'Một lõi, ba giao diện|~5 669|18 IPC|56 command|8 tool|title: None|python3' \
  docs/codebase-summary.md docs/system-architecture.md
```

Expected: matches for the obsolete three-surface description, LOC table, IPC/MCP
counts, fixed-empty title, or PPTX Python path.

- [ ] **Step 2: Replace the top-level codebase map**

In `docs/codebase-summary.md`, replace the tree and “Một lõi, ba giao diện” text with
a stable map containing these responsibilities:

```text
crates/core/       shared conversion engine and detailed diagnostics
crates/cli/        conversion CLI, handoff and benchmark commands
crates/mcp/        stdio MCP surface, deterministic and optional LLM tools
crates/knowledge/  framework-free retrieval/grounding contracts plus opt-in desktop adapters
crates/server/     Markhand Web API, auth, storage adapters and workers
app/               Markhand Tauri desktop
web/               browser-only React/Vite SPA over HTTP/SSE
deploy/            local and POC infrastructure, scripts and observability
bench/             reproducible converter/Web evaluation assets and reports
plans/             authoritative Markhand Web issue catalog and evidence
vendor/            reference-only markitdown-rs, excluded from the workspace
```

State explicitly that CLI, desktop and MCP share `fileconv-core`, while Markhand Web
uses the server/worker boundary and shared `fileconv-knowledge` contracts. Remove the
entire LOC/files table.

- [ ] **Step 3: Update module navigation tables**

Rewrite the tables in `docs/codebase-summary.md` to include:

- core routing and detailed conversion in `src/lib.rs`;
- split PDF implementation under `src/conv/pdf/`;
- `text.rs`, `diagnostics.rs`, `embedding_runtime.rs`, `intelligence.rs` and
  `llm_cli.rs`;
- all eight CLI commands from `registered_commands()`:
  `speed`, `accuracy`, `audio`, `one`, `one-detailed`, `handoff`,
  `pptx-preview`, `info`;
- all nine MCP tools, including `convert_to_markdown_detailed`;
- knowledge contracts and `desktop/{sqlite,hnsw,service}.rs`;
- server route/service/repository-storage/worker boundaries;
- desktop Home, Library, Document, Intelligence, project and conversion queue areas;
- Web login, library/upload, Q&A, graph and admin areas.

Do not state numeric IPC totals. Describe command groups and point to
`tauri::generate_handler!` as the registry.

- [ ] **Step 4: Replace the architecture overview**

In `docs/system-architecture.md`, replace the three-layer diagram with two connected
paths:

```text
CLI / Desktop / MCP
        │
        ▼
fileconv-core ───────► native PDF/OCR/audio runtimes

Browser SPA ──HTTP/SSE──► fileconv-server
                              ├── auth/services/repositories
                              ├── PostgreSQL/Qdrant/MinIO
                              ├── fileconv-knowledge
                              └── workers ──► fileconv CLI/core
```

Clarify that Compose is needed for Markhand Web development but not for standalone
CLI/desktop work.

- [ ] **Step 5: Synchronize core, CLI, MCP and desktop facts**

Apply these exact corrections in `docs/system-architecture.md`:

- add `Text` to `FormatKind`;
- describe `convert_path()` as the compatibility API over
  `convert_path_detailed()`;
- describe title derivation as first Markdown heading, falling back to file stem;
- point PDFium cache/lock details to `crates/core/src/conv/pdf/pdfium.rs`;
- list the eight CLI commands from Step 3 and state that PPTX metadata comes from
  `fileconv_core::probe`;
- list nine MCP tools by adding `convert_to_markdown_detailed` to the deterministic
  group;
- remove the `18`, `56`, or other brittle Tauri command count and describe the command
  groups registered by `generate_handler!`;
- add `opener:allow-open-path` to the capability list;
- mention `reconvert_detailed` alongside legacy `reconvert`.

- [ ] **Step 6: Verify Task 1 and commit**

Run:

```bash
rg -n 'Một lõi, ba giao diện|~5 669|18 IPC|56 command|8 tool|title: None|python3' \
  docs/codebase-summary.md docs/system-architecture.md
git diff --check
```

Expected: no stale-claim matches; `git diff --check` exits 0.

Then:

```bash
git add docs/codebase-summary.md docs/system-architecture.md
git commit -m "docs: refresh codebase map and architecture"
```

### Task 2: Align product scope and code conventions

**Files:**
- Modify: `docs/project-overview-pdr.md`
- Modify: `docs/code-standards.md`

**Interfaces:**
- Consumes: architecture terms established in Task 1; feature definitions in
  `crates/core/Cargo.toml`; workspace `sha2` pin in `Cargo.toml`; OCR temp handling in
  `crates/core/src/image_ocr.rs`.
- Produces: current product boundaries and contributor constraints referenced by the
  local-development runbook.

- [ ] **Step 1: Record contradictory and resolved claims**

Run:

```bash
rg -n 'Hai giao diện|installer distributable|ép light|AtomicU64|conv/pdf\.rs|shell ra `python3`' \
  docs/project-overview-pdr.md docs/code-standards.md
```

Expected: matches for obsolete product scope, installer/theme status, PDF path, OCR
temp mechanism, or resolved PPTX pitfall.

- [ ] **Step 2: Update product surfaces and boundaries**

In `docs/project-overview-pdr.md`:

- replace “Hai giao diện” with a product-surface section covering CLI, desktop, MCP,
  Markhand Web API/workers, and browser SPA;
- keep offline-first as the converter/desktop default, while describing Markhand Web
  as a separate on-prem multi-org track;
- replace the installer out-of-scope item with: Linux `.deb` has build evidence;
  Windows/macOS release still requires platform artifact verification and
  signing/notarization credentials;
- remove the light-theme claim and state that desktop currently ships the dark
  LumiBase theme;
- keep ASR streaming and high-quality handwriting OCR as out of scope/current
  limitations.

- [ ] **Step 3: Update contributor constraints**

In `docs/code-standards.md`:

- change PDF references from `crates/core/src/conv/pdf.rs` to
  `crates/core/src/conv/pdf/`, naming `pdfium.rs` for the cache/lock;
- replace the `AtomicU64` temp PNG statement with exclusive temporary files through
  `tempfile::NamedTempFile`;
- document `audio` as an opt-in feature and state that GPU/BLAS features imply it;
- add the exact workspace pin `sha2 = "=0.11.0"` and its durable-ID contract rationale;
- remove the resolved CLI PPTX/Python pitfall;
- preserve `pdf-extract = "=0.8.2"`, Symphonia 0.5, `time = "=0.3.51"`,
  `background_command`, NFC, extension-only routing, native prerequisites, and cache
  constraints unchanged.

- [ ] **Step 4: Verify Task 2 and commit**

Run:

```bash
rg -n 'Hai giao diện|installer distributable.*chưa làm|ép light|AtomicU64|conv/pdf\.rs|shell ra `python3`' \
  docs/project-overview-pdr.md docs/code-standards.md
rg -n 'Markhand Web|dark LumiBase|NamedTempFile|sha2 = \"=0\.11\.0\"|feature `audio`' \
  docs/project-overview-pdr.md docs/code-standards.md
git diff --check
```

Expected: first search has no matches; second search finds each new fact;
`git diff --check` exits 0.

Then:

```bash
git add docs/project-overview-pdr.md docs/code-standards.md
git commit -m "docs: align product scope and code conventions"
```

### Task 3: Repair the local Markhand Web development flow

**Files:**
- Modify: `docs/runbooks/local-development.md`

**Interfaces:**
- Consumes: accepted-upload enqueue behavior from
  `crates/server/src/services/upload/saga.rs`; API paths from
  `crates/server/src/api/openapi.rs`; worker kinds from `crates/server/src/workers/`;
  current Compose and seed commands already documented in the runbook.
- Produces: an executable local flow for upload, conversion, indexing, retrieval, and
  browser use.

- [ ] **Step 1: Record obsolete local status**

Run:

```bash
rg -n 'enqueue document/job API chưa đủ|Search/Q&A/web SPA.*⏳|chưa enqueue job|Route chưa có' \
  docs/runbooks/local-development.md
```

Expected: matches in the master-status table, E2E checklist and curl verification.

- [ ] **Step 2: Correct the master-status table**

Update the table to state:

- accepted upload creates a durable convert job;
- separate convert, index and embedding workers are required for pipeline completion;
- search, JSON ask, streaming ask and the browser SPA exist on `master`;
- external LLM-backed answer generation and production qualification still depend on
  their configured runtime/evidence.

Do not alter the currently accurate Compose image, port, profile, auth seed or
embedding signature tables.

- [ ] **Step 3: Rewrite the E2E pipeline checklist**

Replace the “upload does not enqueue” note with this sequence:

1. start dependencies, migrate and seed;
2. run convert, index and embedding workers with the documented environment;
3. upload an accepted file through HTTP or Web SPA;
4. inspect `GET /api/v1/jobs/{jobId}` or
   `GET /api/v1/jobs/{jobId}/events`;
5. verify retrieval through `POST /api/v1/search`;
6. verify grounded/extractive behavior through `POST /api/v1/ask`, with provider
   configuration called out only when LLM generation is enabled.

Explain that quarantined/rejected uploads do not follow the accepted conversion path.

- [ ] **Step 4: Replace the missing-route section with current curl examples**

Add request examples using the existing `$TOKEN`, organization and collection scope:

```bash
curl -sS -X POST http://127.0.0.1:8787/api/v1/search \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"query":"hello markhand","collectionIds":["55555555-5555-5555-5555-555555555501"]}'

curl -sS -X POST http://127.0.0.1:8787/api/v1/ask \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"question":"Tài liệu nói gì?","collectionIds":["55555555-5555-5555-5555-555555555501"]}'
```

Before committing, compare field names against
`crates/server/openapi/openapi.yaml`; adjust only if the schema uses different exact
property names.

- [ ] **Step 5: Verify Task 3 and commit**

Run:

```bash
rg -n 'enqueue document/job API chưa đủ|Search/Q&A/web SPA.*⏳|chưa enqueue job|Route chưa có' \
  docs/runbooks/local-development.md
rg -n '/api/v1/search|/api/v1/ask|/api/v1/jobs/\{jobId\}|accepted' \
  docs/runbooks/local-development.md
git diff --check
```

Expected: first search has no matches; second search finds the corrected flow and
current routes; `git diff --check` exits 0.

Then:

```bash
git add docs/runbooks/local-development.md
git commit -m "docs: update local web development flow"
```

### Task 4: Cross-document verification and delivery

**Files:**
- Verify: `docs/codebase-summary.md`
- Verify: `docs/system-architecture.md`
- Verify: `docs/project-overview-pdr.md`
- Verify: `docs/code-standards.md`
- Verify: `docs/runbooks/local-development.md`

**Interfaces:**
- Consumes: all documentation produced by Tasks 1–3.
- Produces: a reviewed, evidence-backed documentation-only change ready for PR review.

- [ ] **Step 1: Check scope and stale claims**

Run:

```bash
git diff master...HEAD --name-only
rg -n '18 IPC|56 command|8 tool|title: None|~5 669|ép light|Route chưa có|chưa enqueue job|shell ra `python3`' \
  docs/codebase-summary.md docs/system-architecture.md docs/project-overview-pdr.md \
  docs/code-standards.md docs/runbooks/local-development.md
```

Expected: changed files are the approved spec/plan plus the five living docs; stale
claim search returns no matches.

- [ ] **Step 2: Verify repository policy and generated roadmap stability**

Run:

```bash
python3 scripts/check-architecture-boundaries.py
python3 scripts/check-dependency-policy.py
python3 scripts/build-roadmap.py --check
git diff --check
```

Expected: all commands exit 0 and generated roadmap remains unchanged.

- [ ] **Step 3: Review factual evidence**

For every newly named command, tool, route, feature and path, use `rg` or the
corresponding manifest/OpenAPI registry to confirm it exists. Specifically confirm:

```text
8 CLI commands
9 MCP tools
FormatKind::Text
convert_path_detailed / reconvert_detailed
opener:allow-open-path
/api/v1/search, /api/v1/ask, /api/v1/ask/stream
accepted upload enqueues JobType::Convert
```

Expected: every item has a current source or contract definition; remove any claim
that cannot be verified.

- [ ] **Step 4: Commit verification fixes if needed**

If Step 1–3 required documentation corrections:

```bash
git add docs/codebase-summary.md docs/system-architecture.md \
  docs/project-overview-pdr.md docs/code-standards.md \
  docs/runbooks/local-development.md
git commit -m "docs: fix documentation verification findings"
```

Expected: a commit is created only when verification changed files.

- [ ] **Step 5: Push and update the pull request**

Run:

```bash
git push -u origin cursor/refresh-codebase-docs-37df
git status --short --branch
```

Expected: push succeeds; working tree is clean and branch tracks its origin.
