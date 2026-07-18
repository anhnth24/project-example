# ADR 0004: Vietnamese embedding model selection (P0-05)

- Status: Proposed
- Date: 2026-07-18
- Owners: retrieval-owner
- Approver: product-owner
- Supersedes: N/A
- Related issues/PRs: P0-05

## Context

Phase 0 gate `G0-RET-RECALL-AT-5` requires Recall@5 >= 0.85 on the Vietnamese
golden corpus. OpenAI cloud embeddings failed the dense quality gate on this
corpus (best observed ~0.78). P0-05 also requires comparing at least two model
families on the same corpus/hardware with immutable config evidence, plus
capacity measurements on target GPU.

Public Vietnamese evidence points to a BGE-M3 fine-tune as the strongest dense
candidate and a lighter PhoBERT bi-encoder as the smallest model still reporting
Accuracy@5 >= 0.85 on Zalo Legal.

## Decision

**Draft (quality-track only, not Accepted):**

1. Best candidate: `AITeamVN/Vietnamese_Embedding` (1024-d, L2, max_seq=2048)
2. Min candidate: `bkai-foundation-models/vietnamese-bi-encoder`
   (768-d, L2, max_seq=256, mandatory `pyvi` word segmentation)

Chunking pinned to `heading-chunks-2000-v1`. Ranking for this eval is dense
max-pool chunk cosine aggregated to documents.

This ADR stays `Proposed` until:

- quality gates pass on golden corpus with >=3 runs;
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

- OpenAI `text-embedding-3-*` / `ada-002`: measured dense Recall@5 < 0.85 on
  Markhand golden; rejected for selection.
- Base `BAAI/bge-m3` dense-only: public Zalo Acc@5 ~0.838 (below margin); keep as
  backbone reference, not draft selected model.
- `intfloat/multilingual-e5-large`: strong hybrid partner in multi-domain studies;
  deferred to a later comparison once the draft pair has local evidence.

## Verification

```bash
python3 -m pip install --user -r bench/markhand_web/requirements-embedding.txt
python3 bench/markhand_web/scripts/run_embedding_eval.py --runs 3
```

Inspect `bench/markhand_web/reports/embedding-evaluation.md` and
`bench/markhand_web/embedding/results/summary.json`.

## Exception lifecycle

N/A while Proposed. Capacity exception (CPU smoke != target GPU) expires when
Profile B GPU evidence is attached or the issue is re-scoped.
