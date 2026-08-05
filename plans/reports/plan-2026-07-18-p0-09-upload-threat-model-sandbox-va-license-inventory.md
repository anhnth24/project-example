<!-- generated-done-issue-plan: P0-09 -->
# P0-09 — Upload threat model, sandbox và license inventory

Issue closed: 2026-07-18
Source issue: [#66](https://github.com/anhnth24/project-example/issues/66)
Catalog: [`backlog/phase-0/issues/README.md`](../markhand-web/backlog/phase-0/issues/README.md)
Phase plan: [`phase-0-discovery-and-gates.md`](../markhand-web/phase-0-discovery-and-gates.md)
Status: Done

## Objective

Security policy thực thi được trước khi nhận upload.

## Context

- Phase: `0`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> local-cpu policy/sandbox smoke evidence closes upload
> threat model, adversarial disposition, and runtime license inventory. This
> does not claim Profile B malware scanner coverage.

## Implementation plan

Threat model spoof/bomb/parser/SSRF/exhaustion/traversal/injection/token/
quota/tenant/compromised worker; chốt allowlist/limits/quarantine/sandbox; inventory
source/version/checksum/license.

## Files/modules

`docs/markhand-web-{upload-threat-model,upload-policy}.md`,
`docs/markhand-web-model-license-inventory.md`, adversarial disposition YAML.

## Dependencies / blocks

P0-02/P0-08 evidence available for local Phase 0
closure; production scanner/runtime hardening remains a later gate.

## Acceptance criteria

Mỗi threat có prevention/detection/owner; sandbox non-root,
read-only, no egress, resource/process/wall limits; unresolved model bị exclude.

## Required tests / evidence

`python3 bench/markhand_web/scripts/run_upload_security.py`
writes `bench/markhand_web/security/summary.json` and
`bench/markhand_web/reports/upload-security.md`; policy linter and in-process
sandbox smoke deny egress/traversal/fork/timeout. Runtime license checker
passes with PhoWhisper excluded/not bundled.

## Security and migration notes

GLM policy theo data classification.

## Out of scope

Production malware scanner.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-18T19:33:38Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
