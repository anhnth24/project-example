<!-- generated-done-issue-plan: P2-13 -->
# P2-13 — Browser/SafeMarkdown hardening

Date: 2026-08-04
Source issue: [#128](https://github.com/anhnth24/project-example/issues/128)
Catalog: [`backlog/phase-2/issues/README.md`](../markhand-web/backlog/phase-2/issues/README.md)
Phase plan: [`phase-2-web-spa.md`](../markhand-web/phase-2-web-spa.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Browser/SafeMarkdown hardening**.

## Context

- Phase: `2`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> SafeMarkdown + sanitize allowlist + content bound ở #311; CSP/frame/nosniff/referrer landed cùng P2-16 (#313). HSTS để cho reverse proxy (không set ở app).

## Implementation plan

CSP-compatible app, protocol allowlist, raw HTML/SVG/data URL denial,
content bounds, header checks.

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

P2-01/07/10.

## Acceptance criteria

Malicious corpus không execute; CSP
browser/OWASP/dependency tests; no inline eval.

## Required tests / evidence

Malicious corpus không execute; CSP
browser/OWASP/dependency tests; no inline eval.

## Security and migration notes

CSP/frame/nosniff/referrer/HSTS proxy.

## Out of scope

WAF/pentest.

## Delivery evidence

### Implementation PRs

- [PR #311](https://github.com/anhnth24/project-example/pull/311) — Web wave 0 remainder and wave 1: client, SSE, mocks, login shell, scope-safe org switch; merged `2026-07-27T03:09:05Z`
- [PR #313](https://github.com/anhnth24/project-example/pull/313) — Web accessibility pass and SPA static serving (P2-14, P2-16); merged `2026-07-27T08:32:24Z`

### Recorded commit/SHA references

- `370c8f738af25f8becb4ecde709057b4ed70a8d4`
- `85991a1bfbf156b48a1d7af68b0088880c866f7f`

- GitHub sync-closed timestamp: `2026-07-27T09:49:31Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
