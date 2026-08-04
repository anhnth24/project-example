<!-- generated-done-issue-plan: P1B-F02 -->
# P1B-F02 — POC deployment và isolation scaffold

Issue closed: 2026-07-26
Source issue: [#79](https://github.com/anhnth24/project-example/issues/79)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **POC deployment và isolation scaffold**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> live boot evidence regenerated 2026-07-26 on a 24-core
> Ubuntu 22.04 host from a clean tree at `f4f33cd`:
> `poc-f02-boot.json` `passed=true`, 81 checks / 0 fails, project
> `markhand-poc-f02-20260726t121843z-1815269-17292`, clean project boot measured,
> nonzero mem/cpu/pids on every service, convert network `Internal=true` with an
> executable egress probe, narrow MinIO credential positive/negative probes, and
> the full native format smoke matrix. Report sha256 prefix `9d7214df30e57a95`.
> The run wrote its evidence outside the tree so the gate could bind to a clean
> worktree; the accepted run was then copied in, with only the recorded raw
> directory paths rewritten to repository-relative form.

## Implementation plan

Pinned API/converter/index images, compose services, health/init, non-root,
read-only, tmpfs, dropped caps, converter no-egress, resource/secret limits.

## Files/modules

`deploy/{Dockerfile.server,Dockerfile.worker,compose.poc.yml,.env.example}`,
`deploy/scripts/poc-*.sh`, `deploy/scripts/poc_f02_boot_evidence.py`, `deploy/poc/*`,
`deploy/README.md`.

## Dependencies / blocks

F01 + G0-CAP/G0-SEC/G0-LIC.

## Acceptance criteria

Clean host boot tự động; API/worker image tách; isolation/
UID/cap/egress/native format smoke tests; `poc-boot-evidence.sh --self-test`.

## Required tests / evidence

Clean host boot tự động; API/worker image tách; isolation/
UID/cap/egress/native format smoke tests; `poc-boot-evidence.sh --self-test`.

## Security and migration notes

Narrow MinIO credentials, no bundled unlicensed model.

## Out of scope

Kubernetes/HA.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- `9d7214df30e57a95`
- `f4f33cd`

- GitHub sync-closed timestamp: `2026-07-26T14:39:44Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
