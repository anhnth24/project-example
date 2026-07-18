# Knowledge index compatibility

## Stable desktop locations

- SQLite authority: `<DATA>/.markhand/knowledge.sqlite`
- HNSW cache: `<DATA>/.markhand/vector-index/<legacy-partition>`
- Existing `local_hash_v1` indexes retain their legacy signature and partition hash.

Tauri continues to path-jail `<DATA>/.markhand` before passing the database path and
ANN root to `fileconv-knowledge`. Shared adapters never discover the data root.

## Migration behavior

- SQLite opens legacy databases additively. Missing `documents.embedding_signature`
  and `chunks.vector_dims` columns are added without deleting rows.
- Legacy local vectors remain readable without a forced rebuild.
- Provider signatures now include model, dimensions, normalization, revision, and a
  credential-free deployment identity. An incompatible provider signature triggers
  one atomic SQLite rebuild and the response warning:
  `Embedding signature thay đổi; đã rebuild knowledge index tương thích.`
- HNSW is a disposable cache. Missing, corrupt, mismatched, oversized, or panicking
  cache data falls back to exact cosine and is rebuilt from SQLite.
- Interrupted HNSW directory replacement restores the last `.old` generation under
  the cache lock.

Committed synthetic fixtures cover the legacy SQLite database and the complete HNSW
manifest/data/graph set. Fixture checksums are registered in
`crates/knowledge/fixtures/manifest.json`.

## Recovery

1. Keep the original source files and Markdown sidecars.
2. If SQLite reports incompatible vectors, use the desktop rebuild command.
3. HNSW can be removed independently; the next index/search cycle rebuilds it.
4. Never copy a provider index between deployments unless their signatures match.

## Deferred concurrency/performance work

SQLite is authoritative and detects concurrent commits while embeddings are prepared,
returning a safe retry instead of partially writing. HNSW generation ordering across
multiple independent desktop processes remains a cache-level optimization concern;
search falls back to exact cosine when candidates are missing or incompatible. Tuning,
cross-process generation epochs, and large-corpus performance belong to a dedicated
post-extraction backlog and do not change Phase 1A contracts.

## Verification

```bash
make check-knowledge-extraction
make check-desktop
```
