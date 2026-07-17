# Phase 2 — Web SPA MVP

## Outcome

React/Vite SPA cho login, thư viện, upload/preview/reindex/delete, Q&A streaming có
citation và admin member/role/usage tối thiểu.

## P2.1 — Web workspace và shared UI

Tạo:

```text
web/src/
├── api/
├── auth/
├── components/
├── hooks/
├── pages/
├── state/
├── types/
└── lib/
```

Tái dùng có chọn lọc từ desktop:

- `SafeMarkdown.tsx`;
- primitives trong `components/ui.tsx`;
- icons, design tokens, focus/reduced-motion styles;
- pure `intelligenceUtils`.

Không import Tauri vào web. Chưa tách `packages/ui` nếu mới có một consumer web;
copy có kiểm soát trước, chỉ shared package khi API giữa hai app ổn định.

## P2.2 — Typed HTTP/SSE client

`web/src/api/client.ts` thay `app/src/lib/ipc.ts`:

- typed request/response theo OpenAPI;
- bearer/access token injection;
- refresh single-flight;
- normalized `{code,message,requestId,details?}` errors;
- quota headers;
- request cancellation;
- fetch-based SSE có bearer header (không dùng native `EventSource`), token refresh,
  reconnect/`Last-Event-ID`, sequence và revocation handling cho jobs/Q&A.

Contract generation/check phải phát hiện Rust/TypeScript drift trong CI.

## P2.3 — Auth và application shell

Routes:

- `/login`;
- `/library/:collectionId?`;
- `/qa/:collectionId?`;
- `/admin/members`;
- `/admin/usage`;
- `/help` stub.

Yêu cầu:

- access token trong memory;
- rotating refresh token trong `HttpOnly`, `Secure`, `SameSite` cookie nếu deployment
  dùng cookie refresh;
- CSRF protection cho mutation dựa trên cookie;
- route và control guards theo permission;
- org switch hủy request/SSE cũ và clear scope cache;
- session/org/role hiển thị rõ.

## P2.4 — Library

Adapt `app/src/components/LibraryView.tsx`:

- collection navigation;
- document list/filter/pagination;
- multipart upload với progress;
- trạng thái `uploaded|converting|converted|indexing|indexed|failed`;
- job SSE;
- Markdown preview đã sanitize;
- delete confirm;
- reindex/retry failed job.

Không dùng filesystem tree, native dialog, local path hoặc client-side convert queue.
Preview/download luôn qua API authorize.

## P2.5 — Q&A

Tách ask/search panel từ `IntelligenceView.tsx`:

- collection/document scope;
- index status;
- hybrid search results;
- streaming answer;
- citation deep-link về document/heading/page;
- offline/extractive fallback và warning provider;
- abort khi đổi org/scope;
- answer region `aria-live`.

Client không tự tin cậy citation URL; dùng route ID do server cấp.

## P2.6 — Admin tối thiểu

- Member list/invite/suspend.
- Role select owner/admin/editor/viewer.
- Usage/quota summary và trạng thái reservation/job.
- Error 403/429 rõ ràng.
- Chỉ render theo permission nhưng server vẫn là nguồn authorization.

## P2.7 — Browser security và accessibility

Headers:

- strict CSP;
- frame denial;
- `nosniff`;
- referrer policy;
- HSTS ở reverse proxy production.

Markdown tests phủ raw HTML, dangerous link, SVG/data URL và oversized content.

Accessibility:

- skip link và semantic landmarks;
- focus sau route change/modal;
- keyboard upload/search/ask;
- progressbar cho upload/job;
- axe không có critical violations;
- contrast và reduced-motion.

## P2.8 — Tests

- Unit: API error/refresh/SSE parser, scope cache, pure utilities.
- Component: login, library state, Q&A stream/citations, member role.
- Contract: OpenAPI fixtures.
- Playwright:
  - login/refresh/logout;
  - upload → indexed;
  - preview/delete/reindex;
  - ask → citation;
  - org switch không hiển thị response cũ;
  - permission deny và quota exceed.
- OWASP baseline/dependency/license scan.

## P2.9 — Build và serve SPA

- Build Vite tạo hashed immutable assets.
- `crates/server` serve `web/dist` hoặc image/runtime layer chứa dist theo ADR deploy.
- History fallback chỉ cho route UI, không nuốt `/api/*`.
- HTML dùng no-cache/revalidate; hashed assets dùng immutable long cache.
- Security headers áp dụng cả HTML và static assets.
- Packaged-server E2E xác minh deep-link refresh, API 404 và asset cache policy.

## Gate

- Golden browser flow end-to-end qua backend deploy thật.
- Expired/revoked token và org switch xử lý đúng.
- Không render dữ liệu từ scope cũ.
- Q&A đạt first-token/retrieval SLO và citation mở đúng nguồn.
- Browser/a11y/security gates xanh.
- Desktop `app/` vẫn test/build bình thường.

## Không thuộc phase

- Desktop editor/compare/source fidelity đầy đủ.
- Watch folder.
- Intelligence ngoài search/Q&A.
- OIDC/SSO.
