# Interim GLM cloud embedding for early Markhand Web delivery

**Date**: 2026-07-18
**Severity**: Medium
**Component**: Phase 0 embedding gates, Markhand Web RAG
**Status**: Superseded (embedding path) — see ADR 0005 / journal 2026-07-20

## What Happened

P0-05 was blocked on Profile B GPU / local vLLM even though coding, POC and DEMO
needed a neural embedding path sooner. Product approved an interim cloud path
using GLM embeddings, with on-prem vLLM kept as the production target.

## Decision

- ~~Interim: GLM `embedding-3` (compare `embedding-2`) via OpenAI-compatible API and
  `FILECONV_EMBEDDING_API_KEY` on synthetic/de-identified corpus only.~~
  **Superseded 2026-07-20:** Markhand Web embedding uses local AITeamVN CPU
  (`local-neural`); GLM retained for Q&A only (ADR 0005).
- Target: Profile B + vLLM (`bge-m3` / multilingual-e5); cutover rebuilds index.
- Recorded in ADR 0004 (superseded) → ADR 0005 (Accepted); vLLM cutover is
  `G0-RET-VLLM-CUTOVER` (block-phase-4).

## Follow-ups

- ~~Implement `run_embedding_eval.py` against GLM and pin signature fields.~~ Done
  on local families (AITeamVN vs BKAI).
- Before production: re-measure on Profile B vLLM; GLM embedding not used on server.
