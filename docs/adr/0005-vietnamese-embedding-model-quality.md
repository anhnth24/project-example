# ADR 0005: Vietnamese embedding — AITeamVN local for POC/1B

- Status: Accepted
- Date: 2026-07-18
- Accepted: 2026-07-20
- Owners: retrieval-owner
- Approver: product-owner
- Supersedes: [ADR 0004](0004-interim-glm-cloud-embedding.md) (POC/1B embedding runtime)
- Related issues/PRs: P0-05; ADR 0006 index signature; journal
  [`2026-07-20-aiteamvn-local-embedding-decision`](../journals/2026-07-20-aiteamvn-local-embedding-decision.md)

## Context

Phase 0 gate `G0-RET-RECALL-AT-5` requires Recall@5 >= 0.85 on the Vietnamese
golden corpus. OpenAI cloud embeddings were re-measured with the same harness
protocol as local models (desktop `{heading}\n{text}` payload, fixture lock,
per-query rows + `rankingSha256`) via `FILECONV_EMBEDDING_API_KEY` →
`api.openai.com`. Best dense Recall@5 was **0.7752** (`text-embedding-ada-002`,
1536-d) — see `bench/markhand_web/embedding/results/openai-rejected/`. The
track is non-gating reject evidence (not a selection draft).

ADR 0004 (2026-07-18) allowed GLM cloud as an interim embedding path before
Profile B GPU existed. Measured local evidence (`local-cpu-quality`) showed
`AITeamVN/Vietnamese_Embedding` clears quality gates without sending full chunk
text to a cloud provider. Index build egress is higher risk than Q&A top-K
handoff; product chose **local embedding + GLM chat-only** for Markhand Web.

## Decision

1. **Selected embedding (dev / POC / DEMO / Phase 1B):**
   `AITeamVN/Vietnamese_Embedding`
   @ `dea33aa1ab339f38d66ae0a40e6c40e0a9249568`
   (1024-d, L2, max_seq=2048, desktop `{heading}\n{text}` payload,
   `runtime_path=local-neural`) — measured Recall@5 **0.9261** (min of 3
   independent loads), nDCG gap **0.0** vs comparator on golden corpus.
2. **Comparator (documented failure):**
   `bkai-foundation-models/vietnamese-bi-encoder`
   @ `84f9d9ada0d1a3c37557398b9ae9fcedcdf40be0`
   (768-d, L2, max_seq=256, mandatory `pyvi` word segmentation) — measured
   Recall@5 **0.7962** (**below** 0.85). Not selectable.
3. **Runtime delivery:** OpenAI-compatible local server on CPU
   (`deploy/dev/embedding-cpu`, port `8088`) for dev/POC; same model pin for
   Phase 1B index/embedding workers. License: Apache-2.0, approved for bundle
   (`docs/markhand-web-model-license-inventory.md`).
4. **GLM cloud:** approved for **grounded Q&A / summarize / structured
   extraction** only (LLM client, top-K citation handoff). **Not** the Markhand
   Web server embedding runtime — cloud embedding sends full chunk text at index
   time.
5. **Target runtime (production aggregate scale):** self-host **vLLM** on Profile B
   GPU with `BAAI/bge-m3` and at least one multilingual-e5 family model
   (`runtime_path=vllm-local`). Cutover rebuilds the vector index;
   `G0-RET-VLLM-CUTOVER` remains a Phase 4 / production gate, not a Phase 1B
   blocker.

Chunking pinned to `heading-chunks-2000-v1`. Hybrid retrieval uses frozen RRF
`VECTOR_WEIGHT=0.55` (P0-06).

## Consequences

- Positive: no customer/restricted corpus egress for embedding; quality exceeds
  gate on CPU smoke hardware.
- Positive: dev stack matches Phase 1B worker path (`local-neural`).
- Negative: BKAI requires word segmentation; forgetting it silently hurts quality.
- Negative: CPU throughput ≠ Profile B GPU capacity claims; ingest saturation
  evidence still deferred.
- Migration: changing model/dim/normalize/chunking creates a new index signature
  generation (ADR 0006, ADR 0011).
- Security: Markhand Web server must not use cloud embedding for production
  customer data; GLM Q&A policy unchanged.

## Alternatives considered

- **GLM cloud interim embedding (ADR 0004):** superseded — avoids waiting for GPU
  but sends full chunk text; rejected as Markhand Web server default after local
  quality evidence passed.
- OpenAI `text-embedding-3-*` / `ada-002`: dense Recall@5 < 0.85; rejected.
  Auditable reject pack: `bench/markhand_web/embedding/results/openai-rejected/`.
- Base `BAAI/bge-m3` dense-only: public Zalo Acc@5 ~0.838 (below margin); deferred
  to vLLM cutover comparison on Profile B.
- `intfloat/multilingual-e5-large`: deferred to Profile B GPU evaluation.
- **Local hash 256-D only:** insufficient semantic quality for Vietnamese RAG.

## Verification

```bash
python3 -m pip install --user -r bench/markhand_web/requirements-embedding.txt
python3 bench/markhand_web/scripts/run_embedding_eval.py --self-test
python3 bench/markhand_web/scripts/run_embedding_eval.py --runs 3
```

Evidence:

- `bench/markhand_web/reports/embedding-evaluation.md`
- `bench/markhand_web/embedding/results/summary.json`
- `bench/markhand_web/retrieval/summary.json` (hybrid with same pin)
- Dev stack: `make dev-up` → `embedding-cpu` @ `:8088`

Harness notes:

- embedding payload = desktop `{heading}\n{text}` (including empty heading)
- each run reloads the model; Recall@5 gate uses **min**, nDCG gap uses **max**
- selection requires both quality gates under gating protocol (≥2 families, ≥3 runs)
- catalog `models.yaml` gates/chunking/normalize/ranking are authoritative
- revisions must be full 40-hex SHAs; fixtures checked vs `manifest.lock.json`

## Exception lifecycle

| Field | Value |
|---|---|
| Exception | CPU smoke capacity ≠ Profile B GPU throughput |
| Owner | `infrastructure-owner` |
| Scope | Capacity/SLO claims only; quality selection closed |
| Expiry | Profile B + vLLM cutover evidence or Phase 4 production gate |
| Retest | `run_embedding_eval.py --runtime vllm-local` on `on-prem-reference` |
