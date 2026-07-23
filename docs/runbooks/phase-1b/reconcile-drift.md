# Reconcile drift

## Detect

- Alert: `MarkhandReconcileDrift`
- Query:

```promql
increase(markhand_reconcile_drift_total[30m])
```

- Worker mode: `MARKHAND_RECONCILE_MODE` = `dry-run` | `repair` (default **dry-run**).
- Scope: set `MARKHAND_RECONCILE_DOCUMENT_ID=<uuid>` for a single document/job.
- Finite exit: `MARKHAND_WORKER_ONESHOT=1` (Compose service sets this).

## Contain

- Keep readiness false if an ops fence is active.
- Prefer dry-run before repair; do not delete across stores ad hoc.
- Dry-run **must not** complete the job or consume repair intent — the same
  pending job remains claimable for a later repair oneshot.
- Capture logs only through the redaction helper:

```bash
docker compose -f deploy/compose.poc.yml --env-file deploy/.env logs --tail=200 \
  worker-index 2>&1 | python3 deploy/scripts/redact_secrets.py
```

## Recover

Use the dedicated **private-network** oneshot service (`worker-reconcile-oneshot`).
Do **not** run reconcile against `worker-convert` (convert network has no Qdrant).

```bash
# 0) Identify the drifted document id (from alert context / operator notes).
DOC_ID="<document-uuid>"

# 1) Dry-run report — one job/document, finite exit.
#    Job returns to pending; attempts restored; no repair mutations.
# Prefer --no-deps when Postgres/MinIO/Qdrant are already healthy (avoids recreate races).
MARKHAND_RECONCILE_MODE=dry-run \
MARKHAND_RECONCILE_DOCUMENT_ID="$DOC_ID" \
docker compose -f deploy/compose.poc.yml --env-file deploy/.env \
  --profile reconcile-oneshot run --rm --no-deps worker-reconcile-oneshot \
  2>&1 | python3 deploy/scripts/redact_secrets.py

# Expect log line containing DryRunReported and finite exit after one cycle.
# Confirm job still pending (same idempotency key reconcile:<doc>:oneshot-scope).
# Missing/empty/malformed DOCUMENT_ID must exit non-zero before DB work.

# 2) Repair — only after dry-run report is acceptable.
MARKHAND_RECONCILE_MODE=repair \
MARKHAND_RECONCILE_DOCUMENT_ID="$DOC_ID" \
docker compose -f deploy/compose.poc.yml --env-file deploy/.env \
  --profile reconcile-oneshot run --rm --no-deps worker-reconcile-oneshot \
  2>&1 | python3 deploy/scripts/redact_secrets.py

# Expect Completed with repaired counts; process exits.

# 3) Idempotent clean — second repair should be NoJob or zero-drift Completed.
MARKHAND_RECONCILE_MODE=repair \
MARKHAND_RECONCILE_DOCUMENT_ID="$DOC_ID" \
docker compose -f deploy/compose.poc.yml --env-file deploy/.env \
  --profile reconcile-oneshot run --rm --no-deps worker-reconcile-oneshot \
  2>&1 | python3 deploy/scripts/redact_secrets.py
```

If `docker compose run` fails on cgroupv2 in a given host, equivalent:

```bash
docker run --rm --network markhand-poc_private \
  -e MARKHAND_WORKER_KIND=reconcile \
  -e MARKHAND_WORKER_ONESHOT=1 \
  -e MARKHAND_RECONCILE_MODE=dry-run \
  -e MARKHAND_RECONCILE_DOCUMENT_ID="$DOC_ID" \
  --env-file deploy/.env \
  markhand-worker:poc
```

(Pass the same DB/Qdrant/MinIO URL env vars the Compose service uses — hostnames
`postgres`, `qdrant`, `minio` resolve on `markhand-poc_private`.)

Notes:

- `--check-config` only validates configuration — it does **not** run detect/repair.
- PostgreSQL tombstones win for delete visibility.
- Never log document bodies, embeddings, or signing keys.

## Verify

- `increase(markhand_reconcile_drift_total[30m]) == 0`; alert inactive.
- Search/citation denial still holds for deleted docs.
