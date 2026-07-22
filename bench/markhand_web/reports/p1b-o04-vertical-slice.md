# P1B-O04 evidence — vertical-slice / security release suite

Status mode: `hermetic` · `claimsLiveVerticalSlice`: **false**

Identity: hermetic deterministic (no runtime timestamp / branch / HEAD).
Run id (stable): `00000000-0000-4000-8000-000000000004`

Severity: `high`

## Summary

- passed: 1
- failed: 0
- blocked: 31
- optional unavailable: 1
- high/critical cases: 8

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
- `fmt-audio-spoken` [format] → **optional_unavailable** (severity=none; blocker=none; http=[])
  - optional spoken-audio coverage requires approved spoken-token fixture/model; silence cannot satisfy all-formats claim
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
- `sec-[REDACTED_FIXTURE_TOKEN]` [security] → **blocked** (severity=medium; blocker=none; http=[])
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
- `adv-audio-silence-no-hallucination` [adversarial] → **blocked** (severity=medium; blocker=none; http=[])
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
- Runtime live evidence is written to a gitignored `.live` artifact;
  the tracked hermetic report stays deterministic.
