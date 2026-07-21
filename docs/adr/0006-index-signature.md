# ADR 0006: Canonical index signature and chunk identity (P0-06)

- Status: Accepted
- Date: 2026-07-18
- Owners: retrieval-owner, architecture-owner
- Approver: Phase 0 architecture gate
- Related issues/PRs: P0-06; ADR 0002, 0004, 0005; PR #209

## Context

Dense and hybrid indexes must refuse to mix incompatible vector generations.
P0-05 pinned embedding candidates and chunking label `heading-chunks-2000-v1`,
but `chunk_identity` lacked `version_id`, and `IndexSignature` collapsed body NFC
and query accent-fold into one `text_version` while the fixture still said
`heading-v1` / `nfc-v1`.

## Decision

1. **Identity schema version = 2** (`IDENTITY_VERSION` in
   `crates/knowledge/src/identity.rs`). Field-layout changes bump this version.
2. **Chunk identity** is the length-delimited SHA-256 of:
   `document_id`, `version_id`, `ordinal`, `heading_path`, `body`,
   `body_text_version` (`nfc-v1`). Versions never share chunk IDs.
3. **Index signature** is the length-delimited SHA-256 of:
   - `runtime_path`: `local-hash` | `local-neural` | `glm-cloud-interim` |
     `vllm-local` | `provider-cloud` — **explicit field** on `EmbeddingPlan` and
     `EmbeddingPlan` and `EmbeddingConfig` (not inferred from the coarse `Provider`
     enum). Markhand Web POC/1B pins **`local-neural`** with
     `AITeamVN/Vietnamese_Embedding` (ADR 0005). Desktop presets may still set
     `vllm-local` or `glm-cloud-interim`; CPU sentence-transformers quality track
     uses `local-neural`. Host/model inference via
     `fileconv_core::embedding_runtime::infer_embedding_runtime_path` is only a
     fallback for unknown/custom endpoints — real vLLM preset URLs do not
     contain the string `"vllm"`.
   - **Single inference owner (CORE-T13):** the helper lives in always-on
     `fileconv_core::embedding_runtime` (not behind the `llm` feature).
     `fileconv_core::llm` re-exports it; `fileconv_knowledge::embedding::infer_runtime_path`
     is a thin alias. Runtime-path string constants are defined once in core and
     re-exported by knowledge as `RUNTIME_*`. Behavior is pinned by
     `INFER_EMBEDDING_RUNTIME_PATH_CASES` (schemed / scheme-less, localhost /
     private / custom hosts, GLM/bigmodel/z.ai/zhipu, vLLM cues, provider
     defaults, malformed URLs, scheme casing, IPv6, userinfo). Changing a row’s
     expected path changes the index signature for custom endpoints that relied
     on inference and **must** trigger reindex.
   - `embedding_family` (provider/model/deployment digest)
   - `embedding_revision`
   - `dimensions` (u64 BE)
   - `normalized` (bool byte)
   - `chunking_version` (`heading-chunks-2000-v1`)
   - `body_text_version` (`nfc-v1`)
   - `query_normalization_version` (`accent-fold-v1`)
4. Changing any signature field creates a **new index generation**. Mixing
   vectors across generations is forbidden; desktop rebuilds on signature
   mismatch. Legacy desktop stores that persisted the bare string
   `local_hash_v1` as signature must rebuild under the schema-v2 digest.
5. Historical fixture `identity-v1.json` remains frozen for checksum continuity;
   live digests / tests use `identity-v2.json`.
6. Golden evaluation pins chunk catalog in
   `bench/markhand_web/retrieval/expected-chunks.tsv` generated from the same
   chunking version. Query/conflict citation `chunkId` fields are filled from
   that catalog (`fill_citation_chunk_ids.py`).

## Consequences

- Positive: glm-cloud vs local-hash vs vLLM cannot silently share an index;
  version-aware citations have stable chunk IDs.
- Negative: any prior identity digests (schema v1) are obsolete; caches rebuild.
- Migration: bump signature → drop/rebuild vector store for that scope.

## Alternatives considered

- Keep single `text_version`: rejected; body NFC and query folding change
  different surfaces and must pin independently.
- Soft-migrate without `IDENTITY_VERSION` bump: rejected; ambiguous digests.

## Verification

```bash
cargo test -p fileconv-core embedding_runtime
cargo test -p fileconv-core --features llm infer_embedding_runtime_path
cargo test -p fileconv-knowledge --lib embedding::tests::runtime_path_inference_matches_core_behavior_table
cargo test -p fileconv-knowledge --lib identity::tests
python3 bench/markhand_web/scripts/generate_expected_chunks.py --check
python3 bench/markhand_web/scripts/fill_citation_chunk_ids.py --check
python3 bench/markhand_web/scripts/run_retrieval_eval.py --self-test
python3 bench/markhand_web/scripts/run_retrieval_eval.py
```

Inspect `crates/knowledge/fixtures/identity-v2.json` (schema v2 payload) and
`bench/markhand_web/reports/retrieval-evaluation.md` (`p0_06_closed`).

## CORE-T13 migration notes

- Desktop presets that already set explicit `runtime_path` (vLLM → `vllm-local`,
  GLM → `glm-cloud-interim`) are unchanged; no reindex for those stores.
- Custom endpoints that previously diverged between core’s string host hint and
  knowledge’s `url::Url::parse` (scheme-less hosts, uppercase `HTTP(S)`, IPv6)
  now share one table. If a store was built with the old knowledge parser and
  the unified helper yields a different `runtime_path`, signature mismatch
  rebuilds the vector index (correct and required).
