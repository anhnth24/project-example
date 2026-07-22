# Runbook: GLM / qa_provider fallback

Issue: P1B-O02  
Alert: `MarkhandGlmProviderErrors`  
Dashboard: Grafana `markhand-deps`  
Threshold source: availability SLA 99.5% → outbreak at 5% `qa_provider` error ratio.  
Note: GLM is Q&A only; embedding remains on-prem local-neural (ADR 0005).

## Prerequisites

- Provider credentials stored in secret manager (never in git/alerts).
- Understanding that search/citation must keep working without GLM answers.

## Detection

1. Confirm qa_provider error ratio > 0.05 for ≥10m via alert expression / fixture series.
2. Check `markhand_retrieval_total{leg="qa_provider",result="error"}` rate.
3. Distinguish provider outage vs local retrieval failure (`leg` vector/lexical/hybrid).

## Contain

1. Disable or short-circuit ask/stream answer path to fail closed with a user-safe error
   (no prompt/content in logs).
2. Keep `/search` and citation paths up if vector/lexical legs are healthy.
3. Do not broaden egress or disable auth to “make GLM work”.

## Recover

1. Validate provider endpoint/status with a non-production synthetic probe.
2. Rotate provider key if auth errors dominate (see [key-rotation](key-rotation.md)).
3. Restore ask/stream when error ratio < 0.05 for ≥15 minutes.
4. If provider remains down, leave Q&A degraded and communicate status.

## Verify

1. `MarkhandGlmProviderErrors` clears.
2. Search/retrieval legs remain within SLO; no cross-tenant leakage in deny tests.
3. Ask path either healthy or intentionally disabled with clear error code.

## Rollback

- Re-disable ask/stream if provider errors return.
- Revert key/config change if the rotation caused auth failures.
- Keep search available independently of GLM.

## Synthetic evidence

Fixture: `MarkhandGlmProviderErrors.json`  
Tabletop: `tt-glm` — synthetic qa_provider error ratio only.
