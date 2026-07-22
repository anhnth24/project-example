# ADR 0006: Canonical index signature and chunk identity (P0-06)

- Status: Accepted
- Date: 2026-07-18
- Owners: retrieval-owner, architecture-owner
- Approver: Phase 0 architecture gate
- Related issues/PRs: P0-06; ADR 0002, 0004, 0005; PR #209; CORE-T13

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
     `EmbeddingConfig` (not inferred from the coarse `Provider` enum). Markhand
     Web POC/1B pins **`local-neural`** with `AITeamVN/Vietnamese_Embedding`
     (ADR 0005). Desktop presets may still set `vllm-local` or
     `glm-cloud-interim`; CPU sentence-transformers quality track uses
     `local-neural`. Public config / persisted values must pass
     `fileconv_core::embedding_runtime::parse_embedding_runtime_path` (empty,
     control characters, and unknown aliases such as `local_hash_v1` are
     rejected with typed errors) **before** index-signature digests or index
     loads. Host/model inference via
     `fileconv_core::embedding_runtime::infer_embedding_runtime_path` is only a
     fallback for unknown/custom endpoints — real vLLM preset URLs do not
     contain a `vllm` DNS label.
   - **Single inference owner (CORE-T13):** always-on
     `fileconv_core::embedding_runtime` (not behind the `llm` feature) owns
     constants and inference. `fileconv_core::llm` re-exports the helper;
     `fileconv_knowledge::embedding` exposes
     `pub use … as infer_runtime_path`. HTTP(S) endpoints are parsed with
     `url::Url`; scheme-less values get an `https://` prefix only when
     syntactically plausible. Malformed / non-http endpoints silently yield an
     empty host (model cues may still apply). Parsed DNS hosts are
     canonicalized **once**: lowercase, strip at most one terminal root dot,
     then reject leading/trailing `.` or empty `..` labels
     (`open.bigmodel.cn.` ≡ `open.bigmodel.cn`; `.bigmodel.cn`,
     `open.bigmodel.cn..`, `vllm..internal` → empty host). Provider domains
     match at DNS
     label boundaries (`z.ai` does not match `modelz.ai`). Cue order:
     official GLM host → vLLM host → known provider/loopback → anchored model
     cues → default `provider-cloud`. A vLLM host beats a GLM-named model;
     `vllm.bigmodel.cn` is official GLM first. GLM model ids `embedding-2` /
     `embedding-3` match only as the full model string (or after `/`) with a
     non-alphanumeric-or-EOS terminator — not as a prefix of
     `embedding-3000` / `embedding-3rdparty` / `text-embedding-3-small`.
     Changing inference for a custom endpoint **must** trigger reindex.
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
- Parallel string-hint vs `Url::parse` helpers in core/knowledge: rejected
  (CORE-T13); divergent hosts changed index signatures across crates.

## Verification

```bash
cargo test -p fileconv-core embedding_runtime
cargo test -p fileconv-core --features llm llm_export_infer_embedding_runtime_path_literal_cases
cargo test -p fileconv-knowledge --lib embedding::tests::infer_runtime_path_literal_cases
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
- Custom endpoints that relied on inference may change `runtime_path` under the
  unified rules (URL parser, DNS-label domains, cue order, scheme-less prefix).
  Signature mismatch rebuilds the vector index (correct and required).
- Notable semantic deltas vs earlier dual helpers: `modelz.ai` is not GLM;
  invalid `[vllm::1]` does not count as a vLLM host; backslash-userinfo cannot
  spoof `bigmodel.cn` / `vllm.*`; vLLM hosts win over GLM-named models;
  non-official hosts such as `glm.example.com` no longer match via substring;
  absolute DNS forms with a trailing root dot canonicalize; `embedding-3000` /
  `embedding-3rdparty` are not GLM model cues.
