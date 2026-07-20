# JWT signing and capability key rotation

Use this to rotate `MARKHAND_AUTH_SIGNING_KEY`, `MARKHAND_AUTH_KID`, or the download
capability key derived from the signing key.

## Detection

- Planned rotation, suspected secret exposure, or failed auth/capability validation.
- HTTP 401/503 spikes on authenticated routes.
- Logs with `invalid_token`, `expired`, `capability_unavailable` or auth provider
  initialization errors.
- Metrics: `markhand_http_requests_total{route,status}` for auth, document and
  download routes.

## Triage

Confirm the current runtime configuration without printing secret values:

```bash
docker compose -f deploy/compose.poc.yml logs --since=30m server
curl -fsS http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}/api/v1/health/ready
```

Server validation requires:

- `MARKHAND_AUTH_ISSUER`
- `MARKHAND_AUTH_AUDIENCE`
- `MARKHAND_AUTH_SIGNING_KEY` with at least 32 bytes
- `MARKHAND_AUTH_ALG=HS256`
- non-empty `MARKHAND_AUTH_KID`

The download capability key is derived from `MARKHAND_AUTH_SIGNING_KEY`; changing
the signing key invalidates existing download capabilities.

## Contain

1. If exposure is suspected, stop issuing new sessions by taking the server out of
   readiness or blocking auth ingress.
2. Do not log or paste old/new key material.
3. Assume existing access tokens and refresh tokens are invalid after an emergency
   signing-key rotation.
4. Existing download capability tokens are short-lived, single-use tokens. Wait at
   least 60 seconds or revoke ingress before rotating if you need a clean cutover.

## Recover

1. Generate a new signing key in the deployment secret store.
2. Set a new `MARKHAND_AUTH_KID` that is not a development/test value in production.
3. Update the server environment and restart:

   ```bash
   docker compose -f deploy/compose.poc.yml up -d server
   ```

4. Ask users to sign in again. Current POC configuration does not support overlapping
   JWT signing keys, so refresh-token continuity is not guaranteed across rotation.
5. Re-authorize downloads instead of retrying old capability URLs.

## Verify

```bash
curl -fsS http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}/api/v1/health/ready
curl -fsS -H "Authorization: Bearer $NEW_TOKEN" \
  http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}/api/v1/auth/me
```

- New login, refresh and `/api/v1/auth/me` succeed.
- Old access and refresh tokens fail.
- New document download authorization returns a fresh capability URL and old URLs
  fail after expiry or replay.
- Logs contain no signing key, bearer token or capability token values.
