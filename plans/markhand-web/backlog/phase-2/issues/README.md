# Phase 2 issues — Web SPA MVP

Parent plan: [`../../../phase-2-web-spa.md`](../../../phase-2-web-spa.md)

<!-- roadmap-default-status: backlog -->

**Trạng thái tổng quan (cập nhật 2026-07-29).** MVP xây trên mock server đã merge vào
`master`: **13/18 issue Done** (P2-01…09, P2-11, P2-12, P2-13, P2-14, P2-16 phần build
+ serve). **4 In progress**: P2-10 (Q&A — UI/mock/stream xây xong trên contract hiện có,
xem chi tiết bên dưới), P2-15 (E2E — nửa mock-based xong, nay có thêm flow ask→citation
của P2-10; nửa real-deployment hoãn), **P2-17** (Document graph — owner request mới
2026-07-29, MVP server+web+mock xong, `similarity`/Qdrant chờ vòng riêng — xem chi tiết
bên dưới), **P2-18** (Project grouping — owner request mới 2026-07-29, org → project →
collection → document, MVP server+web+mock xong — xem chi tiết bên dưới). P2-11/P2-12
rời khỏi Blocked nhờ lát membership API (1C-02/1C-11 slice) landed ở #317.

> Ranh giới quan trọng: "Done" ở đây nghĩa là **hành vi client đã build và test trên
> mock/deterministic**, đã qua CI (`web`, `web-e2e`, `rust`, `rust-integration`) trên
> `master`. **Exit gate của cả Phase 2 vẫn CHƯA đạt** — nó đòi E2E trên backend deploy
> thật + gate denial/security của Phase 1C (xem mục "Exit gate" cuối trang). Mock E2E
> không thay integration.

Truy vết merge: **#311** (P2-01…06 — foundations, client, SSE, mock, login, org switch),
**#312** (P2-07…09 — library, upload, actions), **#313** (P2-14, P2-16 — a11y, serve SPA),
**#317** (P2-11, P2-12 — member/usage admin, trên lát membership API),
**#318** (P2-15 nửa mock-based — Playwright E2E + job `web-e2e`).
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
P2-02..14 → P2-15
P2-15 + Phase 1C gate → P2-16
```

## P2-01 — React/Vite workspace và UI foundations

- **Status:** Done — #311. Web workspace, no Tauri import, CI job `web` xanh.

- **Plan/files:** Tạo `web/` scripts/layout; copy browser-safe tokens/icons/primitives.
- **Depends:** Không. **Acceptance/tests:** Build/test độc lập; no Tauri import;
  typecheck/lint/unit/dependency-boundary; desktop vẫn xanh.
- **Security:** Dependency/license scan. **Out:** shared package/redesign desktop.

## P2-02 — OpenAPI contracts và mock server

- **Status:** Done — #311. Generated types + `pnpm api:check` drift gate; mock server sinh từ OpenAPI.

- **Plan/files:** Pin generator; generated types; drift check; auth/org/library/job/
  Q&A/admin/error/SSE fixtures và mock scenarios.
- **Depends:** Stable 1B OpenAPI. **Acceptance/tests:** Drift fails CI; generated files
  immutable; fixture/schema/breaking-change tests; mock excluded production.
- **Security/migration:** N/A — không thay đổi persisted schema; fixtures synthetic,
  không chứa token/PII thật.
- **Out:** Chờ toàn bộ 1C mới làm UI.

## P2-03 — Typed HTTP client/session refresh

- **Status:** Done — #311. Refresh single-flight; ADR 0010 rotation; concurrent-401/revoke tests.

- **Plan/files:** Fetch wrapper, access token memory, refresh single-flight, one retry,
  normalized errors/request ID/quota, abort.
- **Depends:** P2-02. **Acceptance/tests:** Concurrent 401 một refresh; revoked refresh
  logout; race/loop/malformed/403/429/network/abort tests.
- **Security:** No token storage/log. **Out:** offline queue/Tauri IPC.

## P2-04 — Fetch-based SSE transport

- **Status:** Done — #311. Fetch-based (không EventSource), Last-Event-ID, reconnect/backoff, abort-on-scope.

- **Plan/files:** Streaming parser với bearer, Last-Event-ID, dedupe/gap, refresh/
  reconnect/backoff/snapshot, abort on scope change.
- **Depends:** P2-02/03. **Acceptance/tests:** Không native EventSource/token URL;
  chunk boundary/reconnect/order/revoke/cancel tests.
- **Security:** Bounded buffer/backoff, no content logs. **Out:** WebSocket.

## P2-05 — Login/session/application shell

- **Status:** Done — #311. Bearer refresh trong memory (không cookie/CSRF); router + guard matrix.

- **Plan/files:** Router, auth bootstrap/login/protected shell/guards/logout/help stub.
- **Depends:** P2-01/03 + P1B-F05 browser refresh contract. **Acceptance/tests:**
  Intended route, expiry, guard matrix, login/refresh/logout component tests và
  integration CSRF/cookie-origin contract theo auth ADR.
- **Security:** Transport theo auth ADR. Nếu chọn cookie: HttpOnly/Secure/SameSite +
  CSRF/Origin contract; nếu chọn bearer refresh: không cookie/CSRF nhưng token không
  được persist/log. Server luôn là authority. **Out:** signup/reset/MFA/OIDC.

## P2-06 — Org switch và scope-safe state

- **Status:** Done — #311. Scope epoch; `useScopeSafeRequest`/`useScopeSafeSse` bỏ response scope cũ.

- **Plan/files:** Org-scoped cache keys; atomic switch; abort REST/SSE; clear stores;
  scope generation ignores late response.
- **Depends:** P2-03…05 + backend 1C org APIs. **Acceptance/tests:** No old-org render;
  delayed/active-stream/rapid-switch/stale-membership tests.
- **Security:** No unapproved persisted tenant cache. **Out:** simultaneous org view.

## P2-07 — Library/list/sanitized preview

- **Status:** Done — #312. Collection nav, filter + cursor pagination thật, preview qua SafeMarkdown. Không có endpoint cross-collection nên "tất cả bộ sưu tập" chỉ điều hướng.

- **Plan/files:** Adapt browser-safe LibraryView; collection navigation, filter/page,
  status, preview states + SafeMarkdown; unresolved conflict badge/count, side-by-side
  cited BA/design/dev claims và resolved-history link.
- **Depends:** P2-02/03/05/06. **Acceptance/tests:** Stable URL/pagination; API-only
  preview; unsafe markdown, 403/404, switch-race tests.
- **Security:** No local path/public key. **Out:** desktop editor/compare.

## P2-08 — Upload progress và job lifecycle

- **Status:** Done — #312. XHR progress thật, job SSE-nudge → `GET /jobs/{id}`. Không quan sát được stage index (API chỉ trả convert job id) — báo "converted, indexing tiếp server-side".

- **Plan/files:** Multipart/progress/cancel; job SSE; reconnect snapshot; accessible
  status for uploaded→indexed/failed.
- **Depends:** P2-04/07. **Acceptance/tests:** Client/server progress distinct; recover
  refresh; success/cancel/loss/gap/413/415/429/filename tests.
- **Security:** No client conversion queue. **Out:** folder/watch/resumable protocol.

## P2-09 — Download/delete/reindex/retry

- **Status:** Done — #312. Capability issue+redeem, delete tombstone có confirm. Không có endpoint retry-convert nên "thử lại" = reindex (ghi rõ trong UI).

- **Plan/files:** Authorized actions, permission/confirm/conflict/idempotency handling.
- **Depends:** P2-07/08 + backend 1C guards. **Acceptance/tests:** Delete closes preview;
  server deny wins; confirm/concurrency/stale/signed-route tests.
- **Security:** No client-built object URLs; CSRF/idempotency. **Out:** purge policy.

## P2-10 — Streaming search/Q&A/citations

- **Status:** In progress — **owner hạ gate 2026-07-29** (môi trường dev/test, không chờ
  full live-evidence R02/R03/R05): UI xây trên OpenAPI/SSE contract hiện có + mock server
  như P2-01..09. `QaPage` không còn là placeholder: `search`/`ask` (đồng bộ) và
  `POST /ask/stream` (mock mới, `mocks/handlers/qa.ts`) đều hoạt động; stream reducer
  (`state/askStream.ts`) xử lý đúng thứ tự sự kiện `ask.started → ask.token* →
  [ask.warning]* → ask.citations → ask.version_context → ask.completed → stream.closed`
  (mirror `services/qa/ask_stream.rs`), có dedupe-guard độc lập với transport, và các
  trạng thái `completed`/`revoked` (`citation_revoked` giữa chừng)/`error` (mọi
  `stream.closed` reason + network/session-lost) đều accessible qua `aria-live="polite"`.
  Mock kịch bản `citation_revoked` và fallback extractive điều khiển được qua marker cố
  định trong câu hỏi (`QA_STREAM_MARKERS`, export từ `mocks/handlers/qa.ts` — seam cho
  test, cùng quy ước `__markhandMock*`). current/as-of/compare/history đều có UI thật
  (mode selector + document/version picker qua `GET /documents/{id}/versions` — không
  phải chỉ current), vì cả bốn field đều có sẵn trong `AskRequest`/`SearchRequest`.
  **Gap đã xác minh trong contract (không tự chế client-side):** `CitationPin`
  (`openapi.yaml`, `components.schemas.CitationPin`) KHÔNG có `logicalDocumentId`/
  `versionId` — chỉ `ResolveCitationRequest` (`/citations/resolve`) đòi hỏi hai field đó,
  và caller phải *đã biết* chúng trước khi gọi, nên không có đường nào từ một citation
  thô của `ask`/`ask/stream` quay lại "tài liệu/phiên bản nào" để deep-link preview —
  `CitationCard.tsx`/`AskPanel.tsx` nói rõ điều này trong UI (không hiện nút "Xem trước"
  chết) thay vì bịa một id. Deep-link + version badge **có** hoạt động đầy đủ cho kết quả
  `search` (hits mang `documentId`/`versionId` — quy ước riêng của mock vì
  `SearchResponse.hits` là `additionalProperties: true` trong spec, không phải trường bắt
  buộc theo hợp đồng). Conflict warning demo: chỉ mô phỏng đúng một luật thật của server
  (`services/qa/grounding.rs`: chế độ `current` trích một phiên bản không phải hiện hành
  → warning) — kịch bản "BA 10 triệu vs thiết kế 15 triệu, cảnh báo rồi v2 resolved" đầy
  đủ cần dữ liệu conflict-claim liên kết version mà thời lượng việc này không cho phép
  dựng cho đúng cả 3 chế độ; ghi nhận là gap còn lại, không phải đã làm.
  "Trạng thái reconnect" chỉ hiển thị chung là "đang stream" — transport P2-04
  (`api/sse.ts`) không phát một `SseMessage` kind riêng cho "đang thử kết nối lại" (chỉ
  âm thầm retry/backoff nội bộ), nên UI không bịa tín hiệu không có thật; chỉ có trạng
  thái cuối (`completed`/`revoked`/`error` với lý do) là quan sát được.

  **Cập nhật (chat UI, cùng ngày):** `AskPanel` (đơn lượt) → `ChatPanel` — giao diện hội
  thoại nhiều lượt hỏi-đáp trong một phiên. Kiến trúc giữ nguyên như chốt: server vẫn
  đơn lượt (`/ask`, `/ask/stream` không đổi, không gửi lịch sử lên server, không chế
  conversation memory phía client thành context giả). Lịch sử chat **chỉ tồn tại trong
  React state của `ChatPanel`** (session in-memory) — mất khi tải lại trang (đánh đổi đã
  chấp nhận, ghi rõ trong UI), **không** persist localStorage vì có thể chứa nội dung tài
  liệu. Mỗi lượt (`ChatTurnBubble`) sở hữu một instance `useAskStream` riêng — tái dùng
  nguyên `useAskStream`/`state/askStream.ts`, không sửa reducer; một lượt sau không bao
  giờ ghi đè state của lượt trước. Composer (ô nhập + chọn chế độ truy vấn) chỉ cho một
  stream tại một thời điểm: disable khi lượt cuối chưa "settled", nút "Hủy" gọi
  `reset()` của đúng lượt đang chạy — vì `reset()` tự nó không phân biệt được "đã hủy"
  với "chưa từng chạy" (cả hai đều về `'idle'`), `ChatTurnBubble` tự đóng băng
  answer/citations/warnings ngay trước khi gọi `reset()` và báo lên trạng thái
  `'cancelled'` riêng (không phải trong `state/askStream.ts`) để composer biết lượt đã
  xong. Label mode (`fallback_extractive`/`llm_unverified`/…) tra theo **key string
  thuần** (`components/qa/answerMode.ts`) chứ không theo enum từ `contract.ts` — một
  agent song song có thể thêm `llm_unverified` vào contract sau; UI đã sẵn sàng hiện
  nhãn cảnh báo "Trả lời từ LLM (chưa kiểm chứng đối chiếu)" cho giá trị đó ngay cả
  trước khi `api:generate` chạy lại. Scope-safety: `ChatPanel` tự phát hiện thấy trước đó
  chưa org-scoped (state chat không hề tồn tại ở bản đơn lượt) nên đã làm đúng theo
  pattern P2-06 hiện có (`LibraryPage.tsx`'s `effectiveView`/`retainedDocuments`): lịch sử
  được giữ kèm epoch nó được tạo ra, và bị xoá sạch (adjust-state-while-rendering) ngay
  khi epoch đổi (đổi org/logout) — đóng luôn gap "chưa có test org-switch riêng cho
  AskPanel" đã ghi nhận trước đó (xem Acceptance/tests bên dưới).

- **Plan/files:** Search/ask panel, current/as-of/compare/history selector, index
  readiness, stream reducer, fallback + version-change notes, citation deep-link with
  version badge/effective date, current conflict warning + resolved conflict note,
  abort scope change; **+ chat UI**: turn history, per-turn stream, org-scoped clear.
  - `web/src/mocks/handlers/qa.ts` — `search`/`ask` (đồng bộ, giữ tương thích) + `askStream`
    mới: toàn bộ response `/ask/stream` là một chuỗi `text/event-stream` dựng sẵn (mọi sự
    kiện đã quyết định xong trước khi trả response — không có gì thật sự bất đồng bộ như
    `jobEvents`), trả qua `rawBody` — deterministic, không `setTimeout`/sleep-race.
    `registerOperation('askStream', ...)` khiến nó được match trước khi rơi vào
    `DELIBERATELY_UNMOCKED_OPERATIONS` fallback của `registry.ts`/`fetchMock.ts` (2 file đó
    ngoài phạm vi sửa của task này — comment ở đó vẫn liệt kê `askStream` là "deliberately
    unmocked", nay không còn đúng cho riêng operation này; để lại làm việc chưa xong, xem
    report cuối). Semantics đơn lượt của handler không đổi cho chat UI — mỗi lượt vẫn một
    request/response độc lập.
  - `web/src/mocks/fixtures.ts` — thêm thuần túy (không sửa fixture cũ): một tài liệu 2
    phiên bản (`QA_COMPARE_DOCUMENT_ID`) cho demo compare/history có dữ liệu thật để so
    sánh.
  - `web/src/state/askStream.ts` (+ test) — reducer thuần, `describeAskStreamError`
    (không đổi cho chat UI).
  - `web/src/components/qa/**` — `SearchPanel`, `CitationCard`, `DocumentPreviewPanel`,
    `useAskStream`/`askStreamSource` (SSE qua P2-04, không `EventSource`); **mới**:
    `ChatPanel` (thay `AskPanel`), `ChatTurnBubble` (một lượt, một `useAskStream`),
    `answerMode.ts` (map mode wire-string → nhãn tiếng Việt, key string thuần).
  - `web/src/pages/QaPage.tsx`, `web/src/pages/QaPage.test.tsx` (+ test chat 2 lượt,
    revoke lượt 2 không phá lượt 1, hủy giữa chừng, clear khi đổi org),
    `web/e2e/qa.spec.ts` (4 spec cũ giữ hành vi tương đương qua layout chat + 1 spec chat
    nhiều lượt mới, tổng suite E2E mock 25).
- **Depends:** P2-04…07 + backend ACL. **Acceptance/tests:** `aria-live`; current source
  citation; multi-document citations; old/new amount example labels v1/v2 and delta;
  BA 10m vs design 15m warning then v2 resolved (**chưa làm** — xem gap ở trên);
  as-of/history/deep-link (**search only**, xem gap ở trên)/sequence/fallback/no-answer/
  revoke tests đã có (`askStream.test.ts` + `QaPage.test.tsx` + `qa.spec.ts`).
  switch-mid-answer: **đã đóng** — `QaPage.test.tsx` nay có một kịch bản org-switch cụ thể
  gắn với `ChatPanel` (hỏi 1 lượt, `manager.setScope` sang org khác, xác nhận lịch sử về
  rỗng và composer hết "busy"), bên cạnh bảo đảm chung ở `hooks/useScopeSafeSse.test.tsx`.
  Chat-specific: 2 lượt liên tiếp giữ history độc lập, `citation_revoked` ở lượt 2 không
  phá lượt 1 (cả unit lẫn e2e), hủy giữa chừng giữ nguyên phần trả lời đã có kèm thông báo
  "Đã hủy" thay vì xoá trắng.
- **Security:** Sanitized Markdown/server route IDs. **Out:** intelligence/conversation
  memory (server vẫn đơn lượt; history chat là UI-only, không gửi lên server, không
  persist).

## P2-11 — Member/role admin

- **Status:** Done — #317. UI member table/invite (one-time token)/suspend/role/remove, owner-tier fail-closed mirror server, last-owner 409 + owner-tier 403 mapped. Mở khoá nhờ lát membership API (1C-02/1C-11) landed cùng #317.

- **Plan/files:** Member table/invite/suspend/role selector; owner restrictions from API.
- **Depends:** P2-02/03/05 + backend 1C-02…04. **Acceptance/tests:** Owner/admin matrix,
  last-owner conflict, invite/suspend/role/403/409/stale-update tests.
- **Security:** UI không hard-code matrix hay thay enforcement. **Out:** custom/group/SSO.

## P2-12 — Usage/quota/reservations

- **Status:** Done — #317. Usage cards từ `GET /usage` (endpoint tổng hợp landed cùng lát membership); route gate `member.manage`. Actionable 429 dùng chung path với document actions.

- **Plan/files:** Usage cards, limits, active reservations/jobs, actionable 429.
- **Depends:** P2-03/05 + backend 1C-09…11. **Acceptance/tests:** API numbers match;
  unit/timezone/403/429/stale tests.
- **Security:** No client-derived authority/cross-org usage. **Out:** billing.

## P2-13 — Browser/SafeMarkdown hardening

- **Status:** Done — SafeMarkdown + sanitize allowlist + content bound ở #311; CSP/frame/nosniff/referrer landed cùng P2-16 (#313). HSTS để cho reverse proxy (không set ở app).

- **Plan/files:** CSP-compatible app, protocol allowlist, raw HTML/SVG/data URL denial,
  content bounds, header checks.
- **Depends:** P2-01/07/10. **Acceptance/tests:** Malicious corpus không execute; CSP
  browser/OWASP/dependency tests; no inline eval.
- **Security:** CSP/frame/nosniff/referrer/HSTS proxy. **Out:** WAF/pentest.

## P2-14 — Accessibility/interaction quality

- **Status:** Done — #313. axe không critical/serious (login/library/modal), focus-sau-route-change, progressbar job. Keyboard cho "ask" chưa làm được vì P2-10 chưa tồn tại.

- **Plan/files:** Skip/landmark/focus/keyboard/progress labels/contrast/reduced motion.
- **Depends:** P2-05/07…12. **Acceptance/tests:** No axe critical; keyboard primary
  flows; focus/reduced-motion/screen reader tests.
- **Security:** Error không đọc internal/token. **Out:** formal certification/i18n.

## P2-15 — Contract/integration/E2E suite

- **Status:** In progress — #318 + follow-up. **Nửa mock-based xong**: harness Playwright (mock-mode build, Chromium) + 17 spec chạy trong CI (job `web-e2e`) — auth/library/actions/member-admin/usage/permission-deny/quota, và **upload→indexed đã hết hoãn** (`web/e2e/upload.spec.ts`: chặn XHR bằng `page.route()` rồi replay qua fetch-mock trong page — happy path + 413). **Harness real-deployment đã landed**: `deploy/scripts/web-e2e-real.sh` + Playwright project `real` (`web/e2e-real/`, smoke login + library trên credential seed), chạy trong CI job `dev-stack` tier full (classifier đã có carve-out full-tier cho harness); lần chạy live đầu tiên là chính CI của PR chứa nó. **ask→citation đã hết hoãn** (P2-10): `web/e2e/qa.spec.ts` — search→preview, ask→stream→citations, kịch bản `citation_revoked` giữa chừng, kịch bản fallback extractive (mock 24 spec, xem chi tiết ở mục P2-10). **Còn hoãn**: upload→indexed real-mode, OWASP baseline. Org-switch đã hết hoãn: 1C-01 ship list/switch, UI switcher + E2E `org-switch.spec.ts` chứng minh "no stale org-A render" (mock 24 spec). Unit/component đã có (462 test, tăng từ P2-10's reducer/QaPage suite).

- **Plan/files:** Unit API/SSE/cache; component auth/library/Q&A/admin; Playwright full
  flows, org switch, deny/quota; CI artifacts redacted.
- **Depends:** P2-02…14; deployed integration cần 1C endpoints.
- **Acceptance/tests:** Mock deterministic + real deployment E2E; no stale-scope render;
  desktop regression.
- **Security:** Ephemeral users/credentials. **Out:** thay backend denial suite.

## P2-16 — Production build/static serving/final gate

- **Status:** In progress — build + static serving + packaged-server test đã landed (#313: history fallback không nuốt /api/*, cache immutable vs no-cache, security headers, tất cả mutation-checked). **Final gate CHƯA đạt**: E2E deploy thật + SLO + scan + gate Phase 1C-12/13 còn lại; Dockerfile.server chưa copy web/dist (quyết định ADR riêng, xem deploy/README.md).

- **Plan/files:** Hashed Vite assets; server/image dist; UI-only history fallback;
  API 404; HTML revalidate, immutable assets, headers.
- **Depends:** P2-13/15 + 1C-12/13.
- **Acceptance/tests:** Deep-link, cache/header/API404, packaged E2E, SLO, scans,
  desktop test/build.
- **Security:** Mock/source map policy; rollbackable immutable assets. **Out:** CDN/HA.

## P2-17 — Document graph

- **Status:** In progress — owner yêu cầu mới 2026-07-29 (force-directed graph + sidebar
  "Communities" checkbox, có ảnh mẫu). MVP: `GET /api/v1/graph` (org/ACL-scoped như
  `/collections`/`/documents`, gate `qa.query` cùng tiền lệ `/conflicts`) trả `conflict`
  edges thật từ `claims`/`conflicts` (PG, đã test DB-gated: org isolation, ACL riêng tư,
  bounded cap 500 node/2000 edge) và `co_citation` edges từ
  `ask_stream_sessions.cited_document_ids` (bảng thật duy nhất lưu "tài liệu nào được
  trích dẫn theo answer nào" — không có bảng lịch sử QA riêng). Communities = connected
  components thuần Rust (không thêm crate graph). Web: trang "Đồ thị" (rail icon
  `Network`), force-directed layout tự viết (~150 dòng, `lib/forceLayout.ts`, seeded
  deterministic — không thêm `d3-force`), sidebar cộng đồng + filter bộ sưu tập + chế độ
  xem bảng (a11y fallback) + danh sách node điều hướng bàn phím, mock fixture 13
  node/3 cụm/3 loại cạnh cho org A + graph nhỏ riêng cho org B, unit/component/e2e test.
  **`similarity` (Qdrant) là chỗ gắn sẵn (stub), CHƯA có truy vấn vector thật** —
  sandbox không có Qdrant để kiểm chứng, và API `storage/qdrant.rs` hiện tại
  (`scroll_points`) hard-code `with_vector: false` nên cần một vòng riêng (thêm biến
  thể lấy vector thật + kiểm thử với Qdrant thật) trước khi bật edge này; graph vẫn
  trả đủ `conflict`/`co_citation` khi không có Qdrant, không lỗi.

- **Plan/files:** `crates/server/src/routes/graph.rs`, `db/graph.rs`, `services/graph.rs`
  (thuật toán thuần); OpenAPI path/schemas (`GraphNode`/`GraphEdge`/`GraphCommunity`/
  `GraphResponse`) + `ROUTE_INVENTORY`; `web/src/pages/GraphPage.tsx`,
  `components/graph/**`, `lib/forceLayout.ts`, `mocks/handlers/graph.ts`.
- **Depends:** P2-07 (điều hướng vào `/library/:collectionId` khi click node — không có
  route sâu tới preview một tài liệu cụ thể, vì `LibraryPage` chọn tài liệu bằng state
  cục bộ chứ không phải URL param) + backend 1B claims/conflicts + P1B-R05 ask-stream.
- **Acceptance/tests:** `services::graph` unit test (components/pruning, xác định);
  `tests/graph.rs` DB-gated (permission, conflict edge, co_citation edge, org isolation,
  ACL riêng tư, bounded cap — chạy thật trên PG local, 6/6 pass); web unit
  (`forceLayout.test.ts`, `GraphPage.test.tsx` 7 kịch bản) + `e2e/graph.spec.ts` (3 kịch
  bản: cụm + sidebar, tắt cụm ẩn node, click node → preview thật qua library).
- **Security:** Cùng ACL/permission với `/conflicts`; không thêm quyền mới chưa seed
  role. **Out:** `similarity` edge thật (chờ Qdrant thật), deep-link preview một tài
  liệu cụ thể từ đồ thị.

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

- **Plan/files:** `crates/server/migrations/0032_expand_projects.sql`;
  `db/projects.rs`, `routes/projects.rs`; `routes/collections.rs` (assign-project +
  `projectId`/`projectName` hydration), `routes/search.rs`/`routes/ask.rs` (projectId
  filter); OpenAPI path/schemas (`Project`/`ProjectPage`/`CreateProjectRequest`/
  `UpdateProjectRequest`/`AssignProjectRequest`, `Collection`/`SearchRequest`/
  `AskRequest` mở rộng) + `ROUTE_INVENTORY`/`BODY_TAKING_OPERATIONS`;
  `web/src/components/library/{CollectionNav,ProjectsPanel}.tsx`,
  `components/qa/{ChatPanel,SearchPanel}.tsx`, `pages/QaPage.tsx`,
  `mocks/{fixtures,handlers/{library,projects,qa}}.ts`.
- **Depends:** P2-07 (Thư viện/`CollectionNav`), P2-10 (Q&A composer/`ChatPanel`),
  backend 1B collections + retrieval (`services::retrieval::resolve_scope`).
- **Acceptance/tests:** `tests/projects.rs` DB-gated (CRUD happy/validate/403/404, org
  isolation, assign/unassign, search filter theo project trả đúng tập tài liệu, 404
  projectId lạ — chạy trên PG local); `db::models`/`schema_migrations.rs` drift guard
  cập nhật cho bảng `projects` + cột `collections.project_id`; web unit
  (`QaPage.test.tsx` phạm vi dropdown + reset khi đổi org, `LibraryPage.test.tsx` nhóm
  nav theo dự án, `ProjectsPanel.test.tsx` tạo/gán/bỏ gán + permission gate) +
  `e2e/projects.spec.ts` (tạo dự án → gán bộ sưu tập → Hỏi đáp chọn phạm vi → search
  đúng tập → "Tất cả dự án" ra đủ).
- **Security:** Cùng permission `doc.upload` với collection create/update (không thêm
  permission mới); RLS org-scoped như `collections`. **Out:** xóa dự án, gán một bộ sưu
  tập vào nhiều dự án, project-scoped permission riêng.

## Exit gate

Phase 2 chỉ đóng khi P2-16 đạt trên backend deploy thật và Phase 1C denial/security
gate đã pass; mock E2E không thay thế integration.
