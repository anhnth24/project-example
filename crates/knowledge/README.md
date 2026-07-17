# fileconv-knowledge

Pure contracts for ranking, grounding, citation and index signatures. Phase 1A moves
reusable knowledge logic here from desktop without changing desktop behaviour.

This crate may depend on `fileconv-core`; it must not depend on Tauri, axum, database,
Qdrant, MinIO or other storage/transport adapters by default.

Modules `types`, `embedding`, `query`, `rank`, `citation`, `ask` are framework-free.
Desktop adapters are opt-in:

```bash
cargo check -p fileconv-knowledge --no-default-features
cargo check -p fileconv-knowledge --all-features
```

- `desktop-sqlite`: enables the optional SQLite dependency.
- `desktop-hnsw`: enables the optional HNSW dependency.
