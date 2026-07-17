# Rust conventions

Áp dụng cho `fileconv-core`, `fileconv-knowledge`, `fileconv-server` và worker.
`rustfmt.toml`/`clippy.toml` là baseline máy thực thi; guard CI không cho warning mới
vượt baseline trong code cũ.

## Module và dependency

- Tuân [`dependencies.md`](dependencies.md): core framework/storage-free; knowledge
  pure mặc định; server route → service → repository/adapter.
- `pub` là contract: public type/function phải có rustdoc ngắn nêu invariant, error
  hoặc ownership khi không hiển nhiên.
- Tên module/function `snake_case`, type/trait `PascalCase`, enum variant là danh từ,
  error code là stable machine-facing string.
- Không `mod` lộ implementation-only khi consumer chỉ cần facade.

## Error, panic và logging

- Request/worker/converter path không dùng `unwrap`, `expect`, `panic!` cho input,
  filesystem, network, parse hoặc external process. Propagate typed error kèm context
  không nhạy cảm.
- `unwrap` chỉ trong test, invariant có chứng minh cục bộ hoặc bootstrap immutable;
  comment lý do ngay tại chỗ nếu không hiển nhiên.
- Không đưa document text, prompt, PII, token, API key, signed URL hoặc secret vào
  `Display`, `Debug`, error context hay log.
- `unsafe` bị cấm trừ khi ADR phê duyệt, safety invariant/test ghi cạnh block.

## Async, blocking và cancellation

- CPU-heavy conversion/OCR/parse và blocking external CLI chạy qua boundary blocking
  rõ ràng; không block executor HTTP.
- Mọi network/subprocess/job có timeout, cancellation propagation và cleanup
  idempotent khi applicable.
- Lock không được giữ qua await; pool/transaction scope ngắn và tenant context được
  truyền explicit.

## Quality policy

```bash
bash scripts/check-rust-quality.sh
```

Lệnh chạy format, Clippy warning-as-error cho crate mới (`knowledge`, `server`) và
baseline delta cho code cũ. Baseline không phải miễn trừ vĩnh viễn: khi sửa một vị trí
cũ, ưu tiên xử lý lint tại vị trí đó; thay đổi baseline cần giải thích trong PR.

## Exception

Exception lint/boundary cần: lý do kỹ thuật, phạm vi nhỏ nhất, owner, expiry hoặc
follow-up issue, và regression test. Không dùng `#[allow(...)]` cấp module/crate để
che warning hàng loạt.
