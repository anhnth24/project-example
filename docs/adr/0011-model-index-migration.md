# ADR 0011: Model and index migration lifecycle

- Status: Accepted
- Date: 2026-07-18
- Decision key: `model-index-migration`
- Owners: retrieval-owner, worker-owner, operations-owner
- Approver: Phase 0 architecture gate
- Related issues/PRs: P0-10; ADR 0004; ADR 0005; ADR 0006; ADR 0009

## Context

ADR 0006 makes index signatures canonical: embedding runtime path, model family,
revision, dimensions, normalization, chunking, body text version and query
normalization are part of the vector generation. Any mismatch can change nearest
neighbors or citation identity. The system therefore needs a migration contract
for model cutovers, signature mismatches and rebuilds.

## Decision

An index signature mismatch is fail-closed for retrieval and triggers rebuild or
operator-directed migration. The server must not silently mix vectors from
different signatures in the same visible generation.

Every index generation has:

```text
index_generation_id, index_signature, runtime_path, embedding_family,
embedding_revision, dimensions, normalized, chunking_version,
body_text_version, query_normalization_version, state
```

where `state` is `building`, `shadow`, `active`, `draining` or `retired`.

The migration lifecycle is:

1. **Expand:** create a new generation and destination Qdrant collection or
   namespace; keep PostgreSQL chunk/catalog data authoritative.
2. **Backfill/rebuild:** enqueue idempotent embedding batches from PostgreSQL
   chunks and immutable document versions. Store progress checkpoints and batch
   checksums.
3. **Shadow verify:** run retrieval/citation checks against the new generation
   without serving users. Compare recall, version-citation precision/recall and
   signature metadata.
4. **Cutover:** atomically switch the active generation pointer in PostgreSQL
   when shadow checks pass. Query code reads the active pointer and refuses
   ambiguous/missing signatures.
5. **Contract:** retire old vectors after rollback window, backup retention and
   audit requirements are met.

Desktop/local caches may drop and rebuild on mismatch. Server Profile B
migrations use expand/cutover/contract so query-ready service can return before
full vector rebuild only when PostgreSQL text/FTS and authorized current metadata
are restored.

## Consequences

- Positive: embedding cutovers from GLM interim to on-prem vLLM cannot corrupt
  existing citations or mix dimensions.
- Positive: rebuild progress is durable and restartable.
- Negative: production cutovers require extra storage during expand/shadow.
- Operational: rollback means repointing the active generation while the old
  generation is still retained; after contract, rollback requires rebuild.

## Alternatives considered

- In-place vector overwrite: rejected because partial rebuilds would produce
  mixed signatures and irreproducible rankings.
- Keep one Qdrant collection forever with only payload flags: rejected for
  cutover safety unless the generation filter is mandatory and enforced exactly
  like separate generations.
- Accept approximate compatibility between model revisions: rejected. ADR 0006
  treats signature fields as hard compatibility boundaries.

## Verification

Phase 1B/scale implementation must include:

```bash
cargo test -p fileconv-knowledge --lib identity::tests
cargo test -p fileconv-server index_migration
python3 bench/markhand_web/scripts/run_retrieval_eval.py
```

Required cases:

- signature mismatch refuses retrieval or selects a rebuild path;
- backfill is idempotent and resumes from checkpoints;
- cutover changes the active generation atomically;
- stale/retired generation queries are denied unless explicitly in a rollback
  maintenance path;
- rollback works before contract.

P0-10 accepts the migration contract only. Production migration timing remains
blocked pending Profile B evidence.

## Exception lifecycle

N/A. Local desktop caches may rebuild in place because they are single-user
caches, but server shared generations follow expand/cutover/contract.
