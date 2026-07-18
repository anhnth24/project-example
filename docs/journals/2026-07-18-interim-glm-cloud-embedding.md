# Interim GLM cloud embedding for early Markhand Web delivery

**Date**: 2026-07-18
**Severity**: Medium
**Component**: Phase 0 embedding gates, Markhand Web RAG
**Status**: Resolved

## What Happened

P0-05 was blocked on Profile B GPU / local vLLM even though coding, POC and DEMO
needed a neural embedding path sooner. Product approved an interim cloud path
using GLM embeddings, with on-prem vLLM kept as the production target.

## Decision

- Interim: GLM `embedding-3` (compare `embedding-2`) via OpenAI-compatible API and
  `FILECONV_EMBEDDING_API_KEY` on synthetic/de-identified corpus only.
- Target: Profile B + vLLM (`bge-m3` / multilingual-e5); cutover rebuilds index.
- Recorded in ADR 0004; P0-05 acceptance and G0-RET quality gates now use
  `glm-cloud-interim`; vLLM cutover is `G0-RET-VLLM-CUTOVER` (block-phase-4).

## Follow-ups

- Implement `run_embedding_eval.py` against GLM and pin signature fields.
- Before production: re-measure on Profile B and retire GLM embedding credentials
  from server runtime.
