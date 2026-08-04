<!-- generated-done-issue-plan: P1B-O04 -->
# P1B-O04 — Vertical-slice/security release suite

Issue closed: 2026-07-26
Source issue: [#100](https://github.com/anhnth24/project-example/issues/100)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Vertical-slice/security release suite**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> live release gate 2026-07-26 at `f4f33cd`:
> `o04-release.json` `status=pass` with no blockers, validated through
> `MARKHAND_RELEASE_GATE=1 cargo test -p fileconv-server --test e2e_release_suite`
> (3/3). Full workload format matrix observed (csv, docx, html, pdf, png OCR,
> pptx, txt, xlsx), black-box HTTP probes against the deployed Compose API,
> unauthorized/cross-tenant denial against real foreign-org resources,
> suspend/membership/delete deny, adversarial upload reject/contain, and
> structured external worker kill → lease expiry → reclaim → replay → DB
> verification. Provenance binds the F02 project, container ids and image ids.
> Report sha256 prefix `949e14202849cf8b`.

## Implementation plan

Clean stack, seed org/accounts; every format upload→citation; suspend/
membership remove/delete; adversarial + fault injection.

## Files/modules

`bench/markhand_web/scripts/run_o04_release_suite.py`,
`deploy/scripts/o04-release-suite.sh`,
`crates/server/tests/{e2e_release_suite,retrieval_vertical_slice}.rs`,
`docs/runbooks/phase-1b/release-suite-o04.md`,
`bench/markhand_web/reports/phase-1b-gate/o04-release.*`.

## Dependencies / blocks

F01–R06 + G0-SEC/G1A.

## Acceptance criteria

All workload formats pass; unauthorized gets no text;
malicious rejected/contained; worker kill consistent; evidence redacted;
self-test rejects multi-filter command shapes +
missing/skipped/ignored/zero-test/partial/high-critical/F02 mismatch.

## Required tests / evidence

All workload formats pass; unauthorized gets no text;
malicious rejected/contained; worker kill consistent; evidence redacted;
self-test rejects multi-filter command shapes +
missing/skipped/ignored/zero-test/partial/high-critical/F02 mismatch.

## Security and migration notes

High/critical blocks release.

## Out of scope

full 1C matrix.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- `949e14202849cf8b`
- `f4f33cd`

- GitHub sync-closed timestamp: `2026-07-26T14:40:19Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
