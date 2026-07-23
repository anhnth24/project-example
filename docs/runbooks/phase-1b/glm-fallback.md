# GLM / chat provider fallback

## Detect

- Alert: `MarkhandProviderErrors`
- Query:

```promql
sum(rate(markhand_provider_duration_seconds_count{outcome="error"}[5m])) by (provider)
/
clamp_min(sum(rate(markhand_provider_duration_seconds_count[5m])) by (provider), 1e-9)
```

- Ask warnings: provider unavailable / grounding failure (metadata only).

## Contain

- Leave retrieval online; extractive answers remain available.
- Do not disable citation enforcement to “make chat work”.
- Never paste prompts, completions, or API keys into incident channels.

## Recover

```bash
# Endpoint reachability only — do not print Authorization headers
curl -sS -o /dev/null -w '%{http_code}\n' "${MARKHAND_GLM_BASE_URL:-http://127.0.0.1:9}/health" || true
docker compose -f deploy/compose.poc.yml --env-file deploy/.env logs --tail=80 api \
  2>&1 | python3 deploy/scripts/redact_secrets.py
```

1. Confirm only top-K citations are sent (never full corpus).
2. Restore provider or keep extractive mode.
3. Rotate leaked provider credentials via [key-rotation](key-rotation.md).

## Verify

- Provider error ratio `< 0.5` for ≥5m; alert inactive.
- Ask returns cited extractive or validated GLM answers; audit has no prompts.
