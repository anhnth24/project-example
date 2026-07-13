# Code Review — PR #27 Tauri Desktop Backend

Scope: merge `3c6de6b`, diff `git diff 3c6de6b^1 3c6de6b -- app/src-tauri/` (icons excluded).
Files: `knowledge.rs` (1672), `intelligence.rs` (1168), `projects.rs` (525), `watch.rs` (347),
`vector_index.rs` (187), `lib.rs` (+327), `tauri.conf.json`, platform confs, `Entitlements.plist`, `Cargo.toml`.
Reviewed final file state, not just hunks. No code changed.

## Overall Assessment

Solid, defensively written backend. The core trust-boundary primitive `resolve_within`
(lib.rs:314) is correct: it rejects `..`, absolute paths, and per-component symlinks, and is
well tested. All SQLite access is parameterized; the FTS5 query builder tokenizes and quotes
input, closing the obvious injection vector. API keys are deliberately kept out of disk
persistence. CSP is reasonably tight (`script-src 'self'`, no `unsafe-inline` for scripts),
which materially reduces the content-injection surface. The watcher's anti-loop guard exists
and is double-enforced.

No defect I found is unambiguously Critical. The most important items are (1) a set of
commands that intentionally read/write **arbitrary absolute paths outside DATA** on behalf of
webview JS, which are only as safe as the CSP that gates content injection, and (2) a concrete
HNSW temp-directory collision under concurrent indexing. Details below.

## Critical Issues

None confirmed. The arbitrary-abs-path commands (High-1) become a Critical-class primitive
only if content injection defeats the CSP; on their own they are intended behavior.

## High Priority

### H-1. Webview can read/write/relocate arbitrary absolute paths outside DATA
`resolve_within` sandboxes every *relative* path command, but several commands take raw
absolute paths straight from the webview and bypass the sandbox entirely:

- Arbitrary **write** of attacker-controlled bytes:
  - `export_markdown_table` (intelligence.rs:745) writes CSV bytes to `req.output_abs` with
    **no extension or location check at all**.
  - `export_knowledge_pack` (intelligence.rs:1019) and `export_existing_handoff`
    (intelligence.rs:520) write a zip to `output_abs` (only `.zip` suffix enforced).
- Arbitrary **read**: `import_file` / `import_file_only` (lib.rs:738, 768) and
  `import_local_folder` (projects.rs:349) copy from any `source_abs`.
- Arbitrary **relocation of the whole sandbox**: `set_data_root` (lib.rs:575) points DATA at
  any directory, after which every relative command operates there. Note it also silently
  re-canonicalizes and persists, so a single call permanently repoints the app.

Failure scenario: a converted document rendered in the webview carries a payload that reaches
JS execution (a frontend regression, a vulnerable render dependency, or any future CSP
relaxation). That JS calls `set_data_root("C:\\Users\\<user>")` then `read_bytes(...)` to
exfiltrate arbitrary user files, or `export_markdown_table` to drop an executable-content file
into a Startup folder. These commands perform no verification that the path originated from a
native dialog.

This is partly inherent to Tauri's model and is mitigated today by the CSP, so I am not
calling it Critical — but the backend should treat webview-supplied absolute paths as
untrusted. At minimum: give `export_markdown_table` the same `.csv`/extension guard its
siblings have (it currently has none), and consider funnelling all `*_abs` writes through the
dialog plugin's returned handle rather than a free-form string. Document the accepted residual
risk for `set_data_root`/import if you keep them as-is.

## Medium Priority

### M-1. HNSW temp/backup directory name collides under concurrent indexing
`vector_index::rebuild` (vector_index.rs:64, 101) names its scratch dirs
`.{partition}.{process_id}.tmp` / `.old` — keyed on PID only, with no thread/uniqueness
counter. `rebuild_knowledge_index` and `hybrid_search` both run indexing on the
`spawn_blocking` threadpool, so two overlapping builds in one process compute the *same* temp
path. One thread's `remove_dir_all(&temporary)` (line 69) can wipe the other's in-flight
`file_dump`, and the final `rename(&temporary, &directory)` (line 110) races. Result: failed
or corrupt HNSW build. It is non-fatal (callers fall back to exact cosine and surface a
warning), which is why this is Medium not High, but it wastes work and can leave `.tmp`/`.old`
garbage. Fix: add an atomic counter (as `atomic_write` already does in lib.rs:232) to the temp
names.

### M-2. Read-path search mutates the index and re-runs on every scoped query
`hybrid_search` → `hybrid_search_inner` (knowledge.rs:783) calls `index_documents_inner`
whenever `source_rels` is non-empty, i.e. a *search* opens the DB for writing, re-hashes every
scoped document, and may rebuild the HNSW. Consequences:
- Concurrent searches become concurrent SQLite writers (serialized by the 5s busy-timeout, but
  surprising) and feed directly into the M-1 temp collision.
- For any corpus below `MIN_HNSW_POINTS` (128, vector_index.rs:12), `is_available` is always
  false, so `index_documents_with_plan` re-enters `load_vector_points` + `rebuild` (which then
  `remove_dir_all`s and returns false) on **every** search (knowledge.rs:634-655).
Steady state for a large, unchanged corpus is fine (indexed==0 and HNSW available → skipped),
but the coupling of write-indexing into the read path is a latent perf/concurrency smell.

### M-3. Full-corpus vector load into memory per search, scope filtered in Rust
`load_all_chunks` (knowledge.rs:693) scans **all** chunk rows and deserializes every vector on
each search, applying the `scope` filter row-by-row in Rust rather than in SQL. At the
`MAX_VECTOR_CANDIDATES` ceiling (100k × 256 f32 ≈ 100 MB) each query allocates ~100 MB even
when the scope is a single small document. Push `doc_rel IN (...)` into the SQL `WHERE` when a
scope is provided.

### M-4. Unbounded version-snapshot growth
`snapshot_existing_version` (intelligence.rs:780) writes a new `.md` under
`.markhand/versions/<key>/` on every `reconvert` (lib.rs:825) and every
`update_markdown_table` (intelligence.rs:734), with no pruning or count cap. Long-lived
projects accumulate snapshots without bound (disk pressure inside DATA). Consider a retention
cap.

## Low Priority

### L-1. Internal error strings (including absolute paths) returned to the webview
`es` (lib.rs:228) stringifies underlying IO/serde errors straight to the frontend, e.g.
`fs::read` failures embed full filesystem paths. Same trust domain, so low impact, but if the
frontend logs or ships errors to telemetry this leaks local paths. Consider a generic message
for filesystem errors.

### L-2. `opener:allow-open-path` capability is unscoped
`capabilities/default.json` grants `opener:allow-open-path` with no path scope. Combined with
`resolve_path` (returns any absolute path inside DATA), the webview can ask the OS to open any
DATA-relative file with its default handler. Low because it is bounded to DATA and requires the
OS handler, but a scope on the capability would tighten it.

### L-3. macOS entitlements enable JIT + unsigned executable memory
`Entitlements.plist` sets `allow-jit` and `allow-unsigned-executable-memory`. If these are only
needed for a specific bundled dependency (e.g. a JS engine / whisper), scope them out if
possible; otherwise document why they are required, as they weaken the hardened runtime.

## Edge Cases Checked

- Path traversal via `..`, absolute, and symlink components — blocked and tested
  (lib.rs:1112, intelligence.rs:1151).
- FTS5 injection via punctuation/operators — neutralized by `fts_query` tokenize+quote,
  tested (knowledge.rs:1585).
- Version-id traversal — `valid_version_id` allowlist, tested (intelligence.rs:1114).
- Handoff `out_rel_dir` escape — `load_persisted_pack` requires the `.markhand/handoff/` prefix
  and re-resolves (intelligence.rs:492); tested (intelligence.rs:1122).
- Watcher import loop — rejected inside DATA in both `configure_watcher` (watch.rs:130) and
  `process_path` (watch.rs:183); tested (watch.rs:326).
- Mixed vector dimensions / stale HNSW — dimension mismatch is a hard error
  (knowledge.rs:727); stale HNSW ids are filtered by `scoped_ids`/`by_id` so they degrade
  recall rather than corrupt results.
- API keys — stripped before persistence (lib.rs:988) and not echoed in connection-test
  responses; tested (lib.rs:1270).
- Watcher thread lifecycle — `Drop` sends Shutdown and joins (watch.rs:91); no obvious leak.
- Import symlink escape — `collect_files` skips symlinks (projects.rs:319); source dir
  canonicalized and rejected if inside DATA (projects.rs:365).

## Positive Observations (risk-relevant)

- `resolve_within` re-checks symlinks on every pushed component, not just the final path —
  correctly defeats mid-path symlink swaps.
- `atomic_write` already uses a same-dir temp + atomic replace with a per-write atomic counter;
  M-1 is simply the HNSW path not reusing that pattern.
- Keeping secrets out of `settings.json` (in-memory only) is the right call and is asserted in
  tests.

## Recommended Actions (priority order)

1. Add extension/location handling to `export_markdown_table` and treat all webview-supplied
   `*_abs` paths as untrusted; document accepted residual risk for `set_data_root`/import (H-1).
2. Add a uniqueness counter to HNSW temp/backup dir names (M-1).
3. Decouple index writes from the search read path, or at least gate the sub-128-point rebuild
   so searches don't rebuild every time (M-2).
4. Push scope filtering into SQL and/or cap `load_all_chunks` memory (M-3).
5. Add version-snapshot retention (M-4).
6. Sanitize filesystem error strings; scope the opener capability; review mac entitlements
   (L-1..L-3).

## Metrics

- Type safety: idiomatic Rust, no `unsafe`, no `unwrap` on command paths (unwraps confined to
  tests). Errors are `Result<_, String>` throughout — type-safe but loses structure (L-1).
- Test coverage: strong for path/traversal/FTS/version/watch guards and index incrementality;
  gaps around concurrent indexing (M-1/M-2) and large-corpus memory (M-3).
- Lint/build: not run in this review (read-only). Recommend `cargo clippy -p fileconv-desktop`
  before landing follow-ups.

## Unresolved Questions

- Are the `*_abs` export/import paths always sourced from the dialog plugin in the frontend? If
  so, H-1's residual risk is smaller; the backend still shouldn't assume it.
- Is `set_data_root` intended to accept any location including outside the user profile, or
  should it be constrained to a chosen workspace parent?
- Are the macOS JIT/unsigned-memory entitlements required by a specific bundled dependency?
