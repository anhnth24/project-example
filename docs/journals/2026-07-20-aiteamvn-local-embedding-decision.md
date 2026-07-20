# AITeamVN local embedding for Markhand Web; GLM chat-only

**Date**: 2026-07-20
**Severity**: Medium
**Component**: Phase 0 embedding gates, Markhand Web RAG, dev stack
**Status**: Resolved

## What Happened

ADR 0004 (2026-07-18) approved GLM cloud as an interim **embedding** path so P0-05
could unblock before Profile B GPU existed. Subsequent quality evidence on
`local-cpu-quality` showed `AITeamVN/Vietnamese_Embedding` clears both
`G0-RET-RECALL-AT-5` (0.9261) and `G0-RET-BEST-MODEL-GAP` (0.0 vs BKAI) without
any cloud corpus egress. Dev stack and Phase 1B workers (`embedding-cpu` @
`:8088`, I06 index/embedding workers) were implemented on that local path.

Product confirmed: **embedding stays on-prem local (AITeamVN CPU for POC/1B);
GLM cloud is for grounded Q&A / summarize only** (top-K citation handoff, not
full-chunk index egress).

## Decision

- **Embedding (index + hybrid retrieval):** `AITeamVN/Vietnamese_Embedding`,
  `runtime_path=local-neural`, 1024-d L2, pinned in index signature (ADR 0005).
- **Chat / Q&A:** GLM via existing LLM client policy; no change.
- **Supersede:** ADR 0004 embedding interim default → ADR 0005 Accepted.
- **Deferred unchanged:** Profile B GPU + vLLM cutover (`G0-RET-VLLM-CUTOVER`) before
  production aggregate scale; cutover rebuilds vector index.

## Follow-ups

- Keep `glm-cloud-interim` as a signature enum value for optional desktop presets
  only; Markhand Web server runtime must not use cloud embedding for customer data.
- Re-measure on Profile B GPU before production embedding cutover claims.
