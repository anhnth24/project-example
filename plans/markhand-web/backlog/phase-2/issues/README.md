# Phase 2 issues — Web SPA MVP

Parent plan: [`../../../phase-2-web-spa.md`](../../../phase-2-web-spa.md)

Issue ở **Backlog**. UI/mock có thể chạy sau OpenAPI 1B; final gate phụ thuộc Phase 1C.

## Dependency

```text
P2-01 → P2-02 → P2-03 → P2-04
                    └→ P2-05 → P2-06 → P2-07 → P2-08 → P2-09
                                      ├→ P2-10
                                      ├→ P2-11
                                      └→ P2-12
P2-01 + P2-07/10 → P2-13
P2-05 + P2-07..12 → P2-14
P2-02..14 → P2-15
P2-15 + Phase 1C gate → P2-16
```

## P2-01 — React/Vite workspace và UI foundations

- **Plan/files:** Tạo `web/` scripts/layout; copy browser-safe tokens/icons/primitives.
- **Depends:** Không. **Acceptance/tests:** Build/test độc lập; no Tauri import;
  typecheck/lint/unit/dependency-boundary; desktop vẫn xanh.
- **Security:** Dependency/license scan. **Out:** shared package/redesign desktop.

## P2-02 — OpenAPI contracts và mock server

- **Plan/files:** Pin generator; generated types; drift check; auth/org/library/job/
  Q&A/admin/error/SSE fixtures và mock scenarios.
- **Depends:** Stable 1B OpenAPI. **Acceptance/tests:** Drift fails CI; generated files
  immutable; fixture/schema/breaking-change tests; mock excluded production.
- **Out:** Chờ toàn bộ 1C mới làm UI.

## P2-03 — Typed HTTP client/session refresh

- **Plan/files:** Fetch wrapper, access token memory, refresh single-flight, one retry,
  normalized errors/request ID/quota, abort.
- **Depends:** P2-02. **Acceptance/tests:** Concurrent 401 một refresh; revoked refresh
  logout; race/loop/malformed/403/429/network/abort tests.
- **Security:** No token storage/log. **Out:** offline queue/Tauri IPC.

## P2-04 — Fetch-based SSE transport

- **Plan/files:** Streaming parser với bearer, Last-Event-ID, dedupe/gap, refresh/
  reconnect/backoff/snapshot, abort on scope change.
- **Depends:** P2-02/03. **Acceptance/tests:** Không native EventSource/token URL;
  chunk boundary/reconnect/order/revoke/cancel tests.
- **Security:** Bounded buffer/backoff, no content logs. **Out:** WebSocket.

## P2-05 — Login/session/application shell

- **Plan/files:** Router, auth bootstrap/login/protected shell/guards/logout/help stub.
- **Depends:** P2-01/03 + P1B-F05 browser refresh contract. **Acceptance/tests:**
  Intended route, expiry, guard matrix, login/refresh/logout component tests và
  integration CSRF/cookie-origin contract theo auth ADR.
- **Security:** HttpOnly/Secure/SameSite refresh + CSRF policy; server authority.
  **Out:** signup/reset/MFA/OIDC.

## P2-06 — Org switch và scope-safe state

- **Plan/files:** Org-scoped cache keys; atomic switch; abort REST/SSE; clear stores;
  scope generation ignores late response.
- **Depends:** P2-03…05 + backend 1C org APIs. **Acceptance/tests:** No old-org render;
  delayed/active-stream/rapid-switch/stale-membership tests.
- **Security:** No unapproved persisted tenant cache. **Out:** simultaneous org view.

## P2-07 — Library/list/sanitized preview

- **Plan/files:** Adapt browser-safe LibraryView; collection navigation, filter/page,
  status, preview states + SafeMarkdown.
- **Depends:** P2-02/03/05/06. **Acceptance/tests:** Stable URL/pagination; API-only
  preview; unsafe markdown, 403/404, switch-race tests.
- **Security:** No local path/public key. **Out:** desktop editor/compare.

## P2-08 — Upload progress và job lifecycle

- **Plan/files:** Multipart/progress/cancel; job SSE; reconnect snapshot; accessible
  status for uploaded→indexed/failed.
- **Depends:** P2-04/07. **Acceptance/tests:** Client/server progress distinct; recover
  refresh; success/cancel/loss/gap/413/415/429/filename tests.
- **Security:** No client conversion queue. **Out:** folder/watch/resumable protocol.

## P2-09 — Download/delete/reindex/retry

- **Plan/files:** Authorized actions, permission/confirm/conflict/idempotency handling.
- **Depends:** P2-07/08 + backend 1C guards. **Acceptance/tests:** Delete closes preview;
  server deny wins; confirm/concurrency/stale/signed-route tests.
- **Security:** No client-built object URLs; CSRF/idempotency. **Out:** purge policy.

## P2-10 — Streaming search/Q&A/citations

- **Plan/files:** Search/ask panel, index readiness, stream reducer, fallback warnings,
  citation deep-link, abort scope change.
- **Depends:** P2-04…07 + backend ACL. **Acceptance/tests:** `aria-live`; current source
  citation; sequence/fallback/no-answer/revoke/switch-mid-answer tests.
- **Security:** Sanitized Markdown/server route IDs. **Out:** intelligence/conversation memory.

## P2-11 — Member/role admin

- **Plan/files:** Member table/invite/suspend/role selector; owner restrictions from API.
- **Depends:** P2-02/03/05 + backend 1C-02…04. **Acceptance/tests:** Owner/admin matrix,
  last-owner conflict, invite/suspend/role/403/409/stale-update tests.
- **Security:** UI không hard-code matrix hay thay enforcement. **Out:** custom/group/SSO.

## P2-12 — Usage/quota/reservations

- **Plan/files:** Usage cards, limits, active reservations/jobs, actionable 429.
- **Depends:** P2-03/05 + backend 1C-09…11. **Acceptance/tests:** API numbers match;
  unit/timezone/403/429/stale tests.
- **Security:** No client-derived authority/cross-org usage. **Out:** billing.

## P2-13 — Browser/SafeMarkdown hardening

- **Plan/files:** CSP-compatible app, protocol allowlist, raw HTML/SVG/data URL denial,
  content bounds, header checks.
- **Depends:** P2-01/07/10. **Acceptance/tests:** Malicious corpus không execute; CSP
  browser/OWASP/dependency tests; no inline eval.
- **Security:** CSP/frame/nosniff/referrer/HSTS proxy. **Out:** WAF/pentest.

## P2-14 — Accessibility/interaction quality

- **Plan/files:** Skip/landmark/focus/keyboard/progress labels/contrast/reduced motion.
- **Depends:** P2-05/07…12. **Acceptance/tests:** No axe critical; keyboard primary
  flows; focus/reduced-motion/screen reader tests.
- **Security:** Error không đọc internal/token. **Out:** formal certification/i18n.

## P2-15 — Contract/integration/E2E suite

- **Plan/files:** Unit API/SSE/cache; component auth/library/Q&A/admin; Playwright full
  flows, org switch, deny/quota; CI artifacts redacted.
- **Depends:** P2-02…14; deployed integration cần 1C endpoints.
- **Acceptance/tests:** Mock deterministic + real deployment E2E; no stale-scope render;
  desktop regression.
- **Security:** Ephemeral users/credentials. **Out:** thay backend denial suite.

## P2-16 — Production build/static serving/final gate

- **Plan/files:** Hashed Vite assets; server/image dist; UI-only history fallback;
  API 404; HTML revalidate, immutable assets, headers.
- **Depends:** P2-13/15 + 1C-12/13.
- **Acceptance/tests:** Deep-link, cache/header/API404, packaged E2E, SLO, scans,
  desktop test/build.
- **Security:** Mock/source map policy; rollbackable immutable assets. **Out:** CDN/HA.

## Exit gate

Phase 2 chỉ đóng khi P2-16 đạt trên backend deploy thật và Phase 1C denial/security
gate đã pass; mock E2E không thay thế integration.
