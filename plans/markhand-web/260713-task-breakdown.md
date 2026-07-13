# Markhand Web — Task breakdown giai đoạn 1 (Track A/B/C + C0)

> Nguồn: [`260713-phase-plan.md`](260713-phase-plan.md) +
> [`../../docs/web-architecture.md`](../../docs/web-architecture.md).
> Nhân sự đã chốt: 3-4 member (đa số chưa vững Rust) + lead + AI agent.
> Format mỗi task theo intern guide cũ: **Hướng làm** (định hướng, không code mẫu),
> **Cần tìm hiểu**, **File đụng**, **Tiêu chí xong**, **Cạm bẫy**.
> Phạm vi file này: các track song song đầu tiên. Task M1 trở đi chẻ sau khi A/B/C hội tụ
> (chẻ sớm quá sẽ sai vì phụ thuộc kết quả benchmark + hình dáng crates/knowledge).

## Sơ đồ sở hữu file (tránh đụng nhau khi làm song song)

- **Lead + AI agent:** `app/src-tauri/src/{knowledge,vector_index,intelligence}.rs`,
  `crates/knowledge/` (mới) — KHÔNG member nào đụng vùng này.
- **M1 (Backend-Infra/QA):** `bench/web-spike/` (mới), `bench/REPORT_WEB_*.md`.
- **M2 (Backend Rust học dần):** `crates/server/` (mới), `crates/server/migrations/`.
- **M3 (Frontend):** `web/` (mới), `docs/web-code-standards.md` phần web.
- `docs/web-code-standards.md`: M2 viết phần server, M3 viết phần web, lead review — 2 người
  sửa 2 mục khác nhau, thống nhất khung mục lục trước khi viết.

## Lệnh nền

```bash
cargo build --release && cargo test          # kiểm tra không phá desktop/CLI hiện có
docker compose -f bench/web-spike/docker-compose.yml up -d   # sau khi A1 xong
```

---

## C0 — KICKOFF CHECKLIST (tuần đầu, làm trước hoặc song song mọi task khác)

### C0.1. Quy ước code server+web — `docs/web-code-standards.md` (mới) — M2 + M3, lead review
**Hướng làm:** Thống nhất khung mục lục trước (server / web / chung). Phần server (M2 viết):
layout module axum (router/handlers/repo/domain tách thế nào), error type thống nhất
(thiserror + IntoResponse), quy ước sqlx (query macro hay runtime, migration đặt tên),
async (KHÔNG block runtime — convert phải qua `spawn_blocking`/worker, bài học từ desktop),
logging (tracing). Phần web (M3 viết): kế thừa quy ước app desktop (TS strict,
komponent PascalCase, Zustand store duy nhất, không persist nội dung), thêm quy ước
gọi API (client tập trung, không fetch rải rác), quản lý token.
**Cần tìm hiểu:** đọc `docs/code-standards.md` (giọng văn + mức chi tiết làm chuẩn);
layout các project axum lớn (module per-resource vs layered).
**Tiêu chí xong:** lead duyệt; mọi PR sau bị review theo file này.
**Cạm bẫy:** đừng chép nguyên tắc chung chung từ internet — chỉ ghi quy ước dự án
NÀY thực sự cần, theo YAGNI như code-standards.md hiện có.

### C0.2. Port LumiBase token + ESLint/Prettier cho `web/` — M3
**Hướng làm:** Sau khi C2b tạo skeleton: copy token màu/spacing/font từ `app/src/styles.css`
sang `web/`, giữ tên biến CSS y hệt (đồng bộ 2 chiều dễ). Copy `eslint.config.js`,
`.prettierrc`, `.prettierignore` từ `app/` sang, chỉnh path.
**Cần tìm hiểu:** phần token nào của desktop phụ thuộc Tauri window (vd. titlebar) → bỏ.
**Tiêu chí xong:** `pnpm lint` + `pnpm format:check` pass trong `web/`; 1 trang demo render
đúng dark theme LumiBase.
**Cạm bẫy:** KHÔNG sửa `app/styles.css` trong lúc port — chỉ đọc.

---

## TRACK A — SPIKE & BENCHMARK (M1 chủ trì; không cần vững Rust, script/Python/docker được)

### A1. Docker-compose hạ tầng dev — `bench/web-spike/docker-compose.yml` (mới)
**Hướng làm:** Compose 3 service: PostgreSQL 16 (bật `unaccent` extension), Qdrant, MinIO.
Healthcheck từng service, named volume, port không đụng dải mặc định đang dùng. Kèm
`README.md` ghi lệnh up/down/xoá volume + biến env mẫu (`.env.example`, KHÔNG commit secret).
**Cần tìm hiểu:** image tag ổn định của 3 service; cách bật extension PG trong init script.
**Tiêu chí xong:** `docker compose up -d` → cả 3 healthy; PG có `unaccent`; MinIO console vào được.
**Cạm bẫy:** pin image tag cụ thể (không `latest`); volume phải named để benchmark sau
không mất data khi restart.

### A2. Golden-set tiếng Việt — `bench/web-spike/golden/` (mới)
**Hướng làm:** Chọn 20-30 tài liệu mẫu đa dạng đúng danh sách format POC (docx, xlsx,
pdf text, pdf scan, csv, md, txt, ảnh) → convert bằng `fileconv one` → chunk bằng logic
heading-path (nhờ lead export hàm `chunk_markdown` qua CLI nếu cần) → ~500-1K chunk.
Soạn 50-100 cặp câu hỏi + đáp án + chunk nguồn đúng (manifest TSV: `câu hỏi \t id chunk
đúng \t nhãn` — cùng convention manifest accuracy hiện có, `#` comment).
**Cần tìm hiểu:** format manifest ở `crates/cli` (accuracy); tiêu chí đo recall@k.
**Tiêu chí xong:** manifest + chunk chuẩn hoá nằm trong `bench/web-spike/golden/`; tài liệu
nguồn đại diện đủ loại khó (scan, bảng, IN HOA — xem `bench/REPORT_EDGE.md`).
**Cạm bẫy:** câu hỏi phải viết như người dùng thật hỏi (không copy nguyên văn câu trong
tài liệu — thế thì FTS luôn thắng và eval vô nghĩa); tài liệu nội bộ nhạy cảm không đưa
vào repo — gitignore thư mục data, chỉ commit manifest + script.

### A3. Eval GLM embedding trên golden-set — `bench/web-spike/eval_embedding.py` (mới) + `bench/REPORT_WEB_EMBEDDING.md`
**Chặn bởi:** A2 + GLM endpoint/API key (đang chờ user cung cấp).
**Hướng làm:** Script gọi GLM embedding API (OpenAI-compatible) embed toàn bộ chunk +
câu hỏi → cosine top-k → đo recall@1/5/10. Baseline so sánh: hash-local 256D (mô phỏng
theo thuật toán trong `vector_index.rs`, hoặc nhờ lead export qua CLI). Ghi kèm chi phí
token + latency per-batch vào report.
**Cần tìm hiểu:** API embedding của GLM (model nào, dimension, giới hạn batch/rate);
đọc `crates/core/src/llm.rs` phần embedding client để nhất quán cách gọi sau này.
**Tiêu chí xong:** report có bảng recall@k GLM vs hash-local + chi phí + latency;
lead + user chốt "đạt/không đạt" (GATE Track A phần embedding).
**Cạm bẫy:** rate limit API — batch nhỏ + retry, đừng bắn 1K request song song;
cache kết quả embed ra file để chạy lại eval không tốn tiền lần hai.

### A4. Benchmark Qdrant multi-tenant — `bench/web-spike/bench_qdrant.py` (mới) + `bench/REPORT_WEB_QDRANT.md`
**Chặn bởi:** A1; dimension vector lấy theo model chốt ở A3 (chạy trước bằng vector synthetic cùng dimension được).
**Hướng làm:** Nạp 1-5M vector synthetic chia ~10 org giả (payload `org_id, collection_id`),
bật scalar quantization + payload index → đo: search filter theo org P95, delete theo
filter, RAM trước/sau quantization, snapshot/restore. Song song đo PG FTS trên bảng
`chunks` giả cùng phân bố (query `unaccent` + `simple`).
**Cần tìm hiểu:** Qdrant client Python; cấu hình quantization + payload index; `EXPLAIN ANALYZE` PG.
**Tiêu chí xong:** report đủ các số trên; đề xuất cấu hình collection cho C1b.
**Cạm bẫy:** phân bố org phải lệch (1 org to chiếm 50% data) mới giống thật — phân bố
đều cho kết quả filter đẹp giả tạo.

### A5. Upload threat model — `bench/web-spike/THREAT_MODEL.md` (mới)
**Hướng làm:** Liệt kê bề mặt tấn công upload theo đúng danh sách format POC: extension
giả (đổi tên .exe→.pdf), zip-bomb trong docx/xlsx (là zip), decompression bomb ảnh,
PDF malformed làm panic converter (đã biết: lopdf panic — vì thế core mới bọc
catch_unwind), file quá size, path traversal tên file. Mỗi mục: cách chặn + chặn ở tầng
nào (trước MinIO / trước converter / trong sandbox) — làm input cho task hardening M1.
**Cần tìm hiểu:** `docs/code-standards.md` mục cạm bẫy PDF; magic bytes từng format.
**Tiêu chí xong:** bảng threat → mitigation → tầng chặn, lead duyệt; sample file độc
(zip-bomb nhỏ, extension giả) chuẩn bị sẵn cho test M1.
**Cạm bẫy:** docx/xlsx bản chất là zip — quy tắc chống zip-bomb phải áp cho cả chúng,
không chỉ file .zip.

---

## TRACK B — TÁCH `crates/knowledge` (Lead + AI agent — KHÔNG giao member)

### B1. Test khoá hành vi trước khi tách
`app/src-tauri/src/{knowledge,vector_index}.rs`: viết test cấp hành vi (index → search
hybrid → citation đúng; incremental hash; HNSW persist/reload) chạy được không cần Tauri
runtime. Tiêu chí: test pass trên code HIỆN TẠI, đủ nhạy để fail nếu logic rank/citation đổi.

### B2. Extract logic thuần → `crates/knowledge`
Chunk→rank (RRF/hybrid công thức hiện có)→citation, types chung. KHÔNG generic hoá
storage (Codex finding): desktop giữ SQLite/HNSW tại chỗ, chỉ phần thuần dời đi.
Tiêu chí: `cargo test` toàn workspace pass + desktop chạy đúng (gate Track B).

### B3. Interface embed/store tối thiểu theo nhu cầu server
Chỉ 2 trait/struct server thật sự cần (embed batch, upsert/search) — viết khi M1 bắt đầu
wire, không đoán trước.

---

## TRACK C — NỀN TẢNG

### C1a. PG schema + migration — `crates/server/migrations/` (mới) — M2, lead review
**Hướng làm:** Dựng migration sqlx theo đúng mục 4 của `docs/web-architecture.md`
(tenancy, RBAC, docs, jobs, quota, audit_log — mọi bảng nghiệp vụ có `org_id`).
Kèm seed script dev (1 org, 2 user, 1 collection). Làm TỪNG NHÓM BẢNG một, mỗi nhóm 1 PR
nhỏ để lead review kịp.
**Cần tìm hiểu:** sqlx migrate; PG partition theo `org_id` cho `chunks` (chỉ cần khai báo
partition-ready, chưa cần tách partition thật ở POC); kiểu `tsvector` + index GIN.
**Tiêu chí xong:** `sqlx migrate run` sạch trên PG của A1; seed chạy được; schema đúng
design doc (lead đối chiếu từng bảng).
**Cạm bẫy:** `content_hash` là BLAKE3 hex — cột text/bytea cố định, KHÔNG bigint
(bài học DefaultHasher desktop); `jobs` phải có `idempotency_key` unique ngay từ đầu.

### C1b. Qdrant collection config — M2, **chờ gate A3+A4**
Script/module init collection: dimension theo model GLM đã chốt, scalar quantization,
payload index `org_id`/`collection_id` theo đề xuất report A4.

### C2a. Skeleton `crates/server` — M2 + lead pair — `crates/server/` (mới)
**Hướng làm:** Thêm member workspace `crates/server`: axum + tokio, config từ env
(fail-fast thiếu biến), error type chung, `/healthz` (check PG + Qdrant + MinIO reachable),
tracing, layout module theo `docs/web-code-standards.md` (C0.1 phải xong phần khung trước).
CHƯA có business logic — chỉ khung + healthcheck.
**Cần tìm hiểu:** axum extractor/router cơ bản; sqlx pool; cách desktop cấu trúc error
(`ConvertError` thiserror) làm mẫu.
**Tiêu chí xong:** `cargo run -p fileconv-server` → `/healthz` trả trạng thái 3 service
từ compose A1; CI build pass; KHÔNG phá `cargo test` hiện có.
**Cạm bẫy:** thêm crate vào workspace members — kiểm tra `vendor/markitdown-rs` vẫn bị
exclude; đừng kéo dependency nặng "cho tương lai" (YAGNI).

### C2b. Skeleton `web/` — M3 — `web/` (mới)
**Hướng làm:** Vite + React + TS strict (mirror config `app/`), routing 4 trang placeholder
(login, thư viện, chat, admin), HTTP client tập trung (`web/src/lib/api.ts` — vai trò như
`ipc.ts` desktop nhưng gọi REST), Zustand store skeleton (auth state + tree placeholder).
Copy `SafeMarkdown` từ app desktop sang (component thuần React, không dính Tauri).
**Cần tìm hiểu:** `app/src/lib/ipc.ts` + `state/store.ts` (pattern giữ nguyên tinh thần);
phần nào của `ui.tsx` desktop độc lập Tauri để copy.
**Tiêu chí xong:** `pnpm dev` chạy, điều hướng 4 trang, gọi được `/healthz` của C2a hiển
thị trạng thái; lint pass (C0.2).
**Cạm bẫy:** đừng import gì từ `@tauri-apps/*`; copy component thì copy — KHÔNG tạo
package chung app/web ở giai đoạn này (tránh generic hoá sớm, đúng tinh thần Codex finding).

---

## M1 — INTEGRATION (chẻ chi tiết sau khi A/B/C xong; phân vai dự kiến)

| Đầu việc | Ai | Ghi chú |
|---|---|---|
| Tenant-scoped repository (`OrgContext`) | Lead + AI | Nền mọi data-access — làm ĐẦU TIÊN của M1 |
| Auth JWT + refresh token | Lead làm khung, M2 làm endpoint phụ | Module tách riêng, pluggable OIDC sau |
| Upload: MIME sniff magic-byte đối chiếu extension | M2 (task nhỏ có hướng dẫn) | Theo THREAT_MODEL A5; từ chối `.doc` với message hướng dẫn |
| Upload: size limit + zip-bomb + quarantine bucket | M2, từng task một | Sample độc từ A5 làm test |
| Worker convert: sandbox + timeout + kill | Lead + AI | Gọi fileconv-core qua `spawn_blocking`/process riêng |
| Document state machine + tombstone + reconciliation | Lead + AI | Vùng dễ bug nhất (risk #5) |
| Quota reserve→finalize/refund atomic | Lead + AI | MỘT cơ chế, không bản tạm; test 2 request đồng thời |
| Integration test M1 (kill worker → resume checkpoint) | M1 (QA) | Chuyển từ Track A sang Track T |

## Thứ tự & phụ thuộc tuần đầu

- **Ngày 1 song song:** A1 (M1) · C0.1 khung mục lục (M2+M3) · C2b (M3) · B1 (lead+AI).
- **Tiếp theo:** A2 (M1) · C1a (M2, sau khi C0.1 phần server có khung) · C0.2 (M3, sau C2b) · B2 (lead+AI).
- **Chờ input user:** A3 chờ GLM endpoint/key; A4 chạy synthetic trước, chốt config sau A3.
- **Điểm hội tụ:** A + B + C xong → họp chẻ task M1 chi tiết (bảng trên làm khung).

## Câu hỏi mở còn lại (lặp từ phase plan)

1. GLM endpoint + API key — chặn A3.
2. Máy chạy docker-compose dev/staging — chặn A1 nếu không dùng máy local.
