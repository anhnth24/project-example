# Credential / signing key rotation

## Detect
- Suspected leak, scheduled rotation, or auth anomaly.

## Contain
- Revoke refresh families for affected users.
- Rotate MinIO/Qdrant/DB credentials in secret store first.

## Recover
1. Issue new `MARKHAND_AUTH_SIGNING_KEY` (HS256 ≥ 32 bytes).
2. Rolling restart API; existing access tokens expire naturally (≤15m).
3. Rotate object-store keys used by API/worker roles (narrow policies).

## Verify
- Login/refresh works; old signing key rejects; no secrets in logs/audit.
