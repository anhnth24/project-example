# Runbook: Credential leak / key rotation

Issue: P1B-O02  
Alert: `MarkhandAuthDenySpike` (and manual security trigger)  
Dashboard: Grafana `markhand-ops`  
Threshold: auth deny ratio > 20% for 10m (operational security signal documented in
`deploy/observability/thresholds.yaml`; not a G0 latency gate).

## Prerequisites

- Security on-call + ability to revoke sessions/API keys.
- Secret manager access. **Never** paste secrets, tokens, or signed URLs into
  tickets, chat, metrics, or git.

## Detection

1. Confirm `markhand:auth:deny_ratio_10m > 0.20` or credible leak report.
2. Check `markhand_auth_decisions_total` by bounded `code`
   (`unauthorized|invalid_credentials|permission_denied|...`).
3. Inventory which credential classes might be exposed (session, provider, object
   storage, DB) using secret-manager metadata only.

## Contain

1. Revoke/rotate the suspected credential immediately.
2. Invalidate sessions / disable affected service accounts.
3. Temporarily tighten admission (rate limits) if credential stuffing is active.
4. Preserve audit rows (append-only); do not attempt UPDATE/DELETE on `audit_log`.

## Recover

1. Issue replacement secrets via secret manager; update runtime config/reload.
2. Verify new credentials with `--check-config` / health probes (no secret echo).
3. Re-enable disabled accounts only after ownership is confirmed.
4. If object-storage signed URL scheme leaked, rotate signing key and invalidate
   outstanding URLs per storage policy.

## Verify

1. Auth deny ratio returns to baseline (< 0.20 and not spiking).
2. Legitimate smoke login/search succeeds; unauthorized still denied.
3. Canary secret strings absent from logs/metrics/audit metadata.
4. Record incident timeline without secret material.

## Rollback

- If rotation breaks a dependency, roll forward to a second new secret rather than
  re-using the leaked one.
- Keep the leaked credential revoked permanently.
- Re-disable Q&A/provider integration if provider key rotation is incomplete.

## Synthetic evidence

Fixture: `MarkhandAuthDenySpike.json`  
Tabletop: `tt-key-rotation` — synthetic deny-ratio evaluation only; no live leak.
