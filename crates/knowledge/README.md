# fileconv-knowledge

Pure contracts for ranking, grounding, citation and index signatures. Phase 1A moves
reusable knowledge logic here from desktop without changing desktop behaviour.

This crate may depend on `fileconv-core`; it must not depend on Tauri, axum, database,
Qdrant, MinIO or other storage/transport adapters by default.
