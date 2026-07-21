# REST, OpenAPI, SSE và error conventions

OpenAPI tại `crates/server/openapi/openapi.yaml` là contract authority. Rust route,
generated TypeScript client và fixtures phải cùng thay đổi trong một PR khi public
contract đổi.

## REST

- Base path `/api/v1`; resource plural (`/documents`, `/collections`), action chỉ khi
  không biểu diễn được bằng resource (`/documents/{id}:reindex`).
- ID dùng UUID string. Date/time RFC 3339 UTC. Enum dùng stable lowercase string.
  Optional field rõ `null` hoặc absent theo schema, không dùng sentinel.
- List trả `{items, pageInfo}`; cursor opaque, stable ordering bắt buộc, bounded limit.
- Mutation retryable nhận `Idempotency-Key`; server lưu scope/actor/request hash và
  replay response hoặc trả conflict, không chạy side effect lần hai.
- Request có `requestId`; client được gửi/echo correlation ID nhưng không tự tin cậy
  như authorization input.

## Canonical errors

Mọi lỗi JSON dùng:

```json
{ "code": "validation_failed", "message": "safe user-facing text", "requestId": "uuid", "details": {} }
```

- `code` stable machine-readable; `message` an toàn cho user; `details` chỉ validation
  metadata allowlist.
- Không expose SQL, storage key, stack trace, prompt, document content, token, secret,
  signed URL hoặc policy internals.
- HTTP status phản ánh category: 400 validation, 401 auth, 403 permission, 404
  non-disclosing resource, 409 state/idempotency conflict, 429 limit/quota, 5xx server.

## SSE

SSE event data là JSON envelope versioned:

```json
{ "version": 1, "sequence": 42, "event": "job.progress", "requestId": "uuid", "data": {} }
```

- Sequence tăng đơn điệu trong một stream; reconnect dùng `Last-Event-ID`.
- Client dedupe event đã nhận, detect gap và refetch state thay vì đoán.
- Auth/ACL/revocation được kiểm tra lúc connect và trước mỗi sensitive payload; revoke
  đóng stream. Không đặt credential trong URL/query.
- **Emission guarantee (HTTP-honest):** sau revoke, server dừng generate/enqueue application
  frame mới. Byte đã giao cho Hyper/kernel không thu hồi được; tối đa một frame nhỏ đã
  encode có thể còn trên transport (bounded tail) trước close. Không claim zero network
  bytes post-commit.
- Q&A SSE: `event: metadata` (full JSON envelope: pins / version_context / conflicts) rồi
  token frames, rồi `event: close`.
- Heartbeat, retry hint, retention window và terminal event được route-specific contract
  định nghĩa ở Phase 1B.

## Compatibility

- Additive field/enum value cần client tolerate; breaking rename/remove/type change cần
  version/deprecation plan, migration note và generated-client snapshot.
- Generated TypeScript nằm `web/src/api/generated/`; không sửa tay.
- Sample spec/fixtures được validate bằng Rust round-trip và
  `pnpm --filter markhand-web api:check`.
