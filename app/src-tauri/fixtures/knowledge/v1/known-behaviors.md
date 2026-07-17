# Frozen desktop RAG behavior (v1)

P1A extraction preserves these behaviors first; later issues may change them with an
explicit contract/version update.

- Commands: `rebuild_knowledge_index`, `knowledge_index_stats`, `hybrid_search`,
  `hybrid_ask`; request payload is wrapped in `req` except stats.
- Defaults: search limit 20, ask topK 8, ask `useLlm=false`.
- Modes: `offline_extractive`, `fallback_extractive`, `local_llm`, `cloud_llm`,
  `subscription_cli`.
- Local embedding mode is `local_hash_v1`, dimension 256; stats report ANN threshold
  1000.
- Search with non-empty scope may update the index before reading.
- Small indexes do not persist HNSW; exact vector search is used.
- Provider/signature mismatch warns and may fall back to lexical/local behavior.
- Fallback answers currently report `grounded=true`; this semantic mismatch is frozen,
  not endorsed.
- Score fields are f32. Contract comparisons use epsilon `0.0001`; fixed fixture hit
  order is exact.
- Legacy chunk IDs use Rust hashing and are not promised cross-Rust-version stability.
- Warning strings are user-facing Vietnamese and fixture scenarios lock their current
  text where deterministic.
