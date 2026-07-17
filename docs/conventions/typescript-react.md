# TypeScript and React conventions

Áp dụng cho browser SPA tại `web/`. Desktop `app/` giữ contract hiện tại cho tới khi
Phase 2 thay thế một flow có evidence.

## Boundary và API

- `web/` là browser-only: không import `@tauri-apps/*`, `window.__TAURI__` hay desktop
  filesystem/IPC wrapper. ESLint chặn import này.
- OpenAPI là API authority. `src/api/generated/` chỉ nhận generated output và không sửa
  tay; UI wrapper/hook đặt ngoài generated directory.
- Không log token, refresh credential, document content, prompt, PII hoặc raw error
  response vào browser console/telemetry.

## TypeScript và component

- `strict`, `noUnusedLocals` và `noUnusedParameters` luôn bật.
- Component chỉ sở hữu presentation/local interaction. Server/cache/session state có
  owner rõ, không duplicate cùng state ở nhiều component.
- Mọi fetch/SSE hook phải cancel `AbortController`/stream trong cleanup; 401 refresh
  và reconnect contract được thêm ở Phase 2.
- Không dùng `any`, non-null assertion hoặc unsafe HTML để vượt qua type/error boundary.
- Export component public bằng tên rõ; test import component, không test implementation
  private.

## UI states và accessibility

- Mỗi async screen có loading, empty, error và retry/recovery state rõ.
- Dùng semantic HTML trước; label mọi control, heading tuần tự, focus visible và thông
  báo async qua live region phù hợp.
- Không dùng màu là tín hiệu duy nhất; target click/tap tối thiểu 44px nếu là control.
- Không render untrusted HTML. Markdown/content cần sanitize policy được review trước.

## Commands

```bash
pnpm --filter markhand-web format:check
pnpm --filter markhand-web lint
pnpm --filter markhand-web test
pnpm --filter markhand-web build
```

CI chạy bốn command này. Exception cần issue, scope nhỏ, expiry và test.
