# Phase 2 (Web SPA) — kế hoạch thực thi

Date: 2026-07-26
Base commit: `7b0a85e` (branch `claude/pull-master-large-pr-eg39zh`, PR #310)

`plans/markhand-web/phase-2-web-spa.md` đã định nghĩa **cái gì** (P2.1–P2.9 + Gate).
Tài liệu này là **thứ tự làm**, dựa trên trạng thái code đo được hôm nay, và nêu hai
thứ chặn thật mà bản plan gốc không thể biết trước.

## 1. Điều kiện vào — đã đạt

| Hạng mục | Trạng thái đo được |
|---|---|
| API surface | **32 endpoint** trong `crates/server/openapi/openapi.yaml`: auth, collections, documents/versions/publish/preview/diff, uploads, jobs + `/jobs/{id}/events` (SSE), search, ask, `/ask/stream`, citations/resolve, downloads/{capability}, conflicts + triage, health live/ready/start |
| Contract → TypeScript | `web/src/api/generated/contract.ts` sinh bằng `openapi-typescript` 7.13.0; `pnpm api:check` **chặn drift trong CI** (chính nó bắt được #309 đổi spec mà không regenerate) |
| CI web | job `web` xanh: `format:check`, `lint`, `test`, `api:check`, `build` |
| Khung web | `web/src` mới có 9 file (App, health, main, styles, 1 test, contract sinh tự động) |
| Tái dùng từ desktop | `SafeMarkdown.tsx` 25 dòng, `ui.tsx` 600 dòng, `LibraryView.tsx` 160, `IntelligenceView.tsx` 1207, `lib/ipc.ts` 210 (sẽ bị thay bởi `api/client.ts`) |

## 2. Hai thứ chặn thật — phải xử trước hoặc song song

### B1. OpenAPI thiếu schema cho phần lớn payload thành công

Parity check hiện có (`api::openapi::ROUTE_INVENTORY` +
`openapi_inventory_is_structurally_complete_two_way`) chỉ kiểm **path, method,
status code** — **không** kiểm schema. Vì thế tồn tại lỗ sau, đo trực tiếp từ spec:

- **7 operation** nhận body nhưng **không khai `requestBody`**:
  `POST /auth/login`, `POST /auth/refresh`, `POST /auth/logout`,
  `POST /collections`, `PATCH /collections/{collectionId}`,
  `POST .../download-capability`, `POST /citations/resolve`
- **22 response 2xx** (không tính 204) **không có content schema**, gồm cả
  `POST /auth/login 200`, `POST /auth/refresh 200`, `GET /auth/me 200`,
  `GET /collections/{id} 200`, `GET /documents/{id} 200`, `GET /jobs/{jobId} 200`,
  `GET /openapi.yaml 200`

> Đã sửa số so với bản đầu của tài liệu này. Ban đầu tôi ghi 9 requestBody và 21
> response, vì tôi đếm theo **method** (POST/PATCH/PUT ⇒ phải có body). Đọc handler
> thì `POST .../publish` (`routes/documents.rs:428-432`) và
> `POST /documents/{documentId}/reindex` (`:520-526`) **không có body extractor**
> nào — thêm `requestBody` cho chúng là mô tả sai API, nên chúng không phải gap.
> Ngược lại `GET /openapi.yaml 200` là gap tôi đã bỏ sót.
>
> **Chặn này đã xử lý xong**: 22/22 response có content schema, 7/7 requestBody đã
> khai, và parity check nay kiểm cả mức schema (`openapi_schema_completeness_gaps`)
> nên lỗ không tái diễn.

Hệ quả trực tiếp: **client typed mà SPA phải dựa vào không có type cho luồng auth
và phần lớn CRUD**. P2-02/P2-03/P2-05 giả định điều ngược lại. Nếu bắt đầu mà không
lấp, đội web sẽ tự viết type tay — đúng thứ mà `pnpm api:check` được dựng để ngăn.

Chi phí lấp: khai `requestBody`/response schema trong `openapi.yaml` (hình dạng đã
tồn tại trong handler Rust), rồi `pnpm --dir web api:generate`. Đây là việc server,
thuộc R06 *"Complete OpenAPI/fixtures"*, và giải thích luôn cụm *"full status/schema
matrix"* trong gap của R04 — schema chưa từng được kiểm.

Đề nghị bổ sung một check: mở rộng parity thành schema-level (mọi operation nhận body
phải có `requestBody`; mọi 2xx ≠ 204 phải có content), để lỗ này không tái diễn.
Ghi chú: `refresh_token` đi trong **JSON body** (`routes/auth.rs:70`, `199-211`),
không phải cookie — plan gốc để mở hai khả năng, đây là câu trả lời.

### B2. P2-11 và P2-12 chưa có API để gọi

Trong 32 endpoint **không có** bất kỳ route nào cho member/invite/role/usage/quota.
R04 ghi rõ *"Out: admin membership API"*. Nghĩa là:

- **P2-11 (member/role admin): không build được** — thiếu hoàn toàn endpoint. Việc
  này thuộc Phase 1C (Multi-org Security), chưa được lên lịch.
- **P2-12 (usage/quota/reservations): một phần** — quota metadata có qua header
  (R06), nhưng không có endpoint tổng hợp usage.

Cần quyết định: đưa P2-11/P2-12 ra khỏi MVP Phase 2, hay chèn phần server tương ứng
vào trước. Không nên để hai issue này nằm im trong wave rồi phát hiện lúc code.

## 3. Ánh xạ plan gốc ↔ backlog

| Plan gốc | Issue |
|---|---|
| P2.1 Web workspace và shared UI | P2-01 |
| P2.2 Typed HTTP/SSE client | P2-02, P2-03, P2-04 |
| P2.3 Auth và application shell | P2-05, P2-06 |
| P2.4 Library | P2-07, P2-08, P2-09 |
| P2.5 Q&A | P2-10 |
| P2.6 Admin tối thiểu | P2-11, P2-12 |
| P2.7 Browser security và accessibility | P2-13, P2-14 |
| P2.8 Tests | P2-15 |
| P2.9 Build và serve SPA | P2-16 |

## 4. Waves

### Wave 0 — nền (không phụ thuộc việc server nào ngoài B1)

`P2-01` workspace/UI foundations · `P2-02` mock server từ OpenAPI · `P2-03` typed
client + refresh single-flight · `P2-04` fetch-based SSE transport · `P2-13`
SafeMarkdown/CSP hardening

Chạy song song với B1: phần nào của B1 lấp xong thì phần đó của client được sinh type.
`P2-13` không chờ gì cả — port `SafeMarkdown.tsx` (25 dòng) kèm test raw HTML /
dangerous link / SVG+data URL / oversized theo P2.7.

### Wave 1 — auth và shell

`P2-05` login/session/app shell · `P2-06` org switch + scope-safe state

Endpoint đã có (`/auth/login|refresh|logout|me`). Chặn bởi B1 phần auth: cần schema
cho login/refresh/me trước khi typed client có nghĩa. Access token giữ trong memory;
refresh token đi trong body theo contract hiện tại.

### Wave 2 — library

`P2-07` list/preview sanitized · `P2-08` upload progress + job lifecycle ·
`P2-09` download/delete/reindex/retry

Endpoint ổn định. `P2-08` dùng `/jobs/{jobId}/events` (SSE) nên phụ thuộc `P2-04`.
Preview/download luôn qua API authorize — không tự ghép URL.

### Wave 3 — Q&A (phụ thuộc server đang siết)

`P2-10` streaming search/Q&A/citations

Chặn bởi **R02/R03/R05**: semantics `citation_revoked` khi delete xen giữa 2 batch,
reconnect/`Last-Event-ID`/purge, và việc ask hiện vẫn thường rơi về extractive khi
entailment fail-closed. Đừng khoá UI Q&A trước khi ba điểm đó đóng.

### Wave 4 — admin (chờ quyết định B2)

`P2-11` (thiếu API) · `P2-12` (một phần qua header)

### Wave 5 — đóng phase

`P2-14` accessibility/interaction · `P2-15` contract/integration/E2E (Playwright:
login/refresh/logout, upload→indexed, preview/delete/reindex, ask→citation, org
switch không hiển thị scope cũ, permission deny, quota exceed) · `P2-16` production
build + static serving + final gate

## 5. Wave 0 — định nghĩa xong

- `web/src/{api,auth,components,hooks,pages,state,types,lib}` theo P2.1; **không**
  import Tauri vào web; copy có kiểm soát từ desktop, chưa tách `packages/ui`.
- `api/client.ts`: request/response typed theo contract sinh tự động, bearer
  injection, refresh single-flight, error chuẩn hoá `{code,message,requestId,details?}`,
  đọc quota header, cancellation.
- SSE: fetch-based có bearer header (**không** `EventSource` native), refresh giữa
  stream, reconnect + `Last-Event-ID`, xử lý sequence và revocation.
- Mock server sinh từ OpenAPI để wave 0–1 không cần stack thật; stack thật dành cho
  E2E ở `P2-15`.
- Gate của wave: `pnpm --filter markhand-web format:check lint test api:check build`
  xanh (đã là job `web` trong CI), cộng test SafeMarkdown của `P2-13`.

## 6. Quyết định cần từ owner

1. **B1**: ai lấp schema OpenAPI, và có thêm check schema-level parity không?
2. **B2**: P2-11/P2-12 rời khỏi Phase 2 MVP, hay chèn server membership API trước?
3. Wave 0–1 phát triển trên **mock server** (đề xuất) hay chờ POC stack thật?
4. Tái dùng desktop bằng **copy có kiểm soát** (đề xuất, theo plan gốc) hay tách
   `packages/ui` ngay?

## 7. Không thuộc kế hoạch này

Desktop editor/compare đầy đủ, watch folder, intelligence ngoài search/Q&A, OIDC/SSO
— giữ nguyên phần "Không thuộc phase" của plan gốc.
