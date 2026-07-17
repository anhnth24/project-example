# Phase 1A issues — Extract `fileconv-knowledge`

Parent plan: [`../../../phase-1a-knowledge-extraction.md`](../../../phase-1a-knowledge-extraction.md)

<!-- roadmap-default-status: blocked -->

Phase F exit gate đã đạt. P1A-01 được activate; các issue sau vẫn theo dependency graph.

## Dependency

```text
P1A-01 → P1A-02 → P1A-03 ─┬→ P1A-04
                            ├→ P1A-05 → P1A-06
                            ├→ P1A-07 ─┐
                            └→ P1A-08 ─┴→ P1A-09 → P1A-10
```

## P1A-01 — Freeze desktop RAG và IPC contracts

- **Status:** Done — merged to `master` via PR #184.
- **Objective:** Baseline parity trước khi move code.
- **Plan:** Inventory tests; fixtures top-k/score/snippet/anchor/answer/fallback/stats/
  incremental; canonical JSON cho 4 hybrid commands; offline + mock-provider flows.
- **Files:** `app/src-tauri/src/{knowledge,vector_index}.rs`,
  `app/src/lib/{types,ipc}.ts`, backend/frontend contract fixtures.
- **Dependencies/blocks:** Không.
- **Acceptance:** CamelCase/answer modes/warnings/tolerance được khóa; undesirable
  current behavior cũng được ghi rõ.
- **Tests:** Desktop/core/frontend tests; fixture generation deterministic.
- **Security/migration:** Synthetic content/path, không credential.
- **Out of scope:** Sửa ranking/concurrency.

## P1A-02 — Populate knowledge skeleton và enforce dependency boundaries

- **Status:** Ready — P1A-01 contract baseline đã khóa.
- **Objective:** Hoàn thiện skeleton `crates/knowledge` do F-02 tạo thành reusable
  crate có typed errors và optional desktop features.
- **Plan:** Populate modules types/embedding/query/rank/citation/ask; features
  `desktop-sqlite`, `desktop-hnsw`; mở rộng CI deny-list theo boundary F-01. Không
  tạo lại workspace member hoặc convention.
- **Files:** `Cargo.toml`, `crates/knowledge/**`, `.github/workflows/ci.yml`.
- **Dependencies/blocks:** Baseline committed.
- **Acceptance:** Build no-feature/all-feature; default tree không SQLite/HNSW; không
  Tauri/axum/desktop; API không có DATA-root.
- **Tests:** `cargo check/test/tree` feature matrix.
- **Security/migration:** Minimal dependency review.
- **Out of scope:** PG/Qdrant/server.

## P1A-03 — Shared DTO và serde contract

- **Status:** Blocked bởi P1A-01/02.
- **Objective:** Di chuyển index/search/ask types mà không đổi JSON.
- **Plan:** Index request/result/stats, hit/anchor/grounded answer/metadata; serde
  fixtures; temporary desktop re-export.
- **Files:** `crates/knowledge/src/types.rs`, serde fixtures/tests,
  `app/src/lib/types.ts`.
- **Dependencies/blocks:** Scaffold + frozen JSON.
- **Acceptance:** Canonical JSON equivalent; no desktop path/state type; TypeScript
  không cần behavior change.
- **Tests:** Rust round-trip + TS fixture tests.
- **Security/migration:** Errors không expose provider secrets.
- **Out of scope:** OpenAPI generation.

## P1A-04 — Durable identities và index signatures

- **Status:** Blocked bởi P1A-03.
- **Objective:** Deterministic server identities, desktop compatibility.
- **Plan:** Versioned length-delimited encoding; BLAKE3/SHA-256 document/chunk/index;
  signature model/revision/dim/normalize/chunk/text version; fixed vectors; legacy
  `DefaultHasher` compatibility.
- **Files:** `crates/knowledge/src/{identity,embedding}.rs`, identity fixtures.
- **Dependencies/blocks:** Shared metadata; production values tới từ Phase 0.
- **Acceptance:** Cross-platform stable; no concatenation ambiguity; server không
  dùng DefaultHasher; legacy index mở hoặc explicit rebuild.
- **Tests:** Unicode/boundary/order/version/cross-process + legacy fixture.
- **Security/migration:** Hash là identity, không phải access control; không mix version.
- **Out of scope:** Chọn model.

## P1A-05 — Query, local vectors và embedding plan

- **Status:** Blocked bởi P1A-03; tích hợp signature phụ thuộc P1A-04.
- **Objective:** Tách pure query/embedding preparation.
- **Plan:** Normalization, feature hash/vector norm, provider plan, dimension check,
  FTS escape; HTTP client vẫn ở core; giữ local fallback semantics.
- **Files:** `crates/knowledge/src/{query,embedding}.rs`, tests; source desktop module.
- **Dependencies/blocks:** Shared types.
- **Acceptance:** Output parity; query rỗng/punctuation safe; mismatch/fallback không
  đổi; không Tauri/settings/filesystem.
- **Tests:** Vietnamese/punctuation/determinism/provider mock/dim mismatch.
- **Security/migration:** Credential-bearing URL không vào signature/error.
- **Out of scope:** Async client/new tokenizer.

## P1A-06 — Rank, citation và grounded answer

- **Status:** Blocked bởi P1A-03/05.
- **Objective:** Reusable hybrid merge, anchors và grounding.
- **Plan:** Cosine/RRF/rerank/sort; snippet/page-slide-sheet anchor; extractive answer;
  citation validator; separate LLM calls.
- **Files:** `crates/knowledge/src/{rank,citation,ask}.rs`, golden tests.
- **Dependencies/blocks:** DTO/query.
- **Acceptance:** Top-k/citation/answer parity trong tolerance; invented citation
  fallback; server caller không kéo desktop features.
- **Tests:** Tie/NaN/overlap/anchor/snippet/grounding/golden.
- **Security/migration:** Untrusted passages không thành instruction.
- **Out of scope:** Learned reranker/streaming.

## P1A-07 — SQLite desktop storage feature

- **Status:** Blocked bởi P1A-03…06.
- **Objective:** Move SQLite persistence, bỏ reverse dependency vào Tauri.
- **Plan:** Schema/metadata/vector/incremental/FTS/hydration; API nhận DB path +
  caller-supplied corpus; Tauri giữ path jail/load.
- **Files:** `crates/knowledge/src/desktop/sqlite.rs`, legacy DB fixture,
  `app/src-tauri/src/{knowledge,intelligence}.rs`.
- **Dependencies/blocks:** Shared APIs stable.
- **Acceptance:** Legacy DB parity; incremental/scope/signature/fallback giữ nguyên;
  không gọi data_root/load_documents/resolve_within; optional rusqlite.
- **Tests:** Empty/legacy/changed/scope/corrupt-dim/persistence.
- **Security/migration:** Caller chịu path jail; schema additive hoặc explicit rebuild.
- **Out of scope:** PostgreSQL/perf redesign.

## P1A-08 — Persistent HNSW desktop feature

- **Status:** Blocked bởi P1A-02/04/05.
- **Objective:** Move optional ANN cache, SQLite vẫn authority.
- **Plan:** Manifest/partition/rebuild/search/clear; legacy signature compatibility;
  corrupt/mismatch fallback exact cosine.
- **Files:** `crates/knowledge/src/desktop/hnsw.rs`, legacy HNSW fixture,
  source `vector_index.rs`.
- **Dependencies/blocks:** Feature scaffold + vectors/identity.
- **Acceptance:** Round-trip parity; corruption không mất data; location/threshold
  không đổi; `hnsw_rs` optional.
- **Tests:** Corrupt/count/signature/atomic replacement/feature matrix.
- **Security/migration:** Validate manifest bounds/path.
- **Out of scope:** HNSW tuning/Qdrant.

## P1A-09 — Thin Tauri adapters

- **Status:** Blocked bởi P1A-06/07/08.
- **Objective:** Desktop commands delegate shared crate, IPC giữ nguyên.
- **Plan:** Tauri giữ state/settings/path load/spawn_blocking/error mapping; delegate
  rebuild/stats/search/ask; retain legacy commands; remove duplicate only sau parity.
- **Files:** `app/src-tauri/src/{knowledge,vector_index,intelligence,lib}.rs`,
  Cargo manifests.
- **Dependencies/blocks:** Pure logic + stores.
- **Acceptance:** Command/payload/result unchanged; source adapter mỏng; legacy index
  behavior documented; no duplicate algorithm.
- **Tests:** Backend/frontend contract + manual rebuild/search/offline/LLM fallback.
- **Security/migration:** Path jail và secret-safe errors giữ ở desktop.
- **Out of scope:** UI/IPC rename/async redesign.

## P1A-10 — CI parity và extraction gate

- **Status:** Blocked bởi P1A-09.
- **Objective:** Chứng minh desktop equivalence và server usability.
- **Plan:** Full feature/contract/golden matrix; no-feature server consumer test;
  dependency deny-list; docs compatibility; file perf/concurrency defects riêng.
- **Files:** CI, `crates/knowledge/tests/`, desktop integration tests,
  architecture/compatibility docs.
- **Dependencies/blocks:** Adapter cutover.
- **Acceptance:** Tất cả test xanh; golden trong tolerance; IPC unchanged; legacy
  index path tested; server consumer không desktop deps.
- **Tests:** `cargo test` core/knowledge/desktop, `cargo tree`, `pnpm test/build`.
- **Security/migration:** Synthetic fixtures; explicit index rebuild notice.
- **Out of scope:** Server/storage/auth.

## Exit gate

P1A-10 chỉ đóng khi desktop parity, IPC contract, legacy index handling, optional
dependency boundaries và pure server-consumer test đều đạt.
