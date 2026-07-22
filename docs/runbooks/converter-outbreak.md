# Runbook: Converter / parser outbreak

Issue: P1B-O02  
Alert: `MarkhandConversionErrorOutbreak`  
Dashboard: Grafana `markhand-ops`  
Threshold source: `docs/markhand-web-sla-targets.md` availability 99.5% → outbreak at 5% error ratio (10× error budget).

## Prerequisites

- Access to conversion metrics and worker logs (redacted; no document text).
- Ability to disable specific formats or drain the `convert` queue.

## Detection

1. Confirm `markhand:conversion:error_ratio_10m > 0.05` for ≥10m.
2. Break down `markhand_conversion_total` by bounded `format` and `result`.
3. Check whether one format dominates errors (pdf/image/audio/…).
4. Correlate with queue age on `queue="convert"` and host resource saturation.

## Contain

1. Pause `convert` workers if error ratio is climbing.
2. Optionally reject new uploads for the failing format at the edge (if supported).
3. Preserve failing job IDs only (no content) for later triage.

## Recover

1. Roll back the last converter/config change if the outbreak followed a deploy.
2. Restart convert workers after dependency health is green.
3. Requeue idempotent convert jobs in small batches.
4. If a single format is poison, keep it disabled and open a fix ticket.

## Verify

1. `markhand:conversion:error_ratio_10m` back under 0.05.
2. Convert queue age decreasing; finish success rate recovering.
3. Spot-check allowlisted format smoke (metadata only; no content in evidence).
4. Clear the alert after the pending window.

## Rollback

- Re-disable the suspect format/config.
- Re-pause convert workers if errors rebound.
- Do not bulk-requeue until error ratio is stable for ≥15 minutes.

## Synthetic evidence

Fixture: `deploy/observability/fixtures/alerts/MarkhandConversionErrorOutbreak.json`  
Tabletop: `tt-converter-outbreak` — synthetic only; no live outbreak claimed.
