# Phase 1A — Tách `crates/knowledge`

## Outcome

Tách thuật toán RAG dùng chung khỏi Tauri để desktop và server dùng một nguồn sự
thật. Desktop phải giữ nguyên IPC/JSON/hành vi; chưa thêm PG/Qdrant hay port toàn bộ
intelligence.

Prerequisite: Phase F engineering foundation đã pass; crate skeleton, Rust/test/CI
conventions và dependency rules là đầu vào, không được tái định nghĩa trong 1A.

## Boundary đích

```text
fileconv-core
    ↑
fileconv-knowledge
    ↑                 ↑
fileconv-desktop      fileconv-server (Phase 1B)
```

`fileconv-knowledge` không phụ thuộc Tauri, axum, filesystem DATA root hoặc desktop.

## P1A.1 — Freeze baseline

- Ghi snapshot tên/kết quả các test trong:
  - `app/src-tauri/src/knowledge.rs`;
  - `app/src-tauri/src/vector_index.rs`;
  - `crates/core/src/intelligence_tests.rs`.
- Tạo golden fixtures cho top-k, rerank score, anchor/page và grounded answer.
- Ghi JSON fixtures cho `HybridSearchResponse`, `GroundedAnswer`, `IndexStats` để
  khóa camelCase contract với `app/src/lib/types.ts`.
- Chạy baseline desktop index → search → ask offline và LLM fallback.

## P1A.2 — Populate crate skeleton và types

Phase F đã thêm `crates/knowledge` vào workspace. Phase 1A populate skeleton:

```text
src/
├── lib.rs
├── types.rs
├── embedding.rs
├── query.rs
├── rank.rs
├── citation.rs
├── ask.rs
└── desktop/
    ├── sqlite.rs
    └── hnsw.rs
```

Types dùng chung:

- index request/result/stats;
- hybrid request/response/hit;
- source anchor;
- ask request/grounded answer;
- embedding/index metadata.

Public API trả error typed; Tauri wrapper chuyển thành `String` để không phá IPC.

## P1A.3 — Tách logic thuần

Di chuyển từ `app/src-tauri/src/knowledge.rs`:

- token normalization và local hash vector;
- FTS query escaping;
- cosine/RRF/rerank;
- snippet;
- anchor inference;
- extractive answer và grounding validation;
- embedding plan/signature validation.

`fileconv-core` tiếp tục sở hữu:

- heading-aware chunking;
- `CorpusDocument`/`build_corpus`;
- LLM/embedding HTTP clients;
- deterministic handoff/quality/PII.

Không duplicate các phần trên.

## P1A.4 — Storage desktop sau feature

Giữ SQLite + persistent HNSW cho desktop dưới feature:

- `desktop-sqlite`;
- `desktop-hnsw`.

Storage API nhận corpus/document data từ caller. Nó không gọi ngược
`desktop::load_documents`, `resolve_within` hay Tauri state. Desktop adapter chịu
trách nhiệm load filesystem và truyền dữ liệu vào.

Không đổi trong phase này:

- incremental skip semantics;
- local fallback khi provider embedding lỗi;
- dimension/signature mismatch behavior;
- index file location;
- HNSW threshold;
- frontend answer mode.

Các vấn đề perf/concurrency hiện hữu được ghi ticket riêng; không lén thay đổi trong
refactor nếu chưa có fixture chứng minh tương đương.

## P1A.5 — Tauri thin adapter

`app/src-tauri/src/knowledge.rs` chỉ còn:

- `#[tauri::command]`;
- lấy `AppState`, data root, settings;
- load tài liệu đã path-jail;
- `spawn_blocking`;
- map request/result/error.

Giữ nguyên command:

- `rebuild_knowledge_index`;
- `knowledge_index_stats`;
- `hybrid_search`;
- `hybrid_ask`.

Không xóa legacy `search_intelligence`/`ask_intelligence`; chỉ đánh dấu deprecation
nội bộ vì UI hiện dùng hybrid path.

## P1A.6 — Stable identities

Định nghĩa canonical byte encoding + BLAKE3/SHA-256 cho server document/chunk/index
identity trong crate dùng chung. Desktop có thể giữ hash cũ trong compatibility
mode để không rebuild ngoài ý muốn. Không dùng `DefaultHasher` cho dữ liệu durable
của server.

Index signature phải bao gồm:

- embedding provider/model/revision/dimension/normalization;
- chunking algorithm/version;
- text normalization version.

## Tests

- Unit: normalization, vectors, query escaping, cosine/RRF, grounding, anchors.
- Storage: SQLite incremental/persistence, scope, dimension mismatch, provider
  fallback.
- HNSW persistent round-trip.
- Serde contract fixture với TypeScript.
- Golden retrieval parity trước/sau refactor.
- Desktop manual smoke: rebuild → search → ask.

Commands:

```bash
cargo test -p fileconv-knowledge --all-features
cargo test -p fileconv-core --features llm
cargo test -p fileconv-desktop
cd app && pnpm test && pnpm build
```

## Gate

- Tất cả test cũ và test mới xanh.
- Golden top-k/citation không regression ngoài tolerance đã ghi.
- `cargo tree -p fileconv-knowledge` không có `tauri`, `axum` hay
  `fileconv-desktop`.
- Desktop IPC JSON không đổi.
- Desktop index hiện hữu mở được hoặc migration/rebuild được thông báo rõ.
- Server có thể gọi pure rank/citation API mà không kéo SQLite/HNSW.

## Completion evidence

- P1A-01…P1A-10 đã merge vào `master`.
- `make check-knowledge-extraction` pass: core/LLM/CLI, default server consumer,
  knowledge feature matrix, desktop backend, IPC production wrappers, frontend
  test/build, fixtures, boundaries và roadmap.
- Legacy SQLite và HNSW binary fixtures mở/search được; provider signature mismatch
  phát warning rebuild rõ ràng.
- GitHub-hosted jobs hiện bị từ chối trước bước đầu do billing/spending limit. Đây
  không phải test failure; rerun workflow khi hosted runner được khôi phục.

## Không thuộc phase

- PG/Qdrant/MinIO adapters.
- JWT/RBAC/job queue.
- Port handoff, quality, PII, versions, tables, watch/export — thuộc Phase 3.
