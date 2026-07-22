# Runbook: Credential leak / key rotation

Issue: P1B-O02
Alert: `MarkhandAuthDenySpike`
Dashboard: `markhand-ops`
Threshold: **O02-OPS-AUTH-DENY-COUNT** (>50 deny decisions in 10m) — operational policy, **not** SLA-derived.

## Prerequisites

- Access to secret manager / `deploy/.env` on the operator host (never commit secrets)
- Relevant keys: `MARKHAND_AUTH_SIGNING_KEY`, MinIO keys, embedding API key, GLM provider key
- Audit log is append-only — do not attempt UPDATE/DELETE
- When observability is in use, keep the overlay attached so API OTLP env merge is preserved

## Detection

```bash
REPO_ROOT="$(git rev-parse --show-toplevel)"
export REPO_ROOT
export POC_WITH_OBSERVABILITY=1
# shellcheck source=deploy/scripts/poc-compose.sh
source "$REPO_ROOT/deploy/scripts/poc-compose.sh"
poc_compose_init

curl -fsG http://127.0.0.1:9090/api/v1/query \
  --data-urlencode 'query=markhand:auth:deny_increase_10m'
"${COMPOSE[@]}" logs --tail=200 api | grep -E 'auth|deny' || true
# IDs/codes only — no tokens/passwords
```

## Contain

1. Revoke/rotate the suspected credential in the secret manager immediately.
2. Recreate API with the observability overlay still attached (preserves OTLP merge):

```bash
# Update sealed values in deploy/.env (local) or secret store (prod) — do not echo secrets
# POC_WITH_OBSERVABILITY=1 must already be set before poc_compose_init (see Detection).
"${COMPOSE[@]}" up -d --no-deps api

# Exact verification — OTLP env must remain present after recreate
"${COMPOSE[@]}" exec -T api sh -c \
  'printf "MARKHAND_OTEL_EXPORTER=%s\nMARKHAND_OTEL_EXPORTER_OTLP_ENDPOINT=%s\nMARKHAND_OTEL_METRICS_ENABLED=%s\n" \
    "$MARKHAND_OTEL_EXPORTER" \
    "$MARKHAND_OTEL_EXPORTER_OTLP_ENDPOINT" \
    "$MARKHAND_OTEL_METRICS_ENABLED"'
# expected:
#   MARKHAND_OTEL_EXPORTER=otlp
#   MARKHAND_OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4317
#   MARKHAND_OTEL_METRICS_ENABLED=true
```

Alternatively use the supported wrapper (also keeps the overlay):

```bash
bash "$REPO_ROOT/deploy/observability/up.sh" up -d --no-deps api
```

3. Tighten admission / rate limits if credential stuffing is active (R06 middleware already enforces limits).

## Recover

1. Config check (no secret echo):

```bash
# From a workstation with env loaded — never pipe secrets to logs
cargo run -p fileconv-server -- --check-config
```

2. Health:

```bash
"$REPO_ROOT/deploy/scripts/poc-health.sh"
curl -fsS "http://127.0.0.1:${MARKHAND_API_PORT:-8788}/api/v1/health/ready"
```

3. Re-enable disabled accounts only after ownership is confirmed.
4. If object-storage signing leaked: rotate MinIO app keys via `deploy/poc/minio-init.sh` flow and recreate `minio-init` — coordinate carefully; escalate if unsure.

## Verify

1. `markhand:auth:deny_increase_10m` back under 50.
2. Legitimate login/search smoke succeeds; unauthorized still denied.
3. Canary secrets absent from logs/metrics/audit metadata.
4. OTLP env check from Contain still prints `otlp` / `http://otel-collector:4317` / `true`.

## Rollback

- Roll **forward** to a second new secret rather than reusing a leaked one.
- Keep the leaked credential revoked permanently.

## Synthetic evidence

Promtool `auth_deny_count_threshold`. No live leak claimed.
