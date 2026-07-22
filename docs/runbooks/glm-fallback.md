# Runbook: GLM / qa_provider fallback

Issue: P1B-O02
Alert: `MarkhandGlmProviderErrors` (metric-based)
Blocked: `MarkhandGlmProbeDown` — **no configured GLM health endpoint** in deploy stacks
Dashboard: `markhand-deps`
Threshold: O02-OPS-ERROR-OUTBREAK-RATIO (5%). GLM is Q&A only (ADR 0005).

## Prerequisites

- Provider credentials in secret manager / env — never commit or paste into tickets
- Search/citation must keep working without GLM answers

## Detection

```bash
REPO_ROOT="$(git rev-parse --show-toplevel)"
export REPO_ROOT
export POC_WITH_OBSERVABILITY=1
# shellcheck source=deploy/scripts/poc-compose.sh
source "$REPO_ROOT/deploy/scripts/poc-compose.sh"
poc_compose_init

curl -fsG http://127.0.0.1:9090/api/v1/query \
  --data-urlencode 'query=markhand:qa_provider:failure_ratio_10m'
# Failures: result=~error|outage|timeout|truncated|other on leg=qa_provider
```

There is **no** blackbox GLM probe in `prometheus.yml` (intentionally blocked).

## Contain

1. Disable ask/stream traffic at the edge / feature flag if available.
2. Keep `POST /api/v1/search` up when hybrid/lexical/vector legs are healthy.
3. Do not broaden egress or disable auth to “fix” GLM.

## Recover

1. Validate provider status with your vendor console (outside this repo).
2. If auth failures dominate, follow [key-rotation](key-rotation.md) for the provider key.
3. Reload API with updated sealed secrets (environment-specific — no checked-in secret files).
4. Re-enable ask/stream when failure ratio < 0.05 for ≥15m.

## Verify

1. `MarkhandGlmProviderErrors` clears.
2. Search path within SLO; unauthorized still denied.
3. Ask path healthy or intentionally disabled with a safe error code (no prompt logging).

## Rollback

- Re-disable ask/stream if errors return.
- Never reuse a leaked provider key.

## Synthetic evidence

Promtool `qa_provider_outage_outbreak`. No live GLM outage claimed.
