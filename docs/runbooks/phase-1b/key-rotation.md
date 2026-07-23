# Credential / signing key rotation

## Detect

- Suspected leak, scheduled rotation, or auth anomaly (audit deny spikes).
- Related outage alerts may fire if rotated credentials are not rolled out together.

## Contain

- Revoke refresh families for affected users (admin API / DB procedure — no secret echo).
- Rotate MinIO/Qdrant/DB credentials in the secret store **first**.
- Do not commit keys; do not paste `MARKHAND_*` secret values into tickets or Grafana annotations.

## Recover

1. Issue a new `MARKHAND_AUTH_SIGNING_KEY` (HS256 ≥ 32 bytes) via secret manager.
2. Rolling restart API; existing access tokens expire naturally (≤15m).
3. Rotate object-store keys used by API/worker roles (narrow policies).
4. Update Compose/ops env from the secret store; restart dependents:

```bash
docker compose -f deploy/compose.poc.yml up -d api worker-convert worker-embedding worker-index
curl -sS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8788/api/v1/health/live
```

## Verify

- Login/refresh works with new key material; old signing key rejects.
- `rg -n 'MARKHAND_AUTH_SIGNING_KEY=|BEGIN .*PRIVATE' deploy logs` finds no leaked values in committed files or redacted logs.
