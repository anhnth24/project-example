# P1B-O04 — Vertical-slice / security release suite

Live E2E harness against the real POC Compose stack and `/api/v1` routes.
**Public production APIs only** — no test-only DB/MinIO intake bridge.

## Modes

| Mode | Entry | Docker | Purpose |
|---|---|---|---|
| Hermetic | `python3 scripts/check-e2e-o04.py` | No | Manifest/schema/fixture/redaction/contract checks |
| Live | `deploy/scripts/poc-e2e-o04.sh` | Yes | Format vertical slice + security + fault matrices |

Static CI only runs hermetic validation. Live never silently skips: missing
prerequisites fail the invoked command.

## Production intake requirement

After `POST /api/v1/uploads`, the suite **requires** production identities:

- `documentId`
- `versionId`
- `jobId`

(optionally nested under `intake`). `objectId` alone is **not** enough. There is no
supported follow-up public API that promotes an objectId into a document/job today.

If those fields are absent, the live suite records high/critical
`production_intake_not_wired` and **fails immediately** (no DB inserts, no MinIO
metadata rewrites, no bridge).

Exact postconditions only: search/ask/citation must match the **same**
documentId/versionId and the fixture token — never “any hit”.

## Safety gates (live)

Live runs **must** satisfy all of:

1. `MARKHAND_E2E_CONFIRM=i-understand-this-mutates-only-tagged-test-stacks`
2. Compose project name contains `e2e` or `test` (`MARKHAND_COMPOSE_PROJECT`)
3. Postgres DB name and MinIO bucket name contain `e2e` or `test`
4. Stack tagged via `MARKHAND_E2E_STACK_TAG=test` in `deploy/.env`

## Layout

```text
crates/server/tests/e2e/
  manifest.json           # suite cases (formats / security / fault)
  schema/                 # evidence + manifest JSON schemas
  fixtures/               # deterministic Vietnamese synthetic files
  harness/                # API client, runner, redaction, matrices
  sql/                    # account seed helpers (not intake bridge)
  scripts/                # hermetic + live entrypoints
```

## Optional formats

| Capability | Classification | Behavior |
|---|---|---|
| pdf/docx/pptx/xlsx/csv/html/txt/image OCR | required | live fail if broken |
| audio | optional **only if** server explicitly disables audio | `optional_unavailable`, never a pass claim |

Image OCR fixture is a high-contrast rendered token bitmap (not a blank PNG).

## Evidence hygiene

Committed / CI evidence must never contain: raw bucket keys, document text, prompts,
passwords, tokens, signed URLs, or tenant IDs. Use opaque per-run IDs.
`claimsLiveVerticalSlice` stays false until a real tagged-stack pass with wired intake.
High/critical findings block release.
