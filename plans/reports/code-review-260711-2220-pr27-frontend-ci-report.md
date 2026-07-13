# Code Review — PR #27 Frontend + CI (merge 3c6de6b)

Scope: `app/src/`, `.github/workflows/`, `scripts/`, `bench/*.py` from `git diff 3c6de6b^1 3c6de6b`.
Reviewer focus: XSS in rendered content, API-key exposure, store state bugs, TS type lies, CI correctness/security, test quality.

## Overall Assessment

Frontend is well-typed and the security-sensitive surfaces (markdown/HTML/PPTX rendering, API-key storage)
are handled correctly. No XSS and no key-in-browser-storage findings. The material risks are behavioral:
an unguarded async race in the Intelligence RAG feature that can surface a grounded answer for the wrong
document scope, module-level mutable cache shared across projects, and a per-file index rebuild inside the
convert queue. CI workflows are structurally sound with one supply-chain nit.

---

## Critical Issues

None.

---

## High Priority

### H1 — Stale async result overwrites cleared state (wrong-scope grounded answers)
`app/src/components/IntelligenceView.tsx:212-222` (`run` helper) + scope-clear effect `:167-189`.

`run()` unconditionally calls the setter on completion; it captures `selected` at call time and has no
request-id / scope guard and no `AbortController`. When the document scope changes, the effect at :167-189
clears `answer`/`hits`/`quality`/etc. But any request started under the previous scope keeps running and
repopulates state when it resolves.

Concrete failure: user selects docs A,B in "Hỏi đáp", clicks Hỏi → `hybridAsk([A,B], …, useLlm=true)`
starts (multi-second LLM call). Mid-flight the user changes the selection to doc C. The scope effect fires
and `setAnswer(null)`. The in-flight ask then resolves and runs `setAnswer(result)` (`:299`), so the UI now
shows an answer grounded in A,B — with citations (`:838-850`) that `openSource` into documents no longer in
scope — while the header shows scope C. For a feature whose entire value is *grounded/attributable* answers,
this silently presents a mis-attributed answer as trustworthy. Same class of bug affects `searchContent`,
`loadQuality`, `loadSchemas`, `scanPii`, `loadTables` (all route through `run`), though `ask` is the
highest-trust one.

Fix: capture a scope token (e.g. `scopeKey`) or an incrementing request id when `run` starts and drop the
result if it changed before resolution; ideally also abort in-flight work on scope change/unmount.

---

## Medium Priority

### M1 — Module-level mutable cache shared across all mounts and projects
`app/src/components/IntelligenceView.tsx:75-77` (`cachedHandoff`, `cachedArtifactDrafts`,
`cachedActiveArtifact`) seeded into `useState` at `:120-123`, written from effects at `:191-195`.

These are module singletons, not per-component/per-project state. On remount (including after a project
switch via `setActiveProject`, which resets `intelligenceScope` to `[]` — store.ts:190-198) the component
initializes `handoff`/`artifactDrafts` from the previous project's cached values. It self-heals only because
the scope effect at :167-175 calls `sameScope(cachedHandoff.pack.sources, selected)` and clears on mismatch.
That is fragile: the correctness of not showing Project A's draft BRD inside Project B depends entirely on
that one comparison, and unsaved `artifactDrafts` (edited handoff artifact text) persist in the module across
project switches until overwritten. Prefer a `useRef` scoped to the component, or key the cache by
project/scope.

### M2 — Per-file knowledge-index rebuild inside the convert queue
`app/src/state/store.ts:565-569`. Every completed (re)convert in the queue loop calls
`api.rebuildKnowledgeIndex([next.relPath])` sequentially. For a batch import/reconvert of N files this is N
sequential index-build IPC calls (each of which, for neural embedding providers, may issue embedding requests
per chunk). If the backend "rebuild" is not incremental this is O(N^2). Impact scales with batch size and is
invisible on the happy-path single-file test. Recommend confirming backend semantics; if non-incremental,
debounce/batch the rebuild to once after the queue drains. (Note: on index failure the job is still marked
`done` at :570-574 with only an error toast — acceptable, convert did succeed, but worth a deliberate call.)

---

## Low Priority

### L1 — CI uses a mutable action ref (supply chain)
`.github/workflows/ci.yml:22,67` and `release-desktop.yml:37` pin `dtolnay/rust-toolchain@master` — a moving
branch. A compromised/changed upstream `master` runs with repo checkout + (in release) signing secrets in
scope. Other actions are pinned to major tags (`@v4`), which is the repo's baseline; for the toolchain action
prefer a tag or commit SHA. `permissions: contents: read` and use of `pull_request` (not
`pull_request_target`) are correct — fork PRs cannot read secrets.

### L2 — Dead hostname entry in local-endpoint check
`app/src/lib/llmSettings.ts:44`. `new URL("http://[::1]:…").hostname` returns `"[::1]"` (with brackets), so
the bare `"::1"` entry in the list never matches. Harmless today (the bracketed form is present) but the
bare entry is misleading. Cosmetic.

---

## Positive Observations (risk calibration)

- `SafeMarkdown.tsx`: `rehypeRaw` is ordered *before* `rehypeSanitize` (`:20`), so raw HTML from converted
  (untrusted) docs is parsed then sanitized. Schema extension only widens `colSpan`/`rowSpan` on td/th — safe.
- `PptxPreview.tsx`: renders via React SVG/JSX with escaped text (`{shape.text}`) and no
  `dangerouslySetInnerHTML`; `href={shape.dataUrl}` comes from the trusted Rust backend. No XSS.
- `SourcePreview.tsx:427` `dangerouslySetInnerHTML` uses SheetJS `sheet_to_html`, which HTML-escapes cell
  content; this predates PR #27 (only render options changed). Low risk.
- API keys: `llmApiKey`/`embeddingApiKey` persist only via `api.setSettings` to the Rust backend
  (store.ts:457-465), never to `localStorage` (which holds only baseline markdown + project id). Input is
  `type="password"` `autoComplete="off"` (Settings.tsx:459-461). No key logging found.
- `llmSettings.test.ts` / `intelligenceUtils.test.ts` are genuine behavior tests (key preservation,
  subscription clearing the key, validation-error counts, scope reconcile) — not phantom coverage.

## Verification Notes / Unresolved

- Could not run `pnpm test` / `pnpm build` (no local `node_modules`; install blocked in this env). Confirmed
  the new imports `react-markdown`, `rehype-raw`, `rehype-sanitize`, `remark-gfm`, `vitest` and `xlsx` are
  declared in `app/package.json`, so `pnpm install --frozen-lockfile` should resolve them — assuming
  `pnpm-lock.yaml` is in sync (not verified here). Local `tsc` errors were only missing-module noise from the
  absent `node_modules`, not real type errors.
- M2 severity depends on whether backend `rebuild_knowledge_index` is incremental — not inspected (out of
  frontend scope).
