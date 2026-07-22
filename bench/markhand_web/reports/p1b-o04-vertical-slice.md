# P1B-O04 evidence — vertical-slice / security release suite

Status mode: `hermetic` · `claimsLiveVerticalSlice`: **false**

Run id (opaque): `d07b8ddd-6beb-47d5-86ca-befc739b949d`
Git: `cursor/implement-p1b-o04-5007` @ `8428c69fc7b7`
Severity: `high`

## Summary

- passed: 1
- failed: 0
- blocked: 31
- optional unavailable: 0
- high/critical cases: 9

## Blockers

- Hermetic harness validation only — not a live vertical-slice pass
- Docker/Compose unavailable — live suite not executed
- production_intake_not_wired — current /api/v1/uploads returns objectId only (no documentId/versionId/jobId; no supported follow-up public API)
- claimsLiveVerticalSlice remains false

## Cases

- `harness-manifest` [harness] → **pass** (severity=none; blocker=none; http=[])
  - hermetic harness validation only — not a live vertical-slice pass
- `fmt-txt` [format] → **blocked** (severity=high; blocker=production_intake_not_wired; http=[])
  - blocked: Docker unavailable; production intake wiring unverified
- `fmt-html` [format] → **blocked** (severity=high; blocker=production_intake_not_wired; http=[])
  - blocked: Docker unavailable; production intake wiring unverified
- `fmt-csv` [format] → **blocked** (severity=high; blocker=production_intake_not_wired; http=[])
  - blocked: Docker unavailable; production intake wiring unverified
- `fmt-pdf` [format] → **blocked** (severity=high; blocker=production_intake_not_wired; http=[])
  - blocked: Docker unavailable; production intake wiring unverified
- `fmt-docx` [format] → **blocked** (severity=high; blocker=production_intake_not_wired; http=[])
  - blocked: Docker unavailable; production intake wiring unverified
- `fmt-pptx` [format] → **blocked** (severity=high; blocker=production_intake_not_wired; http=[])
  - blocked: Docker unavailable; production intake wiring unverified
- `fmt-xlsx` [format] → **blocked** (severity=high; blocker=production_intake_not_wired; http=[])
  - blocked: Docker unavailable; production intake wiring unverified
- `fmt-image-ocr` [format] → **blocked** (severity=high; blocker=production_intake_not_wired; http=[])
  - blocked: Docker unavailable; production intake wiring unverified
- `fmt-audio` [format] → **blocked** (severity=high; blocker=production_intake_not_wired; http=[])
  - blocked: Docker unavailable; production intake wiring unverified
- `sec-user-disabled` [security] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `sec-user-suspended` [security] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `sec-membership-removed` [security] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `sec-collection-acl-revoke` [security] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `sec-tombstone-during-query` [security] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `sec-tombstone-during-stream` [security] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `sec-historical-permission-revoke` [security] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `sec-malformed-ids` [security] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `sec-malformed-body` [security] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `sec-malformed-cursors` [security] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `sec-malformed-last-event-id` [security] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `sec-idor-cross-org` [security] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `sec-prompt-injection-untrusted` [security] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `sec-zip-bomb` [security] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `sec-path-traversal` [security] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `sec-extension-spoof` [security] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `sec-oversize` [security] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `sec-malformed-format` [security] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `fault-kill-convert-after-claim` [fault] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `fault-kill-convert-after-checkpoint` [fault] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `fault-kill-index-after-claim` [fault] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment
- `fault-dependency-outage-bounded` [fault] → **blocked** (severity=medium; blocker=none; http=[])
  - live stack unavailable in this environment

## Non-claims

- Hermetic mode validates harness/fixtures/gates only.
- This report never embeds document text, prompts, tokens, passwords,
  signed URLs, raw object keys, or tenant IDs.
- Live vertical-slice pass requires Docker POC stack + confirm gates
  **and** production upload→documentId/versionId/jobId wiring.
