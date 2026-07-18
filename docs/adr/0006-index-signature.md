# ADR 0006: Canonical index signature and chunk identity (P0-06)

- Status: Accepted
- Date: 2026-07-18
- Owners: retrieval-owner, architecture-owner
- Approver: Phase 0 architecture gate
- Related issues/PRs: P0-06; ADR 0002, 0004, 0005

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
   - `runtime_path`: `local-hash` | `glm-cloud-interim` | `vllm-local` |
     `provider-cloud` — **explicit field** on `EmbeddingPlan` and
     `EmbeddingConfig` (not inferred from the coarse `Provider` enum). Desktop
     presets pin this (e.g. vLLM `127.0.0.1:8000` + `BAAI/bge-m3` →
     `vllm-local`, GLM → `glm-cloud-interim`). Host/model inference via
     `infer_embedding_runtime_path` is only a fallback for unknown/custom
     endpoints — real vLLM preset URLs do not contain the string `"vllm"`.
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
   chunking version. Filling `chunkId` into every query citation may follow in a
   later P0-06 PR once span→chunk resolution is green.

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
cargo test -p fileconv-knowledge --lib identity::tests
python3 bench/markhand_web/scripts/generate_expected_chunks.py
python3 bench/markhand_web/scripts/run_retrieval_eval.py --self-test
```

Inspect `crates/knowledge/fixtures/identity-v2.json` (schema v2 payload) and
`bench/markhand_web/reports/retrieval-evaluation.md`.
