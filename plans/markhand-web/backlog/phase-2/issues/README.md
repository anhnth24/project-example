# Phase 2 issues — Web SPA MVP

Parent plan: [`../../../phase-2-web-spa.md`](../../../phase-2-web-spa.md)

<!-- roadmap-default-status: backlog -->

**Trạng thái tổng quan (cập nhật 2026-08-06).** Phase 2 có **23 issue** sau khi owner
duyệt thiết kế full real-E2E: **15 Done**, **3 In progress** (P2-16/P2-18/P2-19),
**4 Blocked** (P2-15/P2-21/P2-22/P2-23), và **1 Backlog draft** (P2-20, chờ trigger
duyệt canonical draft để chuyển `Ready`). P2-15 là umbrella, chỉ `Done` sau P2-20…23;
P2-16 vẫn là final gate. P2-10/P2-17 đã qua independent review và `Done`.

> Ranh giới quan trọng: "Done" ở đây nghĩa là **hành vi client đã build và test trên
> mock/deterministic**, đã qua CI (`web`, `web-e2e`, `rust`, `rust-integration`) trên
> `master`. **Exit gate của cả Phase 2 vẫn CHƯA đạt** — nó đòi E2E trên backend deploy
> thật + gate denial/security của Phase 1C (xem mục "Exit gate" cuối trang). Mock E2E
> không thay integration.

Truy vết merge: **#311** (P2-01…06 — foundations, client, SSE, mock, login, org switch),
**#312** (P2-07…09 — library, upload, actions), **#313** (P2-14, P2-16 — a11y, serve SPA),
**#317** (P2-11, P2-12 — member/usage admin, trên lát membership API),
**#318** (P2-15 nửa mock-based — Playwright E2E + job `web-e2e`),
**#374** (P2-10 conflict-warning demo đa chế độ; P2-15 real-mode upload E2E + 3 job
security scan; P2-17 graph→document deep-link; P2-18 spec 409 PATCH; 1C-12 fixture/test
+ hạ tầng gate 1C — kèm loạt fix CI: repin canonical gates SHA, cargo/pnpm audit
exception có hồ sơ, Trivy `ignore-unfixed`, 2 flaky test integration, contract drift).
**#377/#379** sửa worker harness và chứng minh real E2E login/library/upload→indexed
3/3 trên master ancestry. **#378** chạy ZAP live ba lần; run cuối 0 fail, 66 pass, còn
một alert informational `10049`. Thiết kế owner duyệt ngày 2026-08-06:
[`P2-15 full real-E2E`](../../../../../docs/superpowers/specs/2026-08-06-p2-15-full-real-e2e-design.md).
P2-13 đi cùng wave 0 (#311); phần CSP/header của nó thực tế landed ở P2-16 (#313).

## Dependency

```text
P2-01 → P2-02 → P2-03 → P2-04
                    └→ P2-05 → P2-06 → P2-07 → P2-08 → P2-09
                                      ├→ P2-10
                                      ├→ P2-11
                                      └→ P2-12
P2-01 + P2-07/10 → P2-13
P2-05 + P2-07..12 → P2-14
P2-02..14 → P2-20
P2-18 + P2-19 + P2-20 → P2-21
P2-18 + 1C-12 + P2-20 → P2-22
P2-21 + P2-22 + 1C-13 → P2-23
P2-20 + P2-21 + P2-22 + P2-23 → P2-15
P2-15 + Phase 1C gate → P2-16
```

## P2-01 — React/Vite workspace và UI foundations

- **Status:** Done — #311. Web workspace, no Tauri import, CI job `web` xanh.

- **Plan file:** [P2-01 detailed implementation plan](../../../../reports/plan-2026-07-27-p2-01-react-vite-workspace-va-ui-foundations.md)
- **Plan/files:** Tạo `web/` scripts/layout; copy browser-safe tokens/icons/primitives.
- **Depends:** Không. **Acceptance/tests:** Build/test độc lập; no Tauri import;
  typecheck/lint/unit/dependency-boundary; desktop vẫn xanh.
- **Security:** Dependency/license scan. **Out:** shared package/redesign desktop.

## P2-02 — OpenAPI contracts và mock server

- **Status:** Done — #311. Generated types + `pnpm api:check` drift gate; mock server sinh từ OpenAPI.

- **Plan file:** [P2-02 detailed implementation plan](../../../../reports/plan-2026-07-27-p2-02-openapi-contracts-va-mock-server.md)
- **Plan/files:** Pin generator; generated types; drift check; auth/org/library/job/
  Q&A/admin/error/SSE fixtures và mock scenarios.
- **Depends:** Stable 1B OpenAPI. **Acceptance/tests:** Drift fails CI; generated files
  immutable; fixture/schema/breaking-change tests; mock excluded production.
- **Security/migration:** N/A — không thay đổi persisted schema; fixtures synthetic,
  không chứa token/PII thật.
- **Out:** Chờ toàn bộ 1C mới làm UI.

## P2-03 — Typed HTTP client/session refresh

- **Status:** Done — #311. Refresh single-flight; ADR 0010 rotation; concurrent-401/revoke tests.

- **Plan file:** [P2-03 detailed implementation plan](../../../../reports/plan-2026-07-27-p2-03-typed-http-client-session-refresh.md)
- **Plan/files:** Fetch wrapper, access token memory, refresh single-flight, one retry,
  normalized errors/request ID/quota, abort.
- **Depends:** P2-02. **Acceptance/tests:** Concurrent 401 một refresh; revoked refresh
  logout; race/loop/malformed/403/429/network/abort tests.
- **Security:** No token storage/log. **Out:** offline queue/Tauri IPC.

## P2-04 — Fetch-based SSE transport

- **Status:** Done — #311. Fetch-based (không EventSource), Last-Event-ID, reconnect/backoff, abort-on-scope.

- **Plan file:** [P2-04 detailed implementation plan](../../../../reports/plan-2026-07-27-p2-04-fetch-based-sse-transport.md)
- **Plan/files:** Streaming parser với bearer, Last-Event-ID, dedupe/gap, refresh/
  reconnect/backoff/snapshot, abort on scope change.
- **Depends:** P2-02/03. **Acceptance/tests:** Không native EventSource/token URL;
  chunk boundary/reconnect/order/revoke/cancel tests.
- **Security:** Bounded buffer/backoff, no content logs. **Out:** WebSocket.

## P2-05 — Login/session/application shell

- **Status:** Done — #311. Bearer refresh trong memory (không cookie/CSRF); router + guard matrix.

- **Plan file:** [P2-05 detailed implementation plan](../../../../reports/plan-2026-07-27-p2-05-login-session-application-shell.md)
- **Plan/files:** Router, auth bootstrap/login/protected shell/guards/logout/help stub.
- **Depends:** P2-01/03 + P1B-F05 browser refresh contract. **Acceptance/tests:**
  Intended route, expiry, guard matrix, login/refresh/logout component tests và
  integration CSRF/cookie-origin contract theo auth ADR.
- **Security:** Transport theo auth ADR. Nếu chọn cookie: HttpOnly/Secure/SameSite +
  CSRF/Origin contract; nếu chọn bearer refresh: không cookie/CSRF nhưng token không
  được persist/log. Server luôn là authority. **Out:** signup/reset/MFA/OIDC.

## P2-06 — Org switch và scope-safe state

- **Status:** Done — #311. Scope epoch; `useScopeSafeRequest`/`useScopeSafeSse` bỏ response scope cũ.

- **Plan file:** [P2-06 detailed implementation plan](../../../../reports/plan-2026-07-27-p2-06-org-switch-va-scope-safe-state.md)
- **Plan/files:** Org-scoped cache keys; atomic switch; abort REST/SSE; clear stores;
  scope generation ignores late response.
- **Depends:** P2-03…05 + backend 1C org APIs. **Acceptance/tests:** No old-org render;
  delayed/active-stream/rapid-switch/stale-membership tests.
- **Security:** No unapproved persisted tenant cache. **Out:** simultaneous org view.

## P2-07 — Library/list/sanitized preview

- **Status:** Done — #312. Collection nav, filter + cursor pagination thật, preview qua SafeMarkdown. Không có endpoint cross-collection nên "tất cả bộ sưu tập" chỉ điều hướng.

  **Cập nhật (URL param cho tài liệu đang mở, 2026-07-29):** tài liệu đang chọn chuyển từ
  state cục bộ (`ViewState.selectedDocumentId`) sang query param thật trên URL
  (`/library/:collectionId?doc=<documentId>` — `RouterProvider`'s `searchParams`, mới
  thêm cạnh `pathname`/`match`). Reload giữ nguyên tài liệu đang mở (route param được
  parse lại từ URL, không phải từ state đã mất); back/forward hoạt động qua
  `popstate` có sẵn của `RouterProvider`, không cần code riêng. `?doc=` có thể trỏ tới
  một tài liệu không nằm trên trang hiện tại (deep-link từ citation hoặc reload không
  giữ vị trí phân trang) — rơi vào trường hợp đó thì `LibraryPage` gọi thẳng
  `GET /documents/{documentId}` để lấy tài liệu, thay vì chỉ tìm trong `items` của trang
  đang tải. Đổi trang (`goToPrevPage`/`goToNextPage`) xoá `?doc=` (điều hướng, không chỉ
  set state) vì tài liệu đã chọn thuộc trang cũ. Đây cũng là điều kiện để đóng gap
  citation deep-link của P2-10 (xem mục đó): `CitationCard` dựng đúng path này.
  Thêm hướng dẫn/nút mở nhanh bộ sưu tập đầu tiên trên màn "Tất cả bộ sưu tập" (điều
  hướng thôi, không phải panel upload cross-collection — giữ đúng giới hạn ghi ở dòng
  Status phía trên).

- **Plan file:** [P2-07 detailed implementation plan](../../../../reports/plan-2026-07-27-p2-07-library-list-sanitized-preview.md)
- **Plan/files:** Adapt browser-safe LibraryView; collection navigation, filter/page,
  status, preview states + SafeMarkdown; unresolved conflict badge/count, side-by-side
  cited BA/design/dev claims và resolved-history link.
- **Depends:** P2-02/03/05/06. **Acceptance/tests:** Stable URL/pagination; API-only
  preview; unsafe markdown, 403/404, switch-race tests. **+ ?doc= param**: select
  pushes URL; deep-link preselects + previews; reload (fresh mount, same URL) keeps
  selection; back/forward moves selection (`LibraryPage.test.tsx`'s "P2-07 URL param"
  suite); E2E citation→preview (`qa.spec.ts`).
- **Security:** No local path/public key. **Out:** desktop editor/compare.

## P2-08 — Upload progress và job lifecycle

- **Status:** Done — #312. XHR progress thật, job SSE-nudge → `GET /jobs/{id}`. Không quan sát được stage index (API chỉ trả convert job id) — báo "converted, indexing tiếp server-side".

  **Cập nhật (badge tự cập nhật khi tài liệu đang xử lý, 2026-07-29 — owner critique:
  "trạng thái document chưa đúng giai đoạn xử lý khi load lại trang hoặc mở chức năng
  khác rồi quay lại"):** `LibraryPage` trước đó chỉ `refreshDocuments()` (refetch
  `GET /collections/{id}/documents`) khi có hành động rõ ràng (upload xong, bấm action)
  — một tài liệu worker đang chuyển `converting → converted → indexing → indexed` phía
  server đứng im trên UI tới khi F5. Đóng bằng cách poll đúng request đó (không chế
  transport mới, vẫn qua `useScopeSafeRequest` sẵn có) mỗi 5s khi trang hiện tại có ≥1
  tài liệu ở trạng thái non-terminal (`uploaded|converting|converted|indexing`); dừng khi
  hết non-terminal, tab ẩn (`document.visibilityState`/`visibilitychange`), rời trang,
  hoặc đổi org/collection (đã "miễn phí" theo scope-safety sẵn có của
  `documentsData`/`retainedDocuments`). Backoff 5s→15s→30s khi lỗi liên tiếp, reset khi
  thành công. Preview panel đang mở tài liệu đó cũng cập nhật theo (đọc lại từ cùng danh
  sách đã poll, không cần dây riêng). Mock seam: `__markhandMockDocs.advance(documentId)`
  (`components/library/testSupport.ts`'s `advanceDocumentState`, quy ước
  `__markhandMock*` sẵn có) tiến 1 bước forward-only
  (`uploaded→converting→converted→indexing→indexed`) — không tự chế "failed" (đó là một
  outcome thật, không phải bước tiến). Test: component `LibraryPage.test.tsx` (fake
  timers — bật/tắt theo non-terminal, tắt khi tab ẩn, backoff khi lỗi 429) + E2E mới
  `e2e/document-status-polling.spec.ts` (upload → converting → advance seam 3 lần →
  badge tự chuyển converted/indexing/indexed, không `page.reload()`/`page.goto()`).

- **Plan file:** [P2-08 detailed implementation plan](../../../../reports/plan-2026-07-27-p2-08-upload-progress-va-job-lifecycle.md)
- **Plan/files:** Multipart/progress/cancel; job SSE; reconnect snapshot; accessible
  status for uploaded→indexed/failed.
- **Depends:** P2-04/07. **Acceptance/tests:** Client/server progress distinct; recover
  refresh; success/cancel/loss/gap/413/415/429/filename tests. **+ live status poll**:
  xem cập nhật ở trên.
- **Security:** No client conversion queue. **Out:** folder/watch/resumable protocol.

## P2-09 — Download/delete/reindex/retry

- **Status:** Done — #312. Capability issue+redeem, delete tombstone có confirm. Không có endpoint retry-convert nên "thử lại" = reindex (ghi rõ trong UI).

- **Plan file:** [P2-09 detailed implementation plan](../../../../reports/plan-2026-07-27-p2-09-download-delete-reindex-retry.md)
- **Plan/files:** Authorized actions, permission/confirm/conflict/idempotency handling.
- **Depends:** P2-07/08 + backend 1C guards. **Acceptance/tests:** Delete closes preview;
  server deny wins; confirm/concurrency/stale/signed-route tests.
- **Security:** No client-built object URLs; CSRF/idempotency. **Out:** purge policy.

## P2-10 — Streaming search/Q&A/citations

- **Status:** Done — scope-wide `as_of` closure [#393](https://github.com/anhnth24/project-example/pull/393);
  independent task and whole-branch re-reviews approved with no remaining finding.
- **Plan file:** [P2-10 `as_of` end-to-end closure](../../../../reports/plan-2026-08-06-p2-10-as-of-e2e.md)
- **Objective:** Cung cấp trải nghiệm tìm kiếm/Hỏi đáp chat-first trên contract
  `search`/`ask`/`ask/stream`, với citation có thể kiểm chứng, version context và
  fail-closed collection/project authorization. `as_of` giữ semantics backend
  scope-wide: timestamp áp dụng cho mọi tài liệu trong scope đã authorize, không có
  document picker giả.
- **Implementation plan:**
  1. Dùng generated API client và P2-04 SSE transport cho search, ask đồng bộ và
     `POST /ask/stream`; reducer giữ thứ tự/dedupe sự kiện và trạng thái terminal.
  2. Giữ `QaPage` chat-first: nhiều turn độc lập, cancel giữ partial answer, scope epoch
     xoá state cũ khi đổi org; persisted sidebar thuộc P2-19 và không được gửi lại làm
     conversation context.
  3. Resolve request scope từ active token/profile allow-list, rồi intersect project
     union; explicit collection ngoài allow-list, mixed scope và empty scope đều 403.
  4. Render citation thành inline footnote + khối nguồn, dùng
     `logicalDocumentId`/`versionId`/`collectionId` và `documentTitle` để deep-link
     preview/version; SafeMarkdown tiếp tục sanitize nội dung.
  5. Hỗ trợ `current`/`as_of`/`compare`/`history`; cảnh báo version không hiện hành và
     conflict đã resolved. `as_of` yêu cầu RFC3339 timestamp, chọn latest effective
     version cho từng document trong scope, rồi áp dụng normal matching + limit.
- **Files/modules:**
  - `web/src/pages/QaPage.tsx`, `web/src/components/qa/**`: chat/search UI, project
    picker, turn lifecycle, footnote citation và preview navigation.
  - `web/src/state/askStream.ts`, `web/src/api/sse.ts`: ordered stream reduction,
    reconnect/terminal handling và cancellation.
  - `web/src/mocks/handlers/qa.ts`, `web/src/mocks/fixtures.ts`: deterministic
    search/ask/SSE scenarios, authorization scope và multi-version budget fixture.
  - `web/src/pages/QaPage.test.tsx`, `web/src/state/askStream.test.ts`,
    `web/src/mocks/handlers/qa.asOf.test.ts`, `web/e2e/qa.spec.ts`: focused + browser
    evidence.
- **Dependencies/blocks:**
  - P2-04…07 và backend retrieval/ACL/citation contracts đã có; P2-18 cung cấp project
    grouping, P2-19 sở hữu persisted per-user chat history/multi-project `projectIds[]`.
  - Owner hạ live-evidence R02/R03/R05 gate cho môi trường dev/test ngày 2026-07-29;
    issue này không claim Phase 1C/Phase 2 production exit.
  - Backend `VersionMode::AsOf`/`resolve_as_of_version_ids` là authority và không đổi
    trong closure này.
- **Acceptance criteria:**
  - Search và ask hiển thị grounded answer, multi-document citation footnotes,
    document title/page/version deep-link và accessible `aria-live` status.
  - Stream xử lý sequence/dedupe/reconnect terminal; fallback extractive, no-answer,
    citation revoke, cancel và turn sau không phá turn trước.
  - Đổi org xoá retained turns/composer busy state; collection/project/allow-list
    không thể widen scope. Explicit unauthorized hoặc empty resolved scope 403 và không
    phát citation/version metadata.
  - `current`/`compare`/`history` giữ behavior; conflict fixture v1 10 triệu vs v2
    15 triệu phát warning phù hợp từng mode.
  - `as_of` không gửi `documentId`, bắt buộc timestamp hợp lệ; thời điểm giữa v1/v2 trả
    v1 10 triệu + non-current warning, không trả v2 15 triệu.
- **Required tests/evidence:**
  - Vitest: `QaPage.test.tsx`, `askStream.test.ts`,
    `mocks/handlers/qa.asOf.test.ts`, citation-footnote tests và scope-switch tests.
  - Playwright: `web/e2e/qa.spec.ts` cho search→preview, streaming citation,
    revoke/fallback, multi-turn, compare/history và timezone-portable `as_of` v1 proof;
    `chat-history.spec.ts` cho P2-19 integration.
  - Contract/artifact gates: `pnpm --dir web api:check`,
    `python3 scripts/build-roadmap.py --check`,
    tracker export/dry-run và `git diff --check master...HEAD`.
  - Historical delivery: chat-first/deep-link work landed via #324 and follow-ups;
    #374 added the budget conflict-warning evidence; current `as_of` closure is
    [PR #393](https://github.com/anhnth24/project-example/pull/393). Final review covered
    `6e8def4..fa53c06` and returned `Ready to merge: Yes` with no Critical, Important,
    or Minor finding.
- **Security/migration:**
  - Tenant/ACL-sensitive: active token org + profile `allowedCollectionIds` is the
    maximum mock scope; project/request filters only narrow. Unauthorized/mixed/empty
    scope hard-denies with canonical API error and no citation/version metadata.
  - SafeMarkdown/server-minted route IDs remain required; không log prompt, document
    content, PII, token, key hoặc signed URL. Chat history không đưa ngược vào ask
    request làm memory giả.
  - Không có migration, dependency, backend/OpenAPI/storage/auth contract change trong
    closure này; rollback là scoped revert web mock/UI/tests/catalog.
- **Out of scope:**
  - Thay đổi backend `VersionMode::AsOf`, OpenAPI/generated contract hoặc thêm
    document-scoped `as_of` picker.
  - Conversation intelligence/memory, P2-19 persistence redesign, compare/history
    selector redesign, production deployment/SLO/benchmark.

## P2-11 — Member/role admin

- **Status:** Done — #317. UI member table/invite (one-time token)/suspend/role/remove, owner-tier fail-closed mirror server, last-owner 409 + owner-tier 403 mapped. Mở khoá nhờ lát membership API (1C-02/1C-11) landed cùng #317.

- **Plan file:** [P2-11 detailed implementation plan](../../../../reports/plan-2026-07-28-p2-11-member-role-admin.md)
- **Plan/files:** Member table/invite/suspend/role selector; owner restrictions from API.
- **Depends:** P2-02/03/05 + backend 1C-02…04. **Acceptance/tests:** Owner/admin matrix,
  last-owner conflict, invite/suspend/role/403/409/stale-update tests.
- **Security:** UI không hard-code matrix hay thay enforcement. **Out:** custom/group/SSO.

## P2-12 — Usage/quota/reservations

- **Status:** Done — #317. Usage cards từ `GET /usage` (endpoint tổng hợp landed cùng lát membership); route gate `member.manage`. Actionable 429 dùng chung path với document actions.

- **Plan file:** [P2-12 detailed implementation plan](../../../../reports/plan-2026-07-28-p2-12-usage-quota-reservations.md)
- **Plan/files:** Usage cards, limits, active reservations/jobs, actionable 429.
- **Depends:** P2-03/05 + backend 1C-09…11. **Acceptance/tests:** API numbers match;
  unit/timezone/403/429/stale tests.
- **Security:** No client-derived authority/cross-org usage. **Out:** billing.

## P2-13 — Browser/SafeMarkdown hardening

- **Status:** Done — SafeMarkdown + sanitize allowlist + content bound ở #311; CSP/frame/nosniff/referrer landed cùng P2-16 (#313). HSTS để cho reverse proxy (không set ở app).

- **Plan file:** [P2-13 detailed implementation plan](../../../../reports/plan-2026-07-27-p2-13-browser-safemarkdown-hardening.md)
- **Plan/files:** CSP-compatible app, protocol allowlist, raw HTML/SVG/data URL denial,
  content bounds, header checks.
- **Depends:** P2-01/07/10. **Acceptance/tests:** Malicious corpus không execute; CSP
  browser/OWASP/dependency tests; no inline eval.
- **Security:** CSP/frame/nosniff/referrer/HSTS proxy. **Out:** WAF/pentest.

## P2-14 — Accessibility/interaction quality

- **Status:** Done — #313. axe không critical/serious (login/library/modal), focus-sau-route-change, progressbar job. Keyboard cho "ask" chưa làm được vì P2-10 chưa tồn tại.

- **Plan file:** [P2-14 detailed implementation plan](../../../../reports/plan-2026-07-27-p2-14-accessibility-interaction-quality.md)
- **Plan/files:** Skip/landmark/focus/keyboard/progress labels/contrast/reduced motion.
- **Depends:** P2-05/07…12. **Acceptance/tests:** No axe critical; keyboard primary
  flows; focus/reduced-motion/screen reader tests.
- **Security:** Error không đọc internal/token. **Out:** formal certification/i18n.

## P2-15 — Contract/integration/E2E suite

- **Status:** Blocked — umbrella chờ P2-20, P2-21, P2-22 và P2-23 `Done`.
- **Objective:** Đóng qualification suite Phase 2 bằng parity giữa 38 outcome mock hiện
  có và real backend/browser matrix, kèm blocking OWASP/artifact gate; không dùng mock
  E2E thay integration hoặc thay Phase 1C denial suite.
- **Implementation plan:** Phân rã theo
  [thiết kế owner đã duyệt](../../../../../docs/superpowers/specs/2026-08-06-p2-15-full-real-e2e-design.md):
  P2-20 xây fixture/orchestrator + auth/library/upload/actions; P2-21 real knowledge
  flows; P2-22 real multi-org/admin/denial; P2-23 ghép matrix thành required gate và
  promote ZAP fail-closed. P2-15 chỉ tổng hợp evidence và đóng lifecycle sau bốn child.
- **Files/modules:** Umbrella authority:
  `plans/markhand-web/backlog/phase-2/issues/README.md`, thiết kế P2-15, child issue/plan
  evidence, generated roadmap/tracker. Implementation owner/module nằm trong P2-20…23;
  P2-15 không có production-code PR riêng.
- **Dependencies/blocks:** P2-20…23 phải `Done`; P2-21 còn phụ thuộc P2-18/P2-19;
  P2-22 phụ thuộc P2-18/1C-12; P2-23 phụ thuộc P2-21/P2-22/1C-13 và required-check
  configuration. P2-16 tiếp tục chờ P2-15.
- **Acceptance criteria:** Mọi mock outcome có real scenario hoặc exclusion kỹ thuật
  được independent review; closure-SHA chạy real matrix với 0 required skip; desktop,
  SCA/image, blocking ZAP, artifact checksum/secret-canary và teardown đều pass; không
  còn Critical/Important security finding chưa disposition bằng reviewable evidence;
  bốn child và P2-15 evidence nhất quán.
- **Required tests/evidence:** Exact-SHA CI links từ từng child; final required
  aggregator run; sanitized manifest nêu SHA/ref/tool versions/scenario inventory/
  duration/outcome/skipped/fixture checksum/teardown/artifact checksums; branch/ruleset
  evidence cho required check. Historical evidence: real 3/3 ở
  [PR #377 run 30835682218](https://github.com/anhnth24/project-example/actions/runs/30835682218)
  và [master-ancestry job 91853591422](https://github.com/anhnth24/project-example/actions/runs/30864553227/job/91853591422);
  ZAP live gần nhất
  [job 91823571982](https://github.com/anhnth24/project-example/actions/runs/30854831271/job/91823571982)
  có 0 fail/66 pass + informational alert `10049`.
- **Security/migration:** Mandatory independent review cho auth/session, tenant/RBAC/ACL,
  upload/storage, quota, secret/egress, dependency và CI permissions. Runtime credentials
  chỉ tồn tại trong isolated dev/CI; artifact không chứa content/prompt/PII/token/key/
  signed URL/cookie/password. Không migration trực tiếp ở umbrella; child nào đổi schema
  phải có migration/rollback riêng.
- **Out of scope:** Thay backend denial/RLS/ACL gate bằng browser tests; production
  test-only route/bypass; thay public API/converter/native pin chỉ để làm test; P2-16
  packaging/SLO; Phase 3 intelligence.

## P2-16 — Production build/static serving/final gate

- **Status:** In progress — build + static serving + packaged-server test đã landed (#313: history fallback không nuốt /api/*, cache immutable vs no-cache, security headers, tất cả mutation-checked). **Final gate CHƯA đạt**: E2E deploy thật + SLO + scan + gate Phase 1C-12/13 còn lại; Dockerfile.server chưa copy web/dist (quyết định ADR riêng, xem deploy/README.md).

- **Plan/files:** Hashed Vite assets; server/image dist; UI-only history fallback;
  API 404; HTML revalidate, immutable assets, headers.
- **Depends:** P2-13/15 + 1C-12/13.
- **Acceptance/tests:** Deep-link, cache/header/API404, packaged E2E, SLO, scans,
  desktop test/build.
- **Security:** Mock/source map policy; rollbackable immutable assets. **Out:** CDN/HA.

## P2-17 — Document graph

- **Status:** Done — independent final review trên range
  `f1f3434..05b51436eb9402025c955a98563e241557e48163` kết luận spec và
  code/evidence `APPROVED`, không có Critical/Important finding; acceptance và evidence
  bên dưới đã đầy đủ.
- **Plan file:** [P2-17 detailed implementation plan](../../../../reports/plan-2026-08-05-p2-17-document-graph-closure.md)
- **Objective:** Cung cấp graph tài liệu bounded và review độc lập được: node chỉ từ tài
  liệu caller được phép thấy; edge `conflict`, `co_citation`, và opt-in `similarity`;
  community deterministic; UI graph mở đúng document preview. Qdrant không khả dụng phải
  degrade về graph PostgreSQL mà không lỗi hoặc nới ACL.
- **Implementation plan:** 1. Route gate `qa.query` và truyền collection allow-list vào
  service. 2. Service lấy visible documents cùng conflict/co-citation từ PostgreSQL, chỉ
  gọi Qdrant recommend-by-point khi có cả vector index và approved embedder. 3. Mọi
  recommend mang `VectorScope` org/collection fail-closed; lỗi Qdrant fail-soft bằng cách
  bỏ riêng similarity edges, giữ graph PostgreSQL. 4. Prune node/edge theo cap và tạo
  connected components deterministic; web render force layout, community/filter/table
  fallback/keyboard và deep-link `/library/:collectionId?doc=`. 5. Regression integration
  dùng PostgreSQL thật, active generation thật và test-local TCP listener: listener trả
  scroll response hợp lệ, bắt buộc quan sát recommend-by-point request rồi đóng kết nối
  để tạo transport error deterministic và chứng minh degraded behavior bounded.
- **Files/modules:** Server owner: `crates/server/src/routes/graph.rs`,
  `crates/server/src/services/graph.rs`, `crates/server/src/db/graph.rs`,
  `crates/server/src/storage/qdrant.rs`, `crates/server/tests/graph.rs`, OpenAPI graph
  path/schemas và `ROUTE_INVENTORY`. Web owner: `web/src/pages/GraphPage.tsx`,
  `web/src/components/graph/**`, `web/src/lib/forceLayout.ts`,
  `web/src/mocks/handlers/graph.ts`, graph unit/E2E specs.
- **Dependencies/blocks:** P2-07 `Done` cung cấp route preview `?doc=`; backend 1B
  claims/conflicts và P1B-R05 ask-stream cung cấp edge data. Delivery đã merge qua
  [PR #327](https://github.com/anhnth24/project-example/pull/327) SHA
  `abb392099cfdd2df8427d26fee5ffb6ebc07ebd4`,
  [PR #331](https://github.com/anhnth24/project-example/pull/331) SHA
  `0ae8105972f510a9a8d247fbd5fa3996ddcf60cc`, và
  [PR #374](https://github.com/anhnth24/project-example/pull/374) SHA
  `2a8d7c053b0ca2288b0280511b0488cc2996db8a`. Independent final review đã hoàn tất
  trên [PR #392](https://github.com/anhnth24/project-example/pull/392), reviewed head
  `05b51436eb9402025c955a98563e241557e48163`; không còn blocker trong scope P2-17.
  Local hiện không có Docker/PostgreSQL/Qdrant hay `MARKHAND_TEST_*_DATABASE_URL`, nên
  regression PG chỉ compile local; execution thật đã được chứng minh trên CI PostgreSQL
  fixture và không tính missing-env skip là pass.
- **Acceptance criteria:** `GET /api/v1/graph` trả tối đa 500 visible nodes, tối đa 2.000
  edges và communities deterministic; thiếu `qa.query` bị 403; collection ngoài ACL bị
  404; foreign-org/private node hoặc edge không xuất hiện. Conflict/co-citation lấy từ
  PostgreSQL. Similarity dùng Qdrant recommend-by-point với mandatory scope, top-k 5,
  threshold `0.5`, similarity-node cap 200 và similarity-edge cap 500. Khi Qdrant lỗi,
  service không trả error, vẫn trả đúng ACL-scoped conflict/co-citation nodes/edges và
  không tạo similarity/foreign edge. Web hỗ trợ community/filter/table fallback/keyboard
  và click node mở `/library/:collectionId?doc=<documentId>`; generated API không drift.
- **Required tests/evidence:** Local closure: `cargo test -p fileconv-server
  services::graph --lib` = 18 pass; focused Vitest graph suite = 17 pass;
  `pnpm --dir web api:check` pass; `cargo test -p fileconv-server --test graph --no-run`
  compile pass. Regression `graph_qdrant_failure_preserves_acl_scoped_conflict_graph` ở
  commit `50ef793e78f6ab21be5b87e14707d6f9d6c48376` bắt buộc listener quan sát
  `/points/scroll` rồi `/points/query` với `query.recommend.positive`; bỏ/skip similarity
  path làm listener timeout và test fail. PostgreSQL fixture seed visible conflict +
  co-citation, same-org private edge candidate và foreign-org co-citation candidate; kết
  quả chỉ giữ hai visible nodes cùng hai relation edges. Exact test chạy `... ok` trong
  [rust-integration job 92366635226](https://github.com/anhnth24/project-example/actions/runs/31023634736/job/92366635226);
  [rust job 92366635571](https://github.com/anhnth24/project-example/actions/runs/31023634736/job/92366635571)
  và parent run
  [31023634736](https://github.com/anhnth24/project-example/actions/runs/31023634736)
  đều success. Final closure thuộc
  [PR #392](https://github.com/anhnth24/project-example/pull/392); independent review
  `f1f3434..05b51436eb9402025c955a98563e241557e48163` kết luận
  `Spec compliance: APPROVED`, `Code/evidence quality: APPROVED`, không
  Critical/Important.
  Graph MVP evidence: SHA `abb392099cfdd2df8427d26fee5ffb6ebc07ebd4`, run
  [30435638525](https://github.com/anhnth24/project-example/actions/runs/30435638525),
  jobs [rust-integration 90522758925](https://github.com/anhnth24/project-example/actions/runs/30435638525/job/90522758925),
  [web 90522758960](https://github.com/anhnth24/project-example/actions/runs/30435638525/job/90522758960),
  [web-e2e 90522759000](https://github.com/anhnth24/project-example/actions/runs/30435638525/job/90522759000)
  đều success. Similarity evidence: SHA
  `0ae8105972f510a9a8d247fbd5fa3996ddcf60cc`, run
  [30446846876](https://github.com/anhnth24/project-example/actions/runs/30446846876),
  jobs [rust 90559397129](https://github.com/anhnth24/project-example/actions/runs/30446846876/job/90559397129)
  và [rust-integration 90559397215](https://github.com/anhnth24/project-example/actions/runs/30446846876/job/90559397215)
  success; integration log chạy `graph_similarity_edges_from_qdrant_recommend ... ok`.
  Deep-link evidence chỉ dùng [web-e2e job 91689028325](https://github.com/anhnth24/project-example/actions/runs/30814514369/job/91689028325)
  success trên SHA `2a8d7c053b0ca2288b0280511b0488cc2996db8a`; parent run
  [30814514369](https://github.com/anhnth24/project-example/actions/runs/30814514369)
  overall failure do unrelated
  [dev-stack job 91689028310](https://github.com/anhnth24/project-example/actions/runs/30814514369/job/91689028310),
  nên overall run không dùng làm graph evidence.
- **Security/migration:** Independent final security/code review đã hoàn tất, không có
  Critical/Important finding. PostgreSQL RLS + collection allow-list là authority; mỗi
  Qdrant read mang org/collection `VectorScope`, re-check payload scope, không trả vector
  về app; mọi failure phải không broaden scope và không log content/secret. Không đổi
  schema, migration, dependency hoặc public API trong closure này; rollback là revert
  test/docs.
- **Out of scope:** Qdrant batch recommend; tuning threshold `0.5` trên corpus thật;
  clustering ngoài deterministic connected components; production deployment/Phase 2
  exit gate; batch/tuning và issue khác.

## P2-18 — Project grouping (org → project → collection → document)

- **Status:** In progress — owner yêu cầu mới 2026-07-29. `org → dự án → bộ sưu tập →
  tài liệu`: bảng mới `projects` (org-scoped, RLS pattern như `collections`) + cột
  `collections.project_id uuid NULL` (bộ sưu tập chưa gán vẫn hoạt động; 1 bộ sưu tập ∈
  tối đa 1 dự án). `GET/POST /projects`, `PATCH /projects/{projectId}` (đổi tên),
  `POST /collections/{collectionId}/assign-project` (`{projectId: uuid | null}`, action
  route riêng thay vì gộp vào PATCH collection — xem lý do trong
  `routes::collections`'s module doc). Cùng permission `doc.upload` với create/update
  collection (không thêm permission mới). `SearchRequest`/`AskRequest`/ask-stream nhận
  `projectId` optional: server resolve project → tập collectionIds (org-scoped) → giao
  với ACL caller hiện có → đưa vào `resolve_scope` sẵn có (không có đường retrieval mới)
  — projectId lạ/khác org → 404 đồng nhất (no-oracle). `GET /collections` response thêm
  `projectId`/`projectName` (nullable, joined tại read time). Web: dropdown "Phạm vi"
  trong composer Hỏi đáp (`ChatPanel.tsx`, mặc định "Tất cả dự án", reset khi đổi org),
  nav Thư viện nhóm theo dự án (`CollectionNav.tsx`), panel quản lý đơn giản
  (`ProjectsPanel.tsx`, tạo dự án + gán/bỏ gán, đặt trong trang Thư viện — không thêm
  route/rail admin mới cho một tính năng "đơn giản" theo yêu cầu owner). Project
  deletion ngoài phạm vi vòng này (tránh bàn semantics bộ sưu tập mồ côi).

  **Cập nhật (409 cho trùng tên, 2026-07-29):** `POST /projects` và `POST /collections`
  đều đã có unique constraint `(org_id, name)` trong DB từ trước (`uq_projects__org_name`
  migrations/0032, `uq_collections__org_name` migrations/0004) nhưng route trước đó map
  MỌI lỗi DB không phải `NotFound` thành 500 `internal_error` — trùng tên rơi vào đó
  thay vì 409. Đóng theo đúng tiền lệ `services::orgs::CreateOrgError::SlugTaken`
  (`routes::orgs`): thêm `RouteError::NameTaken` map từ `SqlState::UNIQUE_VIOLATION`
  → `409 name_taken` ở cả hai route. `ROUTE_INVENTORY` + `openapi.yaml` (`createProject`/
  `createCollection` responses) thêm `409`.

  **Cập nhật (chuyển sang "Khu Quản trị" `/admin/projects`, 2026-07-29 — owner critique:
  "quản lý project/document/người dùng đang thiết kế UIUX chưa hợp lý"):** `ProjectsPanel.tsx`
  (đặt trong trang Thư viện, xem quyết định cũ ở trên) bị xoá; toàn bộ chức năng chuyển
  sang trang admin mới `AdminProjectsPage.tsx` tại route `/admin/projects`, gate bởi
  `ProtectedRoute permission="doc.upload"` (cùng permission server yêu cầu, không thêm
  permission mới) — cùng nhóm rail "Quản trị" với Thành viên/Sử dụng đã có (rail thêm
  divider/label "QUẢN TRỊ", item "Dự án" chỉ hiện khi `hasPermission('doc.upload')`;
  Thành viên/Sử dụng không đổi, vẫn không rail-gate như trước). Nâng cấp thành bảng: mỗi
  dự án 1 hàng — tên (sửa inline qua `PATCH /projects/{id}`), chip bộ sưu tập thuộc nó
  (mỗi chip có nút "×" bỏ gán), số bộ sưu tập; khu "Chưa thuộc dự án" liệt kê collection
  chưa gán + dropdown gán nhanh. `LibraryPage` chỉ còn duyệt (nav collection nhóm theo dự
  án như cũ qua `CollectionNav.tsx` — không đổi) + link "Quản lý dự án" (gate cùng
  permission) trỏ `/admin/projects`. **Gap đã xác nhận, không tự chế:** `PATCH
  /projects/{id}` không khai báo 409 trong `openapi.yaml` (chỉ `POST /projects` có) —
  sửa file đó ngoài phạm vi lượt việc chuyển trang này, nên inline-rename không hiện
  thông báo "trùng tên" riêng, chỉ generic error path. Mock: `POST /projects` đã enforce
  trùng tên (409 `name_taken`) từ cập nhật trước; `PATCH /projects/{id}` mock cố tình
  KHÔNG enforce (khớp đúng gap contract ở trên, không lệch hành vi so với spec thật).
  Test: `AdminProjectsPage.test.tsx` (bảng, tạo, 409 inline khi tạo trùng tên, sửa tên
  inline, gán/bỏ gán) + `LibraryPage.test.tsx` cập nhật (ProjectsPanel không còn) +
  `e2e/projects.spec.ts` viết lại theo flow mới (vào `/admin/projects` tạo + gán + đổi
  tên + bỏ gán, quay lại Thư viện thấy nav đổi nhóm, 409 trùng tên, guard rail ẩn "Dự án"
  khi thiếu quyền) + `App.test.tsx` thêm case "in-shell notice cho `/admin/projects`
  không có `doc.upload`" (cùng mẫu case `/admin/members` đã có).

- **Plan/files:** `crates/server/migrations/0032_expand_projects.sql`;
  `db/projects.rs`, `routes/projects.rs`; `routes/collections.rs` (assign-project +
  `projectId`/`projectName` hydration), `routes/search.rs`/`routes/ask.rs` (projectId
  filter); OpenAPI path/schemas (`Project`/`ProjectPage`/`CreateProjectRequest`/
  `UpdateProjectRequest`/`AssignProjectRequest`, `Collection`/`SearchRequest`/
  `AskRequest` mở rộng) + `ROUTE_INVENTORY`/`BODY_TAKING_OPERATIONS`;
  `web/src/components/library/CollectionNav.tsx`, `web/src/pages/AdminProjectsPage.tsx`
  (replaces the old `components/library/ProjectsPanel.tsx`, removed), `App.tsx`/
  `lib/router.ts`/`types/routes.ts` (`/admin/projects` route), `components/shell/Rail.tsx`
  (+ `styles.css`, "QUẢN TRỊ" group), `components/qa/{ChatPanel,SearchPanel}.tsx`,
  `pages/QaPage.tsx`, `mocks/{fixtures,handlers/{library,projects,qa}}.ts`.
- **Depends:** P2-07 (Thư viện/`CollectionNav`), P2-10 (Q&A composer/`ChatPanel`),
  backend 1B collections + retrieval (`services::retrieval::resolve_scope`).
- **Acceptance/tests:** `tests/projects.rs` DB-gated (CRUD happy/validate/403/404, org
  isolation, assign/unassign, search filter theo project trả đúng tập tài liệu, 404
  projectId lạ, **409 trùng tên cho cả `POST /projects` và `POST /collections`** —
  chạy trên PG local; #374 bổ sung response 409 `name_taken` còn thiếu cho
  `PATCH /projects/{projectId}` vào `openapi.yaml` + regenerate `contract.ts` — handler
  đã enforce sẵn qua `uq_projects__org_name`, spec chỉ chưa khai); `db::models`/`schema_migrations.rs` drift guard
  cập nhật cho bảng `projects` + cột `collections.project_id`; web unit
  (`QaPage.test.tsx` phạm vi dropdown + reset khi đổi org, `LibraryPage.test.tsx` nhóm
  nav theo dự án + xác nhận ProjectsPanel không còn, `AdminProjectsPage.test.tsx`
  bảng/tạo/409 inline/sửa tên/gán/bỏ gán, `App.test.tsx` in-shell notice cho
  `/admin/projects` thiếu quyền) + `e2e/projects.spec.ts` viết lại (vào `/admin/projects`
  tạo + gán + đổi tên + bỏ gán → quay lại Thư viện thấy nav đổi nhóm; 409 trùng tên;
  guard rail ẩn "Dự án" khi thiếu quyền) — luồng cũ "gán rồi Hỏi đáp chọn phạm vi →
  search đúng tập → 'Tất cả dự án' ra đủ" giữ nguyên hành vi, chỉ đổi nơi tạo/gán.
- **Security:** Cùng permission `doc.upload` với collection create/update (không thêm
  permission mới); RLS org-scoped như `collections`. **Out:** xóa dự án, gán một bộ sưu
  tập vào nhiều dự án, project-scoped permission riêng.

## P2-19 — Chat history (private per-user) + multi-project `projectIds[]`

- **Status:** In progress — owner yêu cầu mới 2026-07-29. Backend-only vòng này (server
  API + OpenAPI + tests); web UI đọc/hiện lịch sử chat và multi-select phạm vi dự án
  trong composer là việc của vòng sau, chưa làm ở đây.
  **(1) `projectIds: string[]`** trên `SearchRequest`/`AskRequest` (ask-stream dùng
  chung `AskRequest`): hợp (union) collection ids của mọi project trong mảng, giao với
  ACL/`collectionIds` như `projectId` đơn (P2-18) đã làm — không có đường retrieval
  mới. `projectId` đơn deprecated nhưng giữ hoạt động nguyên trạng (web hiện dùng nó);
  gửi cả hai field cùng lúc thì hợp cả hai. Bất kỳ id nào không tồn tại/khác org → 404
  (đồng nhất semantics với đơn). Bounded tối đa 20 ids, quá → 400 (kiểm tra thuần, không
  round-trip DB). Mảng rỗng/absent = như không truyền.
  **(2) Chat history riêng tư per-user**: bảng mới `qa_chat_sessions` (id, org_id,
  user_id, title bounded ≤200, created_at, updated_at) + `qa_chat_turns` (id, session_id
  FK cascade, org_id, seq, question bounded ≤8192, answer, answer_mode, citations jsonb,
  warnings jsonb, created_at; UNIQUE(session_id, seq)) — RLS org-scoped như các bảng
  khác, cộng `user_id = caller` trên mọi query (không có endpoint xem chat người khác,
  kể cả cùng org — 404 đồng nhất, cùng khuôn `ask_stream_sessions`/P1B-R05 đã lập).
  `GET/POST /chat-sessions`, `GET/PATCH/DELETE /chat-sessions/{id}`,
  `POST /chat-sessions/{id}/turns` (client ghi sau khi stream/JSON `/ask` đã hiển thị
  xong; seq server cấp = max+1 trong transaction, khoá `FOR UPDATE` trên session để hai
  lần append đồng thời không đụng seq). Cùng permission `qa.query` với `/search`/`/ask`
  (cùng surface Q&A, không thêm permission mới). Citations/warnings lưu jsonb nguyên vẹn
  từ client, KHÔNG re-validate khi đọc lại (client tự re-validate qua
  `POST /citations/resolve` khi thật sự click deep-link) — trade-off ghi rõ trong doc
  comment route. Chỉ audit metadata-only `chat_session.create`/`chat_session.delete`
  (never nội dung câu hỏi/trả lời/tiêu đề); rename và append-turn không audit.
  **(3) Page number cho CitationPin — điều tra, không cần code mới**: đã tồn tại đầy đủ
  từ trước (không phải P2-19): `pdf-inspector` chèn marker `<!-- Page N -->` mỗi trang
  (`crates/core/src/conv/pdf.rs`), `chunking.rs::prepare_chunks` gọi
  `fileconv_core::intelligence::page_before` để suy trang mỗi chunk, lưu cột
  `chunks.page`, và `CitationPin.page: Option<u32>` (đã serialize `page` — không phải
  tên `pageNumber` — trong OpenAPI/response) đã điền từ đó. Không thêm field trùng lặp.

- **Plan/files:** `crates/server/migrations/0034_expand_qa_chat_history.sql`;
  `db/chat_sessions.rs`, `routes/chat_sessions.rs`; `db/projects.rs` (`merge_project_ids`/
  `resolve_project_scope` đa id), `routes/search.rs`/`routes/ask.rs` (`projectIds`);
  `services/audit.rs` (`ChatSessionCreate`/`ChatSessionDelete` + `AuditResource::ChatSession`
  allowlist); `db/models.rs`/`tests/schema_migrations.rs` drift guard; OpenAPI path/schemas
  (`ChatSession`/`ChatSessionPage`/`ChatSessionDetail`/`ChatTurn`/
  `CreateChatSessionRequest`/`UpdateChatSessionRequest`/`AppendChatTurnRequest`,
  `SearchRequest`/`AskRequest` mở rộng `projectIds`) + `ROUTE_INVENTORY`/
  `BODY_TAKING_OPERATIONS`; `web/src/mocks/spec/{openApiSpec,yaml}.test.ts` pin-count
  49→56 + `contract.ts` regen.
- **Depends:** P2-18 (`db::projects`/`resolve_project_scope`, `routes::search`/
  `routes::ask` project filter), backend 1B ask/search + citation pins.
- **Acceptance/tests:** `tests/projects.rs` DB-gated thêm (union 2 project đúng tài
  liệu, 1 id lạ trong mảng → 404, cả `projectId`+`projectIds` cùng lúc → hợp, mảng rỗng
  = như không truyền, >20 ids → 400) + unit test thuần cho `merge_project_ids`;
  `tests/chat_history.rs` DB-gated (CRUD + append seq đúng thứ tự + cursor pagination +
  user B không thấy/mở/xóa được session user A cùng org (404) + org isolation +
  citations jsonb round-trip + title/question bounds 400).
- **Security:** `qa.query` (không thêm permission mới); RLS org-scoped + lọc `user_id`
  ứng dụng cho `qa_chat_*`, cùng khuôn `ask_stream_sessions`. **Out:** web UI lịch sử
  chat + multi-select dropdown phạm vi dự án (vòng sau), share/team chat history, xóa
  dự án (đã out ở P2-18), `pageNumber` field riêng (đã có `page`, xem mục 3).

- **Cập nhật (web UI landed, 2026-07-29 — cùng vòng với P2-10's chat-first redesign, xem
  mục cập nhật ở đó cho phần layout/footnote/picker):** phần "web UI đọc/hiện lịch sử chat
  và multi-select phạm vi dự án" mà mục "Out" gốc ở trên hoãn sang vòng sau — nay đã làm.
  `web/src/components/qa/useChatHistory.ts` (hook mới, không đụng `state/askStream.ts`)
  gọi đủ 6 operation (`listChatSessions`/`createChatSession`/`getChatSession`/
  `updateChatSession`/`deleteChatSession`/`appendChatTurn`). Luồng ghi lịch sử: một lượt
  `ChatTurnBubble` settle `completed`/`revoked` (không phải `error`/`cancelled` — đã có
  test riêng cho từng trường hợp) báo `snapshot` (answer/mode/citations/warnings) lên qua
  `onStatusChange`; nếu chưa có session hiện hành, `recordTurn` tạo session trước (title =
  câu hỏi đầu, cắt ở 80 ký tự) rồi mới append turn — fire-and-forget, lỗi chỉ hiện notice
  nhỏ (`appendError`), không phá luồng chat. Bug thật đã bắt và sửa trong vòng này:
  `activeSessionId` đổi từ `undefined` sang id thật ngay khi `recordTurn` vừa tạo xong
  session cho cuộc trò chuyện ĐANG MỞ trước đó bị `ChatPanel` hiểu nhầm là "người dùng mở
  phiên khác", xoá mất chính live turn vừa hoàn tất (thấy được qua React's "Cannot update a
  component while rendering a different component" + phần tử biến mất khỏi DOM ngay sau
  khi vừa tìm thấy trong test) — sửa bằng `sessionSwitchToken` riêng (chỉ tăng khi
  `startNewConversation()`/`selectSession()` được gọi tường minh, không tăng khi
  `recordTurn` tự gán id), tách hẳn hai trường hợp "id vừa được gán cho hội thoại đang mở"
  và "người dùng thật sự đổi hội thoại". `mocks/handlers/chatSessions.ts` (mock mới, org+
  user scoped qua `getUserChatSessions`/`findUserChatSession`, không enforce `qa.query` —
  cùng tiền lệ `search`/`ask`/`graph` đã set vì `DEMO_USER` không có permission đó) +
  `mocks/fixtures.ts` seed 2 phiên lịch sử thật (1 phiên 1 turn/1 citation, 1 phiên 2 turns
  với turn thứ hai trích 2 tài liệu khác nhau + 1 citation có `page` — dữ liệu demo cho
  đúng "Tổng hợp từ N tài liệu"/"Trang X" mà không cần dựng kịch bản live-stream phức tạp).
  `ProjectPicker.tsx` gửi `projectIds[]` cho `SearchRequest`/`AskRequest` (mock's
  `resolveProjectScope` union `projectId`+`projectIds` trước khi lọc `collectionIds`, mirror
  `merge_project_ids` server-side). Test: unit (`QaPage.test.tsx`, 14 kịch bản: sidebar
  list/mở lại/đổi tên/xóa, picker multi-select + reset khi đổi org, footnote page/multi-
  doc note, ghi lịch sử completed/revoked có lưu — error/cancelled không lưu) +
  `citationFootnotes.test.ts` (thuần, 10 case) + E2E `chat-history.spec.ts` mới (2 spec:
  2 câu → "Cuộc trò chuyện mới" → 1 câu nữa → mở lại phiên đầu thấy nguyên transcript +
  citation click được; đổi tên + xóa phiên).

## P2-20 — Real E2E foundation + auth/library/upload/actions

- **Status:** Backlog — canonical draft chờ owner dùng trigger `Tôi duyệt draft.` để
  chuyển `Ready` và sync tracker.
- **Objective:** Tạo harness real-browser cô lập, deterministic và fail-closed; chạy
  auth/library/upload/document-action outcomes qua built SPA + real server/PostgreSQL/
  Qdrant/storage/workers, không production test seam hoặc mock fetch.
- **Implementation plan:** Thêm run ID namespace; sinh runtime credential/resource
  fixture bằng dev/CI-only setup; bounded teardown; refactor real Playwright support;
  port scenario-equivalent login/logout, giữ intended deep-link qua anonymous login,
  one-time 401 refresh/session recovery, collection/preview/download/delete/reindex/
  retry, upload progress→indexed/preview, 413 và action-error flows. Supervisor phải
  fail khi fixture/worker/cleanup/redactor/artifact validation lỗi.
- **Files/modules:** Owner theo CODEOWNERS `@anhnth24`; `deploy/scripts/web-e2e-real.sh`,
  fixture/orchestration scripts + tests dưới `deploy/scripts/`, `web/e2e-real/support.ts`,
  real auth/library/upload/action specs, Playwright real config, CI `dev-stack` wiring.
- **Dependencies/blocks:** P2-02…14 `Done`; real 3-flow baseline đã pass trên master
  ancestry. Không phụ thuộc P2-18/P2-19 hoặc Phase 1C closure. Draft chưa được owner
  approve nên chưa `Ready`.
- **Acceptance criteria:** Mỗi run có namespace/credential tạm; không dùng fixed seeded
  account làm authority; setup/teardown idempotent và bounded; required process death,
  cleanup leak, redactor failure hoặc secret-canary match làm job fail. Real Chromium
  giữ anonymous intended route sau login, recover đúng một refresh, hiển thị upload
  progress rồi terminal indexed/preview, và pass toàn bộ library/actions đã liệt kê;
  HTTP mutation/error thực sự chạm backend; 0 required skip; mock suite không regression.
- **Required tests/evidence:** TDD cho orchestration negative paths;
  `python3 deploy/scripts/test_web_e2e_real_orchestration.py`;
  `DEV_STACK_MODE=full bash deploy/scripts/dev-stack-ci.sh`; `make check-web`;
  `make check-desktop`; exact-SHA `dev-stack` job và sanitized manifest chứa scenario
  inventory, tool/SHA/ref, fixture checksum, duration/outcome, teardown và artifact
  checksums.
- **Security/migration:** Mandatory review cho auth/session, upload/converter/storage,
  runtime secrets/egress và CI permissions. Credential chỉ ở masked runtime state; không
  content/prompt/PII/token/key/signed URL/cookie/password trong logs/artifacts. Dev/CI
  fixture tool phải refuse production profile. Mỗi run tạo rồi xoá bounded user/session/
  org/collection/document/upload/job/object records trong isolated test tenant; không đổi
  production API hoặc tenant semantics. N/A migration hiện tại; nếu implementation cần
  schema thì phải tách migration/rollback review, không tự mở rộng scope.
- **Out of scope:** Q&A/version/graph/chat (P2-21); multi-org/admin/quota (P2-22);
  blocking gate/ZAP (P2-23); production test endpoint/bypass; parallel real workers
  trước khi isolation được đo/review.

## P2-21 — Real knowledge flows

- **Status:** Blocked — chờ P2-18, P2-19 và P2-20 `Done`.
- **Objective:** Chứng minh search, streaming Q&A, citation/version modes, graph và
  private chat history hoạt động qua real server/storage/provider boundaries với dữ
  liệu Việt deterministic và không leak scope/version metadata.
- **Implementation plan:** Seed run-scoped documents qua real convert/index workers;
  dùng approved loopback mock embedding/provider profile; port search→preview,
  ask-stream→citation→preview, no-answer, provider fallback, citation revoke, current/
  `as_of`/compare/history, multi-project narrowing, graph filter/table/keyboard/deep-link
  và chat-history outcomes. Search preview phải render sanitized content. Fault phải được
  tạo bằng process/API/ACL thật, không production route flag.
- **Files/modules:** Owner web/server test boundaries theo CODEOWNERS `@anhnth24`;
  `web/e2e-real/{support,qa,graph,chat-history,...}`, real fixture scripts,
  `deploy/scripts/web-e2e-real.sh`, provider supervision, relevant server integration
  tests và sanitized manifest renderer.
- **Dependencies/blocks:** P2-17 `Done`; P2-18/P2-19/P2-20 chưa `Done`. Public
  search/ask/SSE/citation contracts là authority; không đổi contract chỉ để test dễ hơn.
- **Acceptance criteria:** Stream event order/terminal durable; citation mở đúng
  document/version; revoke không complete sau khi mất quyền; fallback chỉ pass khi fault
  được quan sát; `as_of`/compare/history đúng effective versions; project scope không
  widen; search preview sanitize; graph filter/table fallback/keyboard/deep-link đúng;
  chat history per-user giữ reload/pagination và không cross-user. Worker/provider death
  được phân loại riêng; 0 required skip.
- **Required tests/evidence:** Focused server SSE/revoke integration; real Playwright
  project qua `DEV_STACK_MODE=full bash deploy/scripts/dev-stack-ci.sh`; mock Q&A/graph/
  chat regression; `make check-web`; API drift check; exact-SHA manifest và CI job chứng
  minh provider fault/revoke thực sự xảy ra, successful teardown và không skip.
- **Security/migration:** Mandatory review cho tenant/ACL, citation identity, LLM/provider
  content policy, secret/egress và persisted chat. Không log prompt/document/citation
  quote/vector/token. N/A migration theo scope test-only; production schema/API change
  cần issue/migration/rollback riêng.
- **Out of scope:** Auth/library fixture foundation (P2-20); admin/quota/denial matrix
  (P2-22); required gate/ZAP (P2-23); model-quality benchmark hoặc Phase 3 intelligence.

## P2-22 — Real multi-org security/admin

- **Status:** Blocked — chờ P2-18, P2-20 và 1C-12 `Done`.
- **Objective:** Chứng minh qua browser + real API rằng org/project scope, membership
  admin, permission và quota failures không render hoặc mutate dữ liệu tenant ngoài
  quyền, kể cả late HTTP/SSE response.
- **Implementation plan:** Seed hai org cô lập, owner/admin/member actors, nhiều
  project/collection, same-org restricted collection và quota states; port org switch/
  org listing/denied switch, stale HTTP/SSE discard, project narrowing, invite/role/
  suspend/reactivate/remove, last-owner 409, owner-tier/collection/document-action 403,
  API-backed usage values, quota/upload 429 và stale-update outcomes. Delay layer chỉ
  forward response backend thật, không synthesize allow.
- **Files/modules:** Owner security/web/server test boundaries theo CODEOWNERS
  `@anhnth24`; real multi-org/admin/usage specs, runtime fixture setup/cleanup,
  test-runner reverse proxy hoặc Playwright scheduling, denial manifest renderer,
  `deploy/scripts/web-e2e-real.sh`, relevant server denial/integration suites.
- **Dependencies/blocks:** P2-18/P2-20 và 1C-12 chưa `Done`; 1C-12 vẫn là authority cho
  backend denial. P2-22 không thể dùng browser test để thay/waive RLS/ACL evidence.
- **Acceptance criteria:** Sau switch không còn org-A ID/title/version/content hoặc
  active stream render; foreign/unknown scope no-oracle; forbidden mutation không đổi
  state; org list/current state đúng; suspend/reactivate và member mutations đúng role;
  last-owner trả 409; usage UI khớp API; collection/document actions 403; quota không
  over-admit và UI map 429 actionable; incomplete fixture hard-fail thay vì owner
  fallback. Denial manifest chỉ metadata/hash và 0 required skip.
- **Required tests/evidence:** Exploit/denial fixture RED trước; Phase 1C-12 exact-SHA
  evidence; real Playwright via full dev stack; focused server auth/ACL/quota integration;
  `make check-web`; `make check-rust-tests`; sanitized manifest nêu route template,
  expected/actual status, actor role, org relationship, request-ID hash, teardown và CI
  links.
- **Security/migration:** Mandatory independent security review cho auth/session,
  tenant/RBAC/ACL, quota, audit, fixture credentials, SQL/RLS và CI. Không raw ID/content/
  prompt/PII/token/key/signed URL trong artifact. N/A migration theo test scope; schema
  fixture helper không được đổi production semantics hoặc bypass RLS.
- **Out of scope:** Thay Phase 1C denial/load gate; Q&A/graph/chat behavior (P2-21);
  gate promotion/ZAP (P2-23); custom roles/SSO; production seed account.

## P2-23 — Blocking real-E2E/OWASP qualification gate

- **Status:** Blocked — chờ P2-21, P2-22 và 1C-13 `Done`, cùng owner/admin capability
  cấu hình required check.
- **Objective:** Biến full real Playwright matrix, desktop regression, SCA/image scan,
  blocking ZAP, artifact validation và teardown thành một required qualification check
  ổn định cho mọi thay đổi liên quan.
- **Implementation plan:** Mở rộng classifier cho web/server/OpenAPI/migration/deploy/
  locks/workflow/security paths; chạy child jobs trên relevant change; stable aggregator
  luôn report success/failure và chỉ no-op khi classifier chứng minh irrelevant. Promote
  ZAP thành automatic `fail_action: true`, bỏ `continue-on-error`; áp scoped exception
  lifecycle cho alert informational `10049`; validate/sanitize/checksum artifact trước
  upload; chạy full real matrix serial; cấu hình aggregator thành required check. P2-20
  sở hữu base artifact staging/redaction; P2-23 sở hữu release manifest, retention và
  enforcement của toàn matrix.
- **Files/modules:** Owner CI/security/deploy theo CODEOWNERS `@anhnth24`;
  `.github/workflows/ci.yml`, `scripts/classify-ci-changes.py` + self-tests,
  `docs/conventions/ci.md`, ZAP rules/exception record, artifact manifest/redactor/
  validator scripts/tests, Playwright reporters, branch/ruleset evidence.
- **Dependencies/blocks:** P2-21/P2-22/1C-13 chưa `Done`; required-check configuration
  cần repository owner/admin. Security exception cho `10049` cần owner, exact URL scope,
  rationale, compensating cache controls, expiry và retest condition.
- **Acceptance criteria:** Relevant path không thể skip real matrix/ZAP/desktop/scans;
  irrelevant path cho explicit aggregator no-op; mọi unknown ZAP WARN/FAIL block; rules
  change cần live before/after evidence; required scenario skipped, missing artifact,
  checksum mismatch, teardown failure hoặc secret/content canary đều block. Branch/ruleset
  thực tế yêu cầu stable aggregator. Real matrix chạy serial cho tới khi có issue riêng
  chứng minh isolation cho parallel. Release manifest bắt buộc có SHA/ref/tool versions,
  scenario inventory/duration/outcome/skipped count, fixture checksum, teardown và mọi
  artifact checksum; trace/screenshot chỉ giữ cho non-content page allowlist.
- **Required tests/evidence:** `python3 scripts/classify-ci-changes.py --self-test`;
  workflow contract/negative tests; redactor/manifest/checksum/canary tests; closure-SHA
  CI chạy full real matrix + desktop + dependency/image + ZAP với 0 required skip;
  sanitized Playwright/ZAP/scan/desktop artifacts; GitHub required-check/ruleset evidence.
- **Security/migration:** Mandatory security/CI-permission/dependency/egress review.
  Workflow actions pin full SHA; permissions least privilege; artifact retention explicit;
  exception lifecycle reviewable. N/A database migration; rollback restore warning-only
  workflow nhưng không xóa historical reports, và P2-15/P2-16 phải quay lại Blocked.
- **Out of scope:** Authenticated active DAST/pentest; Phase 1C denial replacement;
  production deploy/SLO/HA/CDN; đổi app behavior chỉ để silence scanner.

## Exit gate

Phase 2 chỉ đóng khi P2-20…23 và P2-15 `Done`, P2-16 đạt trên backend deploy thật, và
Phase 1C denial/security gate đã pass; mock E2E không thay thế integration.
