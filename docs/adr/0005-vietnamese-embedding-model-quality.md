# ADR 0005: Vietnamese embedding dense quality candidates (P0-05)

- Status: Proposed
- Date: 2026-07-18
- Owners: retrieval-owner
- Approver: product-owner
- Supersedes: N/A
- Related issues/PRs: P0-05; complements ADR 0004 interim GLM path

## Context

Phase 0 gate `G0-RET-RECALL-AT-5` requires Recall@5 >= 0.85 on the Vietnamese
golden corpus. OpenAI cloud embeddings were re-measured with the same harness
protocol as local models (desktop `{heading}\n{text}` payload, fixture lock,
per-query rows + `rankingSha256`) via `FILECONV_EMBEDDING_API_KEY` →
`api.openai.com`. Best dense Recall@5 was **0.7752** (`text-embedding-ada-002`,
1536-d) — see `bench/markhand_web/embedding/results/openai-rejected/`. The
track is non-gating reject evidence (not a selection draft). P0-05 also requires
comparing at least two local model families on the same corpus/hardware with
immutable config evidence, plus capacity measurements on target GPU.

Public Vietnamese evidence points to a BGE-M3 fine-tune as the strongest dense
candidate and a lighter PhoBERT bi-encoder as the smallest model still reporting
Accuracy@5 >= 0.85 on Zalo Legal.

## Decision

**Draft (quality-track only, not Accepted):**

1. Best / selected draft: `AITeamVN/Vietnamese_Embedding`
   @ `dea33aa1ab339f38d66ae0a40e6c40e0a9249568`
   (1024-d, L2, max_seq=2048, desktop `{heading}\n{text}` payload) — measured
   Recall@5 **0.9261** (min of 3 independent loads) on golden corpus.
2. Min comparator measured: `bkai-foundation-models/vietnamese-bi-encoder`
   @ `84f9d9ada0d1a3c37557398b9ae9fcedcdf40be0`
   (768-d, L2, max_seq=256, mandatory `pyvi` word segmentation) — measured
   Recall@5 **0.7962** (**below** 0.85). Not selectable.

Chunking pinned to `heading-chunks-2000-v1`. Ranking for this eval is dense
max-pool chunk cosine aggregated to documents.

This ADR stays `Proposed` until:

- a second family also clears the quality gate (or product accepts single-family
  selection with documented comparator failure);
- capacity/VRAM/saturation evidence exists on `on-prem-reference` GPU;
- license review completes before any runtime bundle;
- approvers sign off.

## Consequences

- Positive: local/self-host path avoids restricted-corpus cloud egress.
- Negative: BKAI requires word segmentation; forgetting it silently hurts quality.
- Migration: changing model/dim/normalize/chunking creates a new index signature
  generation (P0-06).
- Security: do not send restricted corpora to cloud embedding APIs.

## Alternatives considered

- OpenAI `text-embedding-3-*` / `ada-002`: harness re-run reports dense
  Recall@5 < 0.85 (ada-002 best 0.7752); rejected for selection. Auditable
  reject pack:
  `bench/markhand_web/embedding/results/openai-rejected/`
  (`openai-models.yaml` + `run-*.json` rows/fingerprints).
- Base `BAAI/bge-m3` dense-only: public Zalo Acc@5 ~0.838 (below margin); keep as
  backbone reference, not draft selected model.
- `intfloat/multilingual-e5-large`: strong hybrid partner in multi-domain studies;
  deferred to a later comparison once the draft pair has local evidence.

## Verification

```bash
python3 -m pip install --user -r bench/markhand_web/requirements-embedding.txt
python3 bench/markhand_web/scripts/run_embedding_eval.py --self-test
python3 bench/markhand_web/scripts/run_embedding_eval.py --runs 3
```

Harness notes (Sol review fixes):

- embedding payload = desktop `{heading}\n{text}` (including empty heading)
- each run reloads the model; Recall@5 gate uses **min**, nDCG gap uses **max**
- selection requires both quality gates under gating protocol (≥2 families, ≥3 runs)
- catalog `models.yaml` gates/chunking/normalize/ranking are authoritative
- revisions must be full 40-hex SHAs; fixtures checked vs `manifest.lock.json`
- per-query `rows` kept in `run-*.json`; dirty paths recorded at eval start

Inspect `bench/markhand_web/reports/embedding-evaluation.md` and
`bench/markhand_web/embedding/results/summary.json`.

## Exception lifecycle

N/A while Proposed. Capacity exception (CPU smoke != target GPU) expires when
Profile B GPU evidence is attached or the issue is re-scoped.
