# Report khắc phục — hệ thống, lõi convert, OCR, RAG và embedding

Ngày review: 2026-07-25
Commit review: `ae1a1bd` (`HEAD` == `origin/master`)
Toolchain: `rustc 1.88.0` / `clippy 0.1.88` (đúng pin `rust-toolchain.toml`)

Phạm vi: toàn bộ workspace, đọc code trực tiếp tại commit trên. Trọng tâm theo
yêu cầu là `crates/core` (convert + OCR), `crates/knowledge` + đường RAG, và
lớp embedding. Mỗi finding kèm `file:line` đã đối chiếu trong code, một cách xử
lý cụ thể, và cách kiểm chứng sau khi sửa.

## Bằng chứng đã chạy

| Lệnh | Kết quả |
|---|---|
| `cargo build --workspace --all-targets` | Fail **chỉ** ở `fileconv-desktop` (thiếu `gdk-3.0` trong môi trường review; CI có cài GUI libs). Các crate còn lại build sạch. |
| `cargo test --workspace --exclude fileconv-desktop` | **632 passed, 0 failed, 127 ignored** / 35 test binary, exit 0 |
| `bash scripts/check-rust-quality.sh` | **FAIL, exit 101** — 3 vi phạm clippy |
| `cargo clippy -p fileconv-knowledge -p fileconv-server --all-targets -j1` | 3 warning, đều trong `crates/server/tests/` |
| CI run `30065276505` (master `ae1a1bd`) | 8/9 job pass; job **`rust` FAILED** tại step "Rust fmt, clippy, and tests". `rust-integration` (PG+Qdrant+MinIO thật, `--include-ignored`) **pass** |
| CI run `30055166714` (master `60ae6a98`, #305) | Cùng một job `rust` **FAILED** |

## Quy ước trong report này

- **Trạng thái** — `Đã xác minh` (chạy lệnh hoặc đọc code xác nhận) /
  `Suy luận` (đánh dấu `[Inference]`, cần đo để định lượng).
- **Effort** — ước lượng: `S` < 1 giờ, `M` nửa ngày–1 ngày, `L` nhiều ngày.
- Mọi claim về mức cải thiện độ chính xác đều là `[Inference]` cho tới khi có
  số đo từ `fileconv accuracy` / harness retrieval. Report này **không** khẳng
  định con số chưa đo.

## Tổng hợp finding

### Nhóm A — hệ thống, CI, hạ tầng

| ID | Finding | Mức | Effort | Trạng thái |
|---|---|---|---|---|
| A1 | Master CI đỏ 2 push liên tiếp — 3 vi phạm clippy trong test file server | Critical | S | Đã xác minh |
| A2 | Không có scan advisory dependency (853 crate, không `cargo-audit`/Dependabot) | High | S | Đã xác minh |
| A3 | `CLAUDE.md` bỏ 70% codebase; `codebase-summary.md` lệch 19×; roadmap tự mâu thuẫn | High | M | Đã xác minh |
| A4 | Quota reserve **sau** khi nhận hết body — ngược policy | Medium | M | Đã xác minh |
| A5 | Desktop frontend 11.5k LOC, 0 component test | Medium | L | Đã xác minh |
| A6 | `/metrics` không auth trên cùng listener API | Low | S | Đã xác minh |
| A7 | Rate limiter in-process, không distributed | Low | M | Đã xác minh (self-documented) |
| A8 | `find_user_org` quét mọi org, chọn org đầu tùy ý | Low | M | Đã xác minh |
| A9 | Baseline clippy còn entry cho path đã xoá `conv/pdf.rs` | Low | S | Đã xác minh |
| A10 | `intelligence.rs` 3 340 dòng, hàm dài nhất 206 dòng | Low | M | Đã xác minh |

### Nhóm B — lõi convert, OCR, RAG, embedding

| ID | Finding | Mức | Effort | Trạng thái |
|---|---|---|---|---|
| B1 | `OCR_DPI=300` bị `MAX_LONG_SIDE=2400` hạ xuống ~205 DPI trên mọi trang PDF | Critical (chất lượng) | S | Đã xác minh (tính từ hằng số) |
| B2 | `normalize()` dùng min/max toàn cục → gần như no-op trên scan thật | High | S | Đã xác minh |
| B3 | `heading_token_hits` khớp substring **và** không chuẩn hoá → lệch trọng số ranking | High | S | Đã xác minh |
| B4 | OCR đánh giá `n=1` mỗi kịch bản, toàn bộ font-render; chưa đo end-to-end qua OCR | High (bằng chứng) | L | Đã xác minh |
| B5 | `detect_column_ranges` dùng ngưỡng mực cứng 205, phụ thuộc B2 | Medium | M | Đã xác minh |
| B6 | Bảng Markdown là một "đoạn" → cắt cứng, chunk sau mất header row | Medium | M | Đã xác minh |
| B7 | Lexical desktop `OR` vs server `AND` — ngữ nghĩa query rẽ nhánh | Medium | M | Đã xác minh |
| B8 | Chưa có chỗ đặt `query:`/`passage:` prefix cho họ E5 khi cutover vLLM | Medium (rủi ro tương lai) | S | Đã xác minh |
| B9 | Không có bước deskew trong `preprocess` | Low | M | Đã xác minh |
| B10 | `ocr_text_score` là heuristic thưởng độ dài; bỏ không dùng confidence Tesseract | Low | M | Đã xác minh |
| B11 | Trang 2 cột spawn Tesseract 3–5 lần (pass toàn trang chạy vô điều kiện) | Low | S | Đã xác minh |
| B12 | `body_token_overlap` chuẩn hoá lại full body cho mỗi candidate mỗi query | Low | M | Đã xác minh |
| B13 | `pre.to_luma8()` clone thừa toàn bộ buffer mỗi lần OCR | Low | S | Đã xác minh |
| B14 | Chunk không có overlap | Low | M | Đã xác minh |

---

# Nhóm A — hệ thống, CI, hạ tầng

## A1 — Master CI đỏ: 3 vi phạm clippy trong test file server

- **Mức độ:** Critical · **Effort:** S · **Trạng thái:** Đã xác minh
- **Bằng chứng:**
  - `crates/server/tests/retrieval_vertical_slice.rs:365` — `clippy::uninlined_format_args`
  - `crates/server/tests/api_http_contracts.rs:880` — `clippy::redundant_locals`
  - `crates/server/tests/api_http_contracts.rs:1045` — `clippy::redundant_locals`
  - `bash scripts/check-rust-quality.sh` → exit 101
  - CI run `30065276505` job `rust` = failure; run `30055166714` (#305) cùng lỗi

**Tác động.** `AGENTS.md:10` yêu cầu chạy các gate này trước mỗi lần push Rust,
và `make check-rust` là gate được document. Baseline không xanh làm mất tín hiệu
của toàn bộ hệ thống gate còn lại: lần đỏ tiếp theo không phân biệt được là lỗi
mới hay lỗi cũ còn tồn.

**Nguyên nhân gốc.** `scripts/run-rust-ci-fast.sh:11-16` giới hạn phạm vi lint
theo scope thay đổi:

```bash
clippy_args=(--lib)
if [[ "$RUST_CRATES" == "full" || "$INTEGRATION" == "true" ]]; then
  clippy_args=(--all-targets)
fi
```

Trên PR không phải `full` scope, `--lib` **không lint `tests/*.rs`**. Hai PR
#305/#306 thêm harness vào đúng các file đó nên lọt qua PR; master push
(`INTEGRATION=true`) bắt được nhưng vẫn merge.

**Cách xử lý.**

Bước 1 — sửa 3 vi phạm:

```rust
// crates/server/tests/api_http_contracts.rs:876-884 và :1041-1049
// Uuid là Copy; `move` closure đã copy sẵn nên rebinding là dư. Bỏ cả block wrapper.
-    let audit_after: i64 = with_org_txn(&pool, &ctx, {
-        let org = org;
-        move |txn| {
-            Box::pin(async move {
+    let audit_after: i64 = with_org_txn(&pool, &ctx, move |txn| {
+        Box::pin(async move {
```

```rust
// crates/server/tests/retrieval_vertical_slice.rs:365-371
-            "{ext} unexpected convert outcome: {convert_run:?}; last_error={:?}",
-            convert_last_error
+            "{ext} unexpected convert outcome: {convert_run:?}; last_error={convert_last_error:?}"
```

Bước 2 — bịt lỗ scope để không tái diễn. Lint test file rẻ so với compile, nên
bỏ hẳn nhánh `--lib`:

```bash
# scripts/run-rust-ci-fast.sh
-clippy_args=(--lib)
-if [[ "$RUST_CRATES" == "full" || "$INTEGRATION" == "true" ]]; then
-  clippy_args=(--all-targets)
-fi
-cargo clippy --no-deps -p fileconv-knowledge -p fileconv-server "${clippy_args[@]}" -- -D warnings
+# Luôn --all-targets: lint tests/ trên PR, không đợi tới master push mới phát hiện.
+cargo clippy --no-deps -p fileconv-knowledge -p fileconv-server --all-targets -- -D warnings
```

Bước 3 — bật branch protection required check cho job `rust` để master không
merge được khi job này đỏ.

**Kiểm chứng.** `bash scripts/check-rust-quality.sh` → exit 0;
`cargo clippy --no-deps -p fileconv-knowledge -p fileconv-server --all-targets -- -D warnings`
→ 0 warning. CI run kế tiếp trên master: job `rust` xanh.

**Ảnh hưởng gate.** Không. Chỉ sửa test file và cấu hình lint, không đổi hành vi
runtime, không đụng index signature.

---

## A2 — Không có scan advisory dependency

- **Mức độ:** High · **Effort:** S · **Trạng thái:** Đã xác minh
- **Bằng chứng:** `Cargo.lock` có **853** `[[package]]`. Không có `deny.toml`,
  không `.github/dependabot.yml`. `scripts/check-dependency-policy.py` chỉ chặn
  git-dep, thiếu license metadata, và action không pin SHA — không đối chiếu
  advisory database. Grep `docs/markhand-web-risk-register.md`,
  `docs/conventions/dependencies.md`, `plans/markhand-web/phase-4-production-hardening.md`
  cho `cargo-audit|RUSTSEC|CVE|dependabot|renovate` → 0 kết quả, tức đây **chưa
  phải** accepted risk có chủ đích.

**Tác động.** Sản phẩm on-prem xử lý file do người dùng upload, parse PDF/ZIP/ảnh
bằng nhiều crate C-backed (`pdfium-render`, `pdf-extract`, `image`, `zip`). Đây
đúng là lớp dependency hay có advisory. Không có gate nào phát hiện.

**Cách xử lý.**

```yaml
# .github/workflows/ci.yml — thêm job (pin SHA theo check-dependency-policy.py)
  audit:
    needs: changes-and-static
    if: needs.changes-and-static.outputs.rust == 'true'
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5 # v4
      - uses: rustsec/audit-check@<pin-commit-sha>
        with:
          token: ${{ secrets.GITHUB_TOKEN }}
```

Kèm `deny.toml` để khoá cả license + nguồn, nhất quán với policy đã có:

```toml
[advisories]
version = 2
yanked = "deny"
# ignore = ["RUSTSEC-XXXX-XXXX"]  # mỗi entry phải có owner + expiry, theo DoD Phase 1B

[bans]
multiple-versions = "warn"

[sources]
unknown-registry = "deny"
unknown-git = "deny"
```

Thêm target vào `Makefile` để chạy local đồng nhất với CI:

```make
check-advisories:
	cargo deny --locked check advisories bans sources
```

Và mở `.github/dependabot.yml` cho `cargo` + `github-actions` (weekly) để
advisory có đường vá tự động.

**Kiểm chứng.** `cargo deny --locked check advisories` → exit 0, hoặc mọi
finding còn lại đều có entry `ignore` kèm owner/expiry theo Definition of Done
trong `plans/markhand-web/README.md:131-132`.

**Ảnh hưởng gate.** Thêm gate mới. Có thể đỏ ngay lần đầu — nên chạy trước để
biết khối lượng thật, rồi mới bật blocking.

---

## A3 — Tài liệu entry-point bỏ 70% codebase

- **Mức độ:** High · **Effort:** M · **Trạng thái:** Đã xác minh
- **Bằng chứng:**
  - `CLAUDE.md` — grep `server|knowledge|web/|deploy|postgres|qdrant|minio|make check`
    → **0 kết quả**. File mô tả dự án như một backend convert đơn lẻ.
  - `docs/codebase-summary.md:27` ghi `~5 669 LOC / 43 file`. Thực tế
    `git ls-files '*.rs' | xargs wc -l` = **109 226 LOC / 201 file** (lệch 19×).
    Bảng file map trỏ `conv/pdf.rs`; thực tế là directory `conv/pdf/` gồm 8 file.
  - `docs/project-roadmap.md:84` liệt kê **"Đa người dùng / đồng bộ cloud"** trong
    mục *"Không ưu tiên (theo YAGNI)"* — mâu thuẫn trực tiếp với `crates/server`
    (76 439 LOC, 70% codebase).
  - `AGENTS.md:5` gọi `fileconv-server` và `web/` là "foundation scaffolds".

**Tác động.** Đây là hợp đồng mà agent và người mới đọc đầu tiên. Theo đúng
`CLAUDE.md` thì không biết `crates/server`, `crates/knowledge`, hệ thống
`make check-*`, hay `plans/markhand-web/` tồn tại. Kế hoạch thật ở
`plans/markhand-web/` rất tốt — vấn đề chỉ là entry-point không trỏ tới đó.

**Cách xử lý.** Không viết lại tài liệu; chỉ thêm cầu nối và sửa số liệu sai.

1. `CLAUDE.md` — thêm một section ngắn ngay sau `## Dự án`:

```markdown
## Hai sản phẩm trong repo này

- **Desktop/CLI/MCP offline** (`crates/core`, `crates/cli`, `crates/mcp`, `app/`)
  — sản phẩm đã ship v0.1.0. Ưu tiên: độ chính xác nội dung tiếng Việt.
- **Markhand Web** (`crates/server` 76k LOC, `crates/knowledge`, `web/`, `deploy/`)
  — RAG multi-org on-prem: PostgreSQL (system of record) + Qdrant + MinIO + worker.
  Kế hoạch và trạng thái theo phase: `plans/markhand-web/README.md`.
  ADR: `docs/adr/`. Gate: `make check-static`, `make check-rust`, `make check-web`.

Trước khi push Rust: xem `AGENTS.md` và `docs/runbooks/contributor-setup.md`.
```

2. `docs/codebase-summary.md` — cập nhật bảng quy mô bằng số đo lại, thêm dòng
   cho `crates/server` / `crates/knowledge` / `web/` / `deploy/`, sửa
   `conv/pdf.rs` → `conv/pdf/` (8 file).
3. `docs/project-roadmap.md` — bỏ "Đa người dùng / đồng bộ cloud" khỏi mục YAGNI,
   thay bằng con trỏ tới `plans/markhand-web/README.md`, và ghi rõ roadmap này
   chỉ phủ sản phẩm desktop/CLI.
4. `AGENTS.md:5` — bỏ chữ "foundation scaffolds" cho `fileconv-server`.

**Kiểm chứng.** Đọc lại `CLAUDE.md` và trả lời được: server nằm ở đâu, chạy gate
bằng lệnh gì, trạng thái phase tra ở đâu. Cân nhắc thêm assert vào
`scripts/check-markhand-gates.py` rằng `CLAUDE.md` có tham chiếu
`plans/markhand-web/` để không lệch lại.

**Ảnh hưởng gate.** Không (chỉ tài liệu). Lưu ý `make check-roadmap`
(`build-roadmap.py --check`) đọc registry phase — đổi `docs/project-roadmap.md`
không ảnh hưởng, nhưng nên chạy lại cho chắc.

---

## A4 — Quota reserve sau khi nhận hết body

- **Mức độ:** Medium · **Effort:** M · **Trạng thái:** Đã xác minh
- **Bằng chứng:** `crates/server/src/routes/uploads.rs:109` stream toàn bộ
  multipart vào tempfile (`read_multipart`) trước; saga chỉ reserve quota ở
  `crates/server/src/services/upload/saga.rs:332` (`quota_reserve_hook`), sau
  `transition_reserved` ở `:329`.
- **Lịch sử:** đã nêu là finding #5 trong
  `code-review-260720-1208-markhand-web-phase1b-batch2-report.md`, **chưa sửa**.

**Tác động.** Tenant đã hết quota vẫn tiêu trọn băng thông + disk tempfile mỗi
request trước khi bị từ chối. Ngược thứ tự policy (reserve trước stream).

**Giảm nhẹ hiện có.** Rate limit per-user và per-route đã được implement
(`routes/uploads.rs:76-87`), cộng `max_upload_bytes` và idle timeout — nên tác
động bị chặn trên. **Nhưng** `crates/server/src/middleware/rate_limit.rs:1` tự
khai `In-process token-bucket … Not distributed`: với N replica, limit hiệu dụng
là N× cấu hình, nên không thể coi rate limit là mitigation ở scale (xem A7).

**Cách xử lý.** Thêm một lần admission check *rẻ* trước khi stream, giữ nguyên
reservation atomic trong saga (không thay thế nó — chỉ chặn sớm ca chắc chắn
thất bại):

```rust
// routes/uploads.rs — trước read_multipart
// Content-Length là hint do client cung cấp: chỉ dùng để FAIL SỚM, không bao giờ
// dùng để cho qua. Reservation atomic trong saga vẫn là nguồn sự thật.
if let Some(declared) = headers
    .get(axum::http::header::CONTENT_LENGTH)
    .and_then(|value| value.to_str().ok())
    .and_then(|value| value.parse::<u64>().ok())
{
    if quota::would_exceed_hard_cap(state.pool(), &auth.context, declared).await? {
        return Err(UploadRouteError::Upload(
            UploadError::QuotaExceeded,
            request_id.clone(),
        ));
    }
}
```

Nếu không muốn thêm code path: cập nhật `docs/markhand-web-upload-policy.md` cho
khớp thực tế và ghi accepted risk có owner/expiry theo DoD. Quyết định thuộc
maintainer — hai hướng đều hợp lệ, nhưng hiện trạng là policy và code lệch nhau
mà không có ghi chú.

**Kiểm chứng.** Test: tenant ở sát hard cap gửi request có `Content-Length` vượt
→ nhận 4xx **trước** khi body được đọc hết (assert tempfile không được tạo, hoặc
đếm byte đã đọc). Test hiện có về double-spend trong `quota.rs` phải vẫn xanh.

**Ảnh hưởng gate.** Đổi contract HTTP (thêm nhánh từ chối sớm) → chạy lại
`crates/server/tests/uploads.rs`, `quota.rs`, `phase1b_api_contracts.rs`.

---

## A5 — Desktop frontend không có component test

- **Mức độ:** Medium · **Effort:** L · **Trạng thái:** Đã xác minh
- **Bằng chứng:** `app/src` có 11 497 LOC (TS/TSX/CSS) và **17** component
  (`app/src/components/*.tsx`), nhưng chỉ 5 file test, **toàn bộ là helper thuần**:
  `lib/intelligenceUtils.test.ts`, `lib/knowledgeContract.test.ts`,
  `lib/llmSettings.test.ts`, `lib/markdownBlocks.test.ts`, `lib/tree.test.ts`.
  0 component test. Component lớn nhất `IntelligenceView.tsx` = 1 207 dòng,
  `Settings.tsx` = 762 dòng. Đối chiếu: `crates/server` có 444 test fn.

**Tác động.** Sản phẩm đang thực sự phát hành cho người dùng lại có lưới an toàn
mỏng nhất trong repo.

**Cách xử lý.** `vitest` đã có sẵn (`app/package.json:13,58`); chỉ cần thêm
`@testing-library/react` + `jsdom` và bắt đầu từ đường có rủi ro cao nhất, không
cố phủ hết:

1. `SafeMarkdown` — test sanitizer thật sự chặn: `<script>`, `onerror=`,
   `javascript:` href, và **giữ** `colSpan`/`rowSpan` (đường bảng merge từ
   `conv/xlsx.rs`/`conv/docx.rs`). Đây là bề mặt bảo mật, nên test trước.
2. `Settings.tsx` — round-trip: đổi setting → `set_settings` được gọi với payload
   đúng → reload giữ giá trị. Mock `lib/ipc.ts`.
3. `DocView.tsx` — Save/Reconvert gọi đúng command, và draft không mất khi đổi tab.
4. `Tree.tsx` — ghép cặp `report.pdf` ↔ `report.pdf.md`, `standaloneMd`.

```jsonc
// app/package.json — thêm devDependencies
"@testing-library/react": "^16",
"@testing-library/jest-dom": "^6",
"jsdom": "^25"
```

Sau đó bổ sung `pnpm --filter markhand-desktop test` vào gate `frontend` (đã có
trong `Makefile: check-desktop`) — không cần đổi CI.

**Kiểm chứng.** `make check-desktop` xanh với ≥4 component test mới. Đề xuất đặt
ngưỡng coverage tối thiểu cho `components/` rồi ratchet dần, giống cách
`check-rust-lint-baseline.py` làm với clippy.

**Ảnh hưởng gate.** Không đụng backend.

---

## A6 — `/metrics` không auth trên cùng listener API

- **Mức độ:** Low · **Effort:** S · **Trạng thái:** Đã xác minh
- **Bằng chứng:** `crates/server/src/routes/health.rs:30` mount
  `/metrics` → `metrics_export`, chỉ gate bằng
  `MetricsRegistry::metrics_enabled()` (`:34`). Không có extractor auth, không IP
  allowlist. Các route khác dùng extractor `AuthenticatedOrg` (vd
  `routes/jobs.rs:23`) nên pattern chung là an toàn — đây là ngoại lệ.
- **Hiện chưa hở:** `deploy/compose.poc.yml:255,357,409,453,504` bind
  `MARKHAND_BIND_ADDR: 127.0.0.1:8787`; port map ở `:318` là
  `127.0.0.1:${MARKHAND_API_PORT:-8788}:8787`.

**Tác động.** Trong POC không reachable từ ngoài. Khi đặt sau ingress ở production
thì metric (cardinality theo org, số document, số job) thành thông tin nội bộ lộ
ra ngoài. `docs/conventions/observability-audit.md:60` chỉ mô tả có endpoint, chưa
nêu control nào bảo vệ nó.

**Cách xử lý.** Chọn một, ghi vào convention doc:

- *(gọn nhất)* Tách listener riêng cho metrics, bind chỉ vào interface nội bộ:
  `MARKHAND_METRICS_BIND_ADDR` mặc định `127.0.0.1:9090`, không mount `/metrics`
  trên router chính.
- Hoặc giữ nguyên route nhưng thêm guard bearer-token riêng
  (`MARKHAND_METRICS_TOKEN`) so sánh constant-time.
- Hoặc ghi rõ trong `docs/conventions/observability-audit.md` +
  `plans/markhand-web/phase-4-production-hardening.md` rằng control nằm ở tầng
  ingress, kèm ví dụ cấu hình.

**Kiểm chứng.** Test: `GET /metrics` không kèm credential trên listener công khai
→ 404/401. Test scrape nội bộ vẫn 200.

**Ảnh hưởng gate.** Nếu đổi bind: cập nhật `deploy/observability/prometheus`
scrape config và `deploy/scripts/health.sh`.

---

## A7 — Rate limiter in-process, không distributed

- **Mức độ:** Low · **Effort:** M · **Trạng thái:** Đã xác minh (tự document)
- **Bằng chứng:** `crates/server/src/middleware/rate_limit.rs:1` —
  `//! In-process token-bucket rate limiter (P1B-R06). Not distributed.`
  `HARD_CAP_KEYS = 10_000` (`:7`), state là `Arc<Mutex<HashMap<..>>>` (`:3-4`).

**Tác động.** Với N replica API, limit hiệu dụng là N× giá trị cấu hình. Bản thân
điều này đã được khai báo trung thực nên không phải bug. Vấn đề là **nó đang được
tính ngầm như mitigation cho A4** — nếu deploy nhiều replica thì mitigation đó
yếu đi đúng theo N.

**Cách xử lý.** Không cần đổi code cho POC single-replica. Cần:
1. Ghi liên kết A4 ↔ A7 vào risk register: rate limit chỉ chặn A4 ở
   single-replica.
2. Thêm mục vào `plans/markhand-web/phase-4-production-hardening.md`: chuyển
   sang limiter chia sẻ (Redis token bucket hoặc rate limit ở ingress) **trước**
   khi scale ngang.

**Kiểm chứng.** Risk register có entry với owner + điều kiện kích hoạt
("khi replica > 1").

---

## A8 — `find_user_org` quét mọi org và chọn org đầu tùy ý

- **Mức độ:** Low · **Effort:** M · **Trạng thái:** Đã xác minh
- **Bằng chứng:** `crates/server/src/auth/session.rs:356-383`. Hàm
  `SELECT id FROM orgs ORDER BY created_at` rồi **mỗi org một transaction**
  (`:364-377`, cần vì `SET LOCAL` chỉ sống trong transaction tường minh), trả về
  org khớp **đầu tiên** (`:378-380`).

**Tác động.** Hai vấn đề:
- Chi phí login là O(số org), mỗi org một round-trip transaction.
- User thuộc nhiều org sẽ được gán **một org tùy ý** theo `created_at`, im lặng.

Đây là nợ có chủ đích của phạm vi single-org Phase 1B, và
`plans/markhand-web/phase-1c-multi-org-security.md:8-17` đã có kế hoạch thay thế
("Tạo/join/switch org", "Org context lấy từ route/header đã validate với
membership, không tin claim do client tự chọn"). Rủi ro là nếu 1C trượt mà org
thứ hai đã được tạo thì hành vi sai âm thầm.

**Cách xử lý.** Không refactor sớm (1C sẽ thay). Thêm hàng rào rẻ ngay bây giờ để
biến "sai âm thầm" thành "lỗi rõ ràng":

```rust
// auth/session.rs — trong find_user_org, sau khi đã tìm được match
// Phase 1B là single-org. Nếu user thuộc nhiều org mà chưa có org switch (1C),
// chọn org tùy ý là sai âm thầm → fail closed.
if matches.len() > 1 {
    return Err(SessionError::MembershipAmbiguous);
}
```

Kèm một query gộp thay cho loop, nếu muốn xử luôn phần chi phí — cần RLS cho
phép, nên gói trong đúng một transaction có `app.org_id` chưa set và dựa vào
`org_memberships` policy, hoặc dùng role đọc riêng đã được ADR 0007 phê duyệt.
**Không** nới RLS chỉ để tối ưu login.

**Kiểm chứng.** Test: user có membership ở 2 org → login trả lỗi tường minh, không
trả token cho org tùy ý. Test single-org hiện có vẫn xanh.

**Ảnh hưởng gate.** Thêm biến thể lỗi auth → cập nhật `crates/server/tests/auth.rs`
và bảng mã lỗi trong `docs/conventions/api.md` nếu có.

---

## A9 — Baseline clippy còn entry cho path đã xoá

- **Mức độ:** Low · **Effort:** S · **Trạng thái:** Đã xác minh
- **Bằng chứng:** `scripts/check-rust-lint-baseline.py:18` —
  `("clippy::field_reassign_with_default", "crates/core/src/conv/pdf.rs"): 1`.
  `crates/core/src/conv/pdf.rs` không còn tồn tại; đã tách thành directory
  `crates/core/src/conv/pdf/` (8 file, 2 453 dòng).

**Tác động.** Vô hại về mặt gate (script làm `current - allowed`, allowance không
dùng thì bị bỏ qua) nhưng là cấu hình chết, và làm baseline trông như còn nợ ở
chỗ không còn nợ.

**Cách xử lý.** Xoá dòng đó. Nếu sau khi tách file mà lint chuyển sang một file
con thì thêm entry mới đúng path (`conv/pdf/mod.rs` hoặc file tương ứng) — chạy
`python3 scripts/check-rust-lint-baseline.py` để biết chính xác.

Đề xuất thêm: cho script fail khi baseline chứa path không tồn tại, để entry chết
không tích tụ:

```python
for (_code, file_name) in LEGACY_WARNINGS:
    if not (ROOT / file_name).is_file():
        print(f"baseline references missing file: {file_name}", file=sys.stderr)
        return 1
```

**Kiểm chứng.** `python3 scripts/check-rust-lint-baseline.py` và
`--self-test` đều exit 0.

---

## A10 — `intelligence.rs` quá lớn

- **Mức độ:** Low · **Effort:** M · **Trạng thái:** Đã xác minh
- **Bằng chứng:** `crates/core/src/intelligence.rs` = 3 340 dòng, 104 item
  top-level, hàm dài nhất `extract_handoff_items` = 206 dòng, kế tiếp
  `render_handoff_artifacts` = 140, `validate_handoff` = 124. `clippy.toml` đặt
  `cognitive-complexity-threshold = 25`. Test đi kèm nằm riêng ở
  `intelligence_tests.rs` (1 346 dòng).

**Tác động.** Không phải bug. Là chi phí bảo trì và là nơi tập trung 4/20 warning
trong legacy baseline (`check-rust-lint-baseline.py:22-25`).

**Cách xử lý.** Áp dụng đúng cách đã dùng thành công cho `conv/pdf/`: tách theo
trục chức năng, giữ `pub` API nguyên vẹn để không đụng call site ở
`crates/server` (`build_corpus`, `normalize_search_text`, `page_before`) và
`app/src-tauri`:

```
crates/core/src/intelligence/
├── mod.rs          # re-export, giữ đúng public surface hiện tại
├── handoff.rs      # extract_handoff_items, render_handoff_artifacts, validate_handoff
├── corpus.rs       # build_corpus, page_before, normalize_search_text
├── pii.rs          # redact_pii + span coalescing
└── quality.rs      # scoring / schema / tables
```

Làm sau khi B3/B6 đã xong để tránh conflict, vì hai finding đó cũng đụng
`normalize_search_text` call site.

**Kiểm chứng.** `cargo test -p fileconv-core` xanh không đổi;
`cargo test -p fileconv-desktop` và `-p fileconv-server` xanh (chứng minh public
API không đổi); baseline clippy cập nhật đúng path mới.

---

# Nhóm B — lõi convert, OCR, RAG, embedding

## B1 — `OCR_DPI=300` bị `MAX_LONG_SIDE=2400` hạ xuống ~205 DPI

- **Mức độ:** Critical (chất lượng) · **Effort:** S · **Trạng thái:** Đã xác minh
  (tính từ hằng số trong code)
- **Bằng chứng:**
  - `crates/core/src/conv/pdf/ocr.rs:13` — `const OCR_DPI: f32 = 300.0;`
    kèm comment *"cao hơn = OCR tốt hơn, chậm hơn"*
  - `crates/core/src/conv/pdf/ocr.rs:117-125` — `ocr_full_page` render theo
    `OCR_DPI` rồi gọi `image_ocr::ocr_dynimage_detailed`
  - `crates/core/src/image_ocr.rs:298` — `ocr_dynimage_detailed` gọi `preprocess`
  - `crates/core/src/image_ocr.rs:203` — `const MAX_LONG_SIDE: u32 = 2400;`
  - `crates/core/src/image_ocr.rs:336-344` — nhánh `long > MAX_LONG_SIDE` resize xuống

**Tác động.** Với A4 (595.28 × 841.89 pt):

| Bước | Kích thước | DPI hiệu dụng |
|---|---|---|
| `page.render(w, h)` @ `OCR_DPI=300` | 2480 × 3508 | 300 |
| `preprocess`: `long=3508 > 2400` → `f = 2400/3508 = 0.684` | **1697 × 2400** | **≈205** |

Mọi trang PDF đi qua đường OCR đều bị Lanczos-downsample ~32% trước khi vào
Tesseract. `OCR_DPI = 300` bị vô hiệu hoá: vừa tốn thời gian render ở độ phân
giải không dùng tới, vừa mất chi tiết.

`[Inference]` Tác động lên tiếng Việt nặng hơn tiếng Anh vì dấu thanh/dấu mũ là
chi tiết vài pixel, mất trước tiên khi downsample. Số đo của chính dự án ủng hộ
hướng này: `bench/REPORT_ACCURACY.md:13` cho `image-lowres-OCR` = **81.0%**
(CER 0.190) so với `image-print-OCR` = 98.5% (`:12`). **Chưa** có thí nghiệm đo
delta trực tiếp cho finding này; cần B4 để định lượng.

**Nguyên nhân gốc.** Trần 2400 sinh ra cho **ảnh chụp quá lớn** — đúng như
comment ở `image_ocr.rs:11` (*"Ảnh quá lớn (cạnh dài > 2400px) → thu xuống (giữ
tốc độ)"*) — nhưng `ocr_full_page` dùng chung đúng hàm `preprocess` đó, nên trần
speed-guard áp cả lên trang tài liệu. Hai hằng số đều hợp lý khi đọc riêng.

**Vì sao gate không bắt được.** `bench/markhand_web/scripts/generate_corpus.py:580`
tạo ảnh scan ở `Image.new("RGB", (1800, 2400))` — cạnh dài **đúng bằng 2400**, nên
`long > MAX_LONG_SIDE` là `false`, không downscale. 3 fixture `image_ocr` (`:54`)
đi thẳng đường PNG, không bao giờ chạm nhánh này. 2 fixture `pdf_scan` (`:47`) có
chạm, nhưng nguồn của chúng vốn là ảnh 1800px được `write_scan_pdf` (`:637-650`)
nhúng vào A4, nên PDFium upscale 1800→2480 rồi `preprocess` downscale về 1697 —
mất mát nhỏ và chỉ trên 2/27 document. Corpus **under-weight** finding này chứ
không mù hẳn.

**Cách xử lý.** Cho trang tài liệu một trần riêng, tách khỏi trần ảnh chụp.

```rust
// crates/core/src/image_ocr.rs
- fn preprocess(img: &DynamicImage) -> DynamicImage {
+ fn preprocess(img: &DynamicImage, max_long_side: u32) -> DynamicImage {
      ...
-     } else if long > MAX_LONG_SIDE {
-         let f = MAX_LONG_SIDE as f32 / long as f32;
+     } else if long > max_long_side {
+         let f = max_long_side as f32 / long as f32;

+ /// OCR ảnh in-memory với trần cạnh dài tuỳ biến.
+ ///
+ /// Trang tài liệu render từ PDF cần trần cao hơn ảnh chụp: Tesseract hoạt động
+ /// tốt nhất quanh 300 DPI, và A4 @300 DPI là 3508px cạnh dài.
+ pub fn ocr_dynimage_with_max_side(
+     img: &DynamicImage,
+     langs: &str,
+     config: &OcrRunConfig,
+     max_long_side: u32,
+ ) -> Result<String, OcrAttemptError> { /* thân hiện tại, truyền max_long_side */ }
+
+ pub fn ocr_dynimage_detailed(
+     img: &DynamicImage, langs: &str, config: &OcrRunConfig,
+ ) -> Result<String, OcrAttemptError> {
+     ocr_dynimage_with_max_side(img, langs, config, MAX_LONG_SIDE)
+ }
```

```rust
// crates/core/src/conv/pdf/ocr.rs
+ /// Trang A4 @300 DPI là 3508px cạnh dài. Trần riêng cho trang tài liệu, KHÔNG
+ /// dùng MAX_LONG_SIDE (trần đó là speed-guard cho ảnh chụp).
+ const PAGE_MAX_LONG_SIDE: u32 = 3600;

  // trong ocr_full_page
- let text = image_ocr::ocr_dynimage_detailed(&img, langs, ocr_config)?;
+ let text =
+     image_ocr::ocr_dynimage_with_max_side(&img, langs, ocr_config, PAGE_MAX_LONG_SIDE)?;
```

Nếu ưu tiên giữ tốc độ hơn độ chính xác, phương án thay thế là **đừng render
thừa** — tính DPI từ trần thay vì render 300 DPI rồi cắt:

```rust
// conv/pdf/ocr.rs — biến thể tiết kiệm: render đúng độ phân giải sẽ OCR
let dpi = (MAX_LONG_SIDE as f32 * 72.0 / page.height().value.max(1.0)).min(OCR_DPI);
```

Hai phương án đối lập về đánh đổi; **nên quyết bằng số đo từ B4**, không quyết
bằng suy luận. Điểm chung của cả hai là bỏ được vòng upscale-rồi-downscale hiện tại.

**Kiểm chứng.**
1. Unit test khẳng định trang A4 @300 DPI **không** bị downscale:
   ```rust
   #[test]
   fn a4_page_at_300dpi_is_not_downscaled_for_ocr() {
       let img = DynamicImage::ImageLuma8(GrayImage::from_pixel(2480, 3508, Luma([255])));
       let pre = preprocess(&img, PAGE_MAX_LONG_SIDE);
       assert_eq!((pre.width(), pre.height()), (2480, 3508));
   }
   ```
2. Thêm fixture corpus ở 2481×3508 để gate nhìn thấy nhánh này (xem B4).
3. Đo lại: `./target/release/fileconv accuracy <manifest.tsv>` trên corpus scan
   trước/sau, báo cáo CER/WER. **Chỉ khi có số này mới được khẳng định mức cải thiện.**
4. Đo tốc độ: `fileconv speed` — ghi lại delta ms/page, vì bỏ downscale làm
   Tesseract xử ảnh lớn hơn.

**Ảnh hưởng gate.** Đổi output OCR ⇒ Markdown của tài liệu scan đổi ⇒ reconvert
sinh **version mới** (đúng semantics versioning, không phải migration). **Không**
đụng index signature (`heading-chunks-2000-v1` phụ thuộc thuật toán chunking, không
phụ thuộc nội dung). Cần chạy lại `make check-corpus` và gate retrieval nếu fixture
corpus thay đổi.

---

## B2 — `normalize()` dùng min/max toàn cục, gần như no-op trên scan thật

- **Mức độ:** High · **Effort:** S · **Trạng thái:** Đã xác minh
- **Bằng chứng:** `crates/core/src/image_ocr.rs:357-369`

```rust
fn normalize(buf: &mut GrayImage) {
    let (mut lo, mut hi) = (255u8, 0u8);
    for p in buf.pixels() { lo = lo.min(p[0]); hi = hi.max(p[0]); }   // min/max TOÀN CỤC
    if hi > lo {
        let range = (hi - lo) as f32;
        for p in buf.pixels_mut() { p[0] = (((p[0] - lo) as f32 / range) * 255.0).round() as u8; }
    }
}
```

**Tác động.** Chỉ cần **một** pixel giá trị 0 (bụi, lỗ ghim, viền đen mép scan,
vệt tối từ nắp máy) và **một** pixel giá trị 255 là `lo=0, hi=255` → `range=255`
→ phép biến đổi thành hàm đồng nhất. Scan thật gần như luôn có cả hai cực, nên
bước "kéo giãn tương phản" được quảng cáo trong doc comment (`:5`, `:350`) thực
tế không làm gì. Đây là cạm bẫy kinh điển của min/max contrast stretch.

Hệ quả dây chuyền: B5 (`detect_column_ranges`) so ngưỡng mực trên ảnh *đã qua*
`normalize`, nên ngưỡng đó cũng chạy trên grayscale thô.

**Cách xử lý.** Clip theo percentile thay vì min/max — bỏ qua outlier hai đầu.

```rust
/// Kéo giãn histogram grayscale, clip 2% mỗi đầu.
///
/// Min/max toàn cục vô dụng trên scan thật: một pixel bụi (0) và một pixel giấy
/// trắng (255) làm phép kéo giãn thành hàm đồng nhất.
fn normalize(buf: &mut GrayImage) {
    const CLIP: f32 = 0.02;
    let mut hist = [0u32; 256];
    for p in buf.pixels() {
        hist[p[0] as usize] += 1;
    }
    let total: u32 = buf.len() as u32;
    if total == 0 {
        return;
    }
    let lo = percentile(&hist, total, CLIP);
    let hi = percentile(&hist, total, 1.0 - CLIP);
    if hi <= lo {
        return;
    }
    let (lo_f, range) = (lo as f32, (hi - lo) as f32);
    for p in buf.pixels_mut() {
        p[0] = (((p[0] as f32 - lo_f) / range).clamp(0.0, 1.0) * 255.0).round() as u8;
    }
}

fn percentile(hist: &[u32; 256], total: u32, fraction: f32) -> u8 {
    let target = (total as f32 * fraction) as u32;
    let mut seen = 0u32;
    for (value, count) in hist.iter().enumerate() {
        seen += count;
        if seen >= target {
            return value as u8;
        }
    }
    255
}
```

**Kiểm chứng.**
1. Unit test bắt đúng lỗi hiện tại:
   ```rust
   #[test]
   fn normalize_ignores_single_pixel_outliers() {
       // Nền xám 100..=150 cộng 1 pixel đen + 1 pixel trắng: min/max cũ ra no-op.
       let mut img = GrayImage::from_pixel(100, 100, Luma([120]));
       img.put_pixel(0, 0, Luma([0]));
       img.put_pixel(99, 99, Luma([255]));
       for x in 1..99 { img.put_pixel(x, 50, Luma([150])); }
       normalize(&mut img);
       // Sau khi clip outlier, dải 120..150 phải được kéo giãn thật.
       assert!(img.get_pixel(50, 50)[0] < 120, "nền phải tối đi (đã kéo giãn)");
       assert!(img.get_pixel(50, 50)[0] != 120, "không được là no-op");
   }
   ```
2. Đo lại CER trên corpus scan (cần B4). `[Inference]` cải thiện có nhưng chưa định lượng.

**Ảnh hưởng gate.** Như B1: đổi output OCR → version mới khi reconvert; không
đụng index signature.

---

## B3 — `heading_token_hits` khớp substring và không chuẩn hoá

- **Mức độ:** High · **Effort:** S · **Trạng thái:** Đã xác minh
- **Bằng chứng:** `crates/knowledge/src/rank.rs:47-62`

```rust
pub fn heading_token_hits(query_tokens: &[String], heading: &str) -> f32 {
    let normalized = fileconv_core::intelligence::normalize_search_text(heading);
    query_tokens.iter().filter(|token| normalized.contains(*token)).count() as f32
}                                          // ^^^^^^^^ (a) substring, không phải token
                                           // (b) thiếu phép chia

pub fn body_token_overlap(query_tokens: &[String], body: &str) -> f32 {
    let body_tokens: HashSet<String> = normalized_tokens(body).into_iter().collect();
    query_tokens.iter().filter(|token| body_tokens.contains(*token)).count() as f32
        / query_tokens.len().max(1) as f32          // ← có chia
}
```

**Tác động (a) — bắn nhầm substring.** `query.rs:3` đặt
`MIN_QUERY_TOKEN_CHARS = 2`, và sau accent-fold
(`intelligence.rs:488-490` = NFD-strip + `đ→d` + lowercase) token 2 ký tự rất
phổ biến trong tiếng Việt. Ví dụ: token `"an"` khớp heading *"ban hành"* (nằm
trong cả `ban` và `hanh`); `"co"` khớp *"công"*; `"ba"` khớp *"bảng"*. Boost
heading bắn nhầm thường xuyên. `body_token_overlap` ngay bên cạnh dùng `HashSet`
đúng — hai hàm không nhất quán.

**Tác động (b) — mất cân bằng trọng số.** Trong công thức tổng
(`rank.rs:64-76`, hằng số ở `:7-11`):

| Số hạng | Trần thực tế |
|---|---|
| `reciprocal_rank_fusion(..) * RRF_RERANK_SCALE` | `(2/60) × 30 = 1.00` |
| `vector_score.max(0.0) * VECTOR_WEIGHT` | `1.0 × 0.55 = 0.55` |
| `body_token_overlap(..) * BODY_OVERLAP_WEIGHT` | `1.0 × 0.35 = 0.35` |
| `heading_token_hits(..) * HEADING_HIT_WEIGHT` | **không có trần** |

`heading_token_hits` trả **đếm thô**, nên query 10 token khớp hết heading cho
`10 × 0.1 = 1.00` — bằng cả số hạng RRF và gấp gần đôi số hạng vector. Với query
dài (rất phổ biến ở Q&A tiếng Việt), heading có thể áp đảo cả dense lẫn lexical
rank. `[Inference]` Đây gần như chắc chắn không phải chủ ý, vì hàm liền kề có
chia còn hàm này thì không.

**Cách xử lý.**

```rust
/// Tỉ lệ token query khớp heading, theo TOKEN (không phải substring).
///
/// Trả về [0,1] để cùng thang với `body_token_overlap`: `heading_token_hits`
/// trước đây trả đếm thô nên query dài làm số hạng heading vượt cả số hạng RRF.
pub fn heading_token_hits(query_tokens: &[String], heading: &str) -> f32 {
    let heading_tokens: HashSet<String> = normalized_tokens(heading).into_iter().collect();
    query_tokens
        .iter()
        .filter(|token| heading_tokens.contains(*token))
        .count() as f32
        / query_tokens.len().max(1) as f32
}
```

Lưu ý: đổi thang từ "đếm" sang "tỉ lệ" làm giảm ảnh hưởng của heading. Nếu số đo
cho thấy heading nên mạnh hơn, **điều chỉnh `HEADING_HIT_WEIGHT`** (một hằng số
có tên, đo được) chứ không quay lại đếm thô không trần.

**Kiểm chứng.**
1. Test bắt lỗi substring:
   ```rust
   #[test]
   fn heading_hits_require_token_match_not_substring() {
       let tokens = vec!["an".to_string()];
       // "ban hành" → tokens {ban, hanh}: không có token "an".
       assert_eq!(heading_token_hits(&tokens, "Ban hành"), 0.0);
       assert_eq!(heading_token_hits(&tokens, "An toàn"), 1.0);
   }

   #[test]
   fn heading_hits_are_bounded_to_one() {
       let tokens: Vec<String> = (0..10).map(|i| format!("tok{i}")).collect();
       let heading = tokens.join(" ");
       assert!(heading_token_hits(&tokens, &heading) <= 1.0);
   }
   ```
2. `rank.rs:144-157` (`rrf_and_rerank_match_frozen_golden_score`) **sẽ đỏ** — giá
   trị golden 1.875 được tính với công thức cũ. Phải tính lại và ghi rõ trong
   commit message rằng golden đổi có chủ ý.
3. Chạy lại gate `G0-RET-RECALL-AT-5` trên corpus vàng (268 query, graded
   judgment). Đây là điều kiện bắt buộc — xem bên dưới.

**Ảnh hưởng gate.** Đây là finding có ảnh hưởng gate lớn nhất trong nhóm B:
- Đổi ranking ⇒ `rankingSha256` trong evidence retrieval đổi ⇒ phải chạy lại
  harness và cập nhật baseline, **không** được sửa tay file kết quả.
- Recall@5 hiện là **0.9261** (ADR 0005, min của 3 lần load), gate là 0.85.
  Nếu sau khi sửa mà Recall@5 tụt dưới 0.85 thì fix này phải đi kèm hiệu chỉnh
  `HEADING_HIT_WEIGHT`, chứ không revert.
- **Không** đụng index signature (ranking là query-time, không phải index-time).

---

## B4 — Bằng chứng OCR quá mỏng; chưa đo end-to-end qua OCR

- **Mức độ:** High (bằng chứng) · **Effort:** L · **Trạng thái:** Đã xác minh
- **Bằng chứng:**
  - `bench/REPORT_ACCURACY.md:19-28` — bảng "Trung bình theo kịch bản":
    `image-print-OCR` **n=1**, `image-lowres-OCR` **n=1**, `handwrite-OCR` **n=2**.
    Ground truth mỗi mẫu 399 ký tự (`:7-15`).
  - `bench/markhand_web/scripts/generate_corpus.py:45-56` — `FORMATS` gồm
    `pdf_scan × 2` và `image_ocr × 3` trên tổng 27 document (≈19% chạm OCR).
  - `:579-605, 637-650` — cả hai loại đều do `PIL.ImageDraw.text` render bằng
    DejaVu trên nền trắng. Grep `rotate|skew|noise|jpeg` trong generator → chỉ có
    import PIL, **không** có phép biến đổi nào.
  - `docs/project-roadmap.md:64` — dự án đã tự ghi nhận: *"sample hiện là
    font-render, không phải viết tay thật"*.
  - `:80` — backlog đã có mục *"Benchmark tài liệu hành chính chuyên ngành"*.

**Tác động.** Độ nghiêm ngặt của bằng chứng rất lệch giữa hai mảng:

| Mảng | Nền đánh giá |
|---|---|
| Embedding / retrieval | 31 version, **268 query** graded judgment + UTF-8 span, min-of-3-load, `rankingSha256`, comparator bị loại có số liệu (BKAI 0.7962, OpenAI ada-002 0.7752), generator pin theo font fingerprint |
| **OCR** | **n=1** mỗi kịch bản, toàn bộ font-render tổng hợp, không nghiêng/nhiễu/artifact/con dấu |

Hệ quả cụ thể: **Recall@5 = 0.9261 là chất lượng retrieval trên text sạch.**
Chất lượng end-to-end trên đường thật (scan → OCR có lỗi → chunk → embed →
retrieve) chưa được đo lần nào. Với thị trường mục tiêu là văn bản hành chính
Việt Nam — phần lớn là scan — đây là khoảng trống bằng chứng lớn nhất của dự án,
và nó **chặn việc định lượng B1, B2, B5, B9, B10**.

**Cách xử lý.** Ba việc, làm theo thứ tự:

1. **Corpus scan thật có nhãn.** 30–50 trang tài liệu hành chính (công văn,
   quyết định, biểu mẫu) với ground-truth gõ tay. Cần phủ: nghiêng 1–3°, nhiễu
   JPEG, con dấu đỏ đè chữ, nền ố/vàng, IN HOA có dấu, bảng nhiều cột, 150–300
   DPI. Lưu ý license/PII — nếu không public được thì để ngoài repo và pin bằng
   checksum như `bench/markhand_web/manifest.lock.json` đang làm.

2. **Fixture tổng hợp lấp đúng nhánh code hiện bị bỏ.** Rẻ và làm được ngay,
   không đợi (1):
   ```python
   # bench/markhand_web/scripts/generate_corpus.py
   # A4 @300 DPI = 2481×3508: vượt MAX_LONG_SIDE nên chạm nhánh downscale mà
   # fixture 1800×2400 hiện tại không bao giờ chạm (xem B1).
   SCAN_SIZES = {"scan_a4_300dpi": (2481, 3508), "scan_legacy": (1800, 2400)}
   ```
   Thêm biến thể có `image.rotate(2.0, fillcolor="white")` và lưu JPEG quality 70
   để có nhiễu nén.

3. **Metric end-to-end.** Harness mới: `scan → convert_path → prepare_chunks →
   embed → retrieve`, báo Recall@5 **trên corpus scan**, đặt cạnh Recall@5 trên
   text sạch. Chênh lệch giữa hai số chính là "thuế OCR" của hệ thống — con số
   quan trọng nhất mà dự án hiện chưa có.

```make
# Makefile
check-ocr-accuracy:
	./target/release/fileconv accuracy bench/scan_real/manifest.tsv \
	  bench/REPORT_SCAN_REAL.md
	python3 bench/markhand_web/scripts/retrieval_e2e.py --corpus scan_real
```

**Kiểm chứng.** Có `bench/REPORT_SCAN_REAL.md` với CER/WER theo từng nhóm khó, và
một dòng Recall@5-qua-OCR đặt cạnh 0.9261. Sau đó B1/B2/B5/B9/B10 mới có cơ sở
báo cáo mức cải thiện bằng số thay vì `[Inference]`.

**Ảnh hưởng gate.** Đổi corpus ⇒ `manifest.lock.json` phải regenerate
(`python3 scripts/validate_corpus.py --reproducible`), `make check-corpus` phải
xanh. Thêm gate mới thì cân nhắc để non-blocking ở lần đầu để biết baseline thật.

---

## B5 — `detect_column_ranges` dùng ngưỡng mực cứng 205

- **Mức độ:** Medium · **Effort:** M · **Trạng thái:** Đã xác minh · **Phụ thuộc:** B2
- **Bằng chứng:** `crates/core/src/image_ocr.rs:383`

```rust
projection[x as usize] = (y_start..y_end)
    .filter(|y| image.get_pixel(x, *y)[0] < 205)     // ngưỡng cứng
    .count() as u32;
```

Hàm nhận ảnh **đã qua** `preprocess` (gọi từ `:451` với `&pre.to_luma8()` ở `:303`),
tức đã qua `normalize` — nhưng theo B2 thì `normalize` thường là no-op, nên ngưỡng
205 thực tế chạy trên grayscale thô.

**Tác động.** Tách cột brittle đúng lúc cần nhất:
- Scan sáng (nền ~235): gần như không pixel nào `< 205` → `projection` ≈ 0 khắp
  nơi → mọi cột đều bị coi là gutter → `ranges.len()` không nằm trong `2..=3`
  (`:427`) → fallback một cột.
- Scan xám/ố (nền ~190): mọi pixel `< 205` → không có gutter nào (`:390`) → cũng
  fallback một cột.

Nghĩa là tách cột chỉ hoạt động trong dải nền hẹp quanh giá trị mà ngưỡng 205 giả
định. "Bảng PDF nhiều cột" đang là điểm yếu #2 mà roadmap ghi nhận.

**Cách xử lý.** Sửa B2 trước (khi `normalize` hoạt động thật, dải giá trị được
kéo về [0,255] nên 205 hợp lý hơn), rồi thay ngưỡng cứng bằng ngưỡng suy từ chính
ảnh:

```rust
/// Ngưỡng mực suy từ modal background của ảnh, thay cho hằng số cứng.
///
/// Ngưỡng cố định 205 chỉ đúng với một dải nền hẹp: scan sáng thì không pixel nào
/// dưới ngưỡng, scan ố thì mọi pixel đều dưới ngưỡng — cả hai đều làm tách cột
/// fallback về một cột.
fn ink_threshold(image: &GrayImage) -> u8 {
    let mut hist = [0u32; 256];
    for p in image.pixels() {
        hist[p[0] as usize] += 1;
    }
    // Nền là mode ở nửa sáng; mực là mọi thứ tối hơn nền một khoảng an toàn.
    let background = (128..256)
        .max_by_key(|value| hist[*value])
        .unwrap_or(255) as u8;
    background.saturating_sub(45)
}
```

Rồi dùng `let threshold = ink_threshold(image);` ở `:380` và so `< threshold`.
Phương án chặt hơn là Otsu trên toàn histogram; `ink_threshold` ở trên là bước
trung gian rẻ, không thêm dependency.

**Kiểm chứng.**
1. Test tổng hợp 3 mức nền — sáng (235), trung bình (200), ố (185) — cùng một bố
   cục 2 cột, đều phải phát hiện đúng 2 cột. Test hiện có
   `detects_two_content_columns_but_not_one_wide_block` (`:818`) phải vẫn xanh.
2. CER trên nhóm "bảng nhiều cột" của corpus B4.

**Ảnh hưởng gate.** Như B1/B2.

---

## B6 — Bảng Markdown bị cắt cứng, chunk sau mất header row

- **Mức độ:** Medium · **Effort:** M · **Trạng thái:** Đã xác minh
- **Bằng chứng:** `crates/core/src/chunk.rs:161-181`

```rust
for para in text.split("\n\n") {          // ← dòng bảng cách nhau bằng \n ĐƠN
    ...
    if plen > max_chars {
        let mut it = para.chars().peekable();
        while it.peek().is_some() {
            let piece: String = it.by_ref().take(max_chars).collect();   // cắt giữa dòng/ô
            push_chunk(&mut chunks, &heading, piece.trim());
        }
    }
```

`CHUNK_MAX_CHARS = 2000` (`crates/server/src/services/chunking.rs:11`).

**Tác động.** Các dòng của một bảng Markdown cách nhau bằng `\n` đơn, nên
`split("\n\n")` coi **cả bảng là một "đoạn" duy nhất**. Bảng > 2000 ký tự rơi vào
nhánh cắt cứng: chunk thứ 2 trở đi là các dòng **không có header row và không có
dòng separator**, tức mất hết tên cột. Khi retrieve, những chunk đó gần như vô
nghĩa — số liệu không còn biết thuộc cột nào.

Comment ở `:168` ghi *"hiếm: bảng khổng lồ"*, nhưng bảng lớn là **output đặc trưng**
của chính bộ convert này: `conv/xlsx.rs` đọc **mọi** sheet (`docs/system-architecture.md:86`),
`conv/pdf` sinh bảng có cấu trúc, và `conv/docx.rs`/`conv/xlsx.rs` còn xuất HTML
table cho merge cell.

**Cách xử lý.** Nhận diện khối bảng và cắt theo dòng, lặp lại header ở mỗi mảnh.

```rust
/// Nhận diện khối bảng Markdown/HTML trong một "đoạn".
fn table_header(para: &str) -> Option<(&str, &str)> {
    let mut lines = para.lines();
    let header = lines.next()?;
    let separator = lines.next()?;
    let is_md_table = header.trim_start().starts_with('|')
        && separator.trim_start().starts_with('|')
        && separator.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '));
    if is_md_table {
        Some((header, separator))
    } else {
        None
    }
}

/// Cắt bảng theo DÒNG và lặp header ở mỗi mảnh.
///
/// Cắt cứng theo ký tự làm chunk thứ 2 trở đi mất tên cột, nên số liệu trong đó
/// không còn truy được về cột nào.
fn push_table_chunks(chunks: &mut Vec<Chunk>, heading: &str, para: &str, max_chars: usize) {
    let Some((header, separator)) = table_header(para) else {
        push_hard_split(chunks, heading, para, max_chars);
        return;
    };
    let prefix_len = header.chars().count() + separator.chars().count() + 2;
    let mut current = String::new();
    for row in para.lines().skip(2) {
        let row_len = row.chars().count() + 1;
        if !current.is_empty() && prefix_len + current.chars().count() + row_len > max_chars {
            chunks_push_table(chunks, heading, header, separator, &current);
            current.clear();
        }
        current.push_str(row);
        current.push('\n');
    }
    if !current.trim().is_empty() {
        chunks_push_table(chunks, heading, header, separator, &current);
    }
}
```

Cân nhắc thêm: dòng bảng đơn lẻ dài hơn `max_chars` vẫn phải cắt cứng — giữ
`push_hard_split` làm đường cuối, nhưng ghi log/warning để biết tần suất.

**Kiểm chứng.**
```rust
#[test]
fn large_table_repeats_header_in_every_chunk() {
    let mut md = String::from("# Báo cáo\n\n| Mã | Tên | Số tiền |\n|---|---|---|\n");
    for i in 0..400 {
        md.push_str(&format!("| M{i} | Khoản mục {i} | {} |\n", i * 1000));
    }
    let chunks = chunk_markdown(&md, 2000);
    assert!(chunks.len() > 1, "bảng lớn phải chia nhiều chunk");
    for chunk in &chunks {
        assert!(chunk.text.contains("| Mã | Tên | Số tiền |"), "mỗi chunk phải có header");
        assert!(chunk.chars <= 2000);
    }
}
```

**Ảnh hưởng gate — quan trọng.** Đây là finding **đụng index signature**:
- Đổi thuật toán chunking ⇒ `chunk_identity` (`knowledge/identity.rs`, dùng ở
  `chunking.rs:54-61`) đổi cho **cùng một Markdown** ⇒ pin
  `heading-chunks-2000-v1` (ADR 0005) không còn đúng.
- Phải bump lên `heading-chunks-2000-v2` và đi theo đường migration ADR 0006 /
  ADR 0011 (expand/cutover/contract), tức **rebuild vector index**.
- Vì vậy nên gom B6 **cùng lô** với bất kỳ thay đổi chunking nào khác (B14) để chỉ
  rebuild index một lần.
- Chạy lại `G0-RET-RECALL-AT-5` và `generate_expected_chunks.py`.

---

## B7 — Lexical desktop `OR` vs server `AND`

- **Mức độ:** Medium · **Effort:** M · **Trạng thái:** Đã xác minh
- **Bằng chứng:**

| | Desktop (SQLite FTS5) | Server (PostgreSQL) |
|---|---|---|
| Query | `"tok1"* OR "tok2"* OR …` — `crates/knowledge/src/query.rs:44-48` | `plainto_tsquery('simple', $N)` → **AND** — `crates/server/src/db/search.rs:224,242,257` |
| Gọi từ | `crates/knowledge/src/desktop/service.rs:320,861,938` | `fts_search` (`db/search.rs:203`) |
| Hành vi | recall cao, precision thấp | precision cao, recall thấp |

**Tác động.** Cùng một câu hỏi cho ra hành vi lexical khác hẳn giữa desktop và
web, trong khi invariant của dự án là hai sản phẩm dùng chung `crates/knowledge`.
Logic *rank* (`rank.rs`) đúng là dùng chung; nhưng *query semantics* rẽ nhánh ở
tầng FTS mà không có tài liệu nào nêu sự khác biệt này. Kết quả là gate retrieval
đo trên đường server không nói gì về chất lượng đường desktop, và ngược lại.

Ghi chú: nhánh OR + prefix wildcard trên token đã accent-fold có precision đặc
biệt thấp với tiếng Việt — `"doi"*` khớp *đối/đội/đôi/dời/dõi/dối/doi* và mọi từ
bắt đầu bằng chuỗi đó.

**Cách xử lý.** Thống nhất về một chiến lược, và đặt nó trong `crates/knowledge`
để cả hai phía dùng: AND trước, fallback OR khi thiếu kết quả.

```rust
// crates/knowledge/src/query.rs
impl PreparedQuery {
    /// FTS5: AND tất cả token (precision, khớp semantics `plainto_tsquery` của server).
    pub fn fts5_all(&self) -> String {
        self.tokens.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(" AND ")
    }

    /// FTS5: OR + prefix (recall). Chỉ dùng khi `fts5_all` không đủ kết quả.
    pub fn fts5_any_prefix(&self) -> String {
        self.tokens.iter().map(|t| format!("\"{t}\"*")).collect::<Vec<_>>().join(" OR ")
    }
}
```

```rust
// desktop/service.rs — hai tầng, giữ đúng ngưỡng limit hiện tại
let mut lexical = store.lexical_ranks(&prepared.fts5_all(), &scope, 250)?;
if lexical.len() < MIN_LEXICAL_CANDIDATES {
    lexical = store.lexical_ranks(&prepared.fts5_any_prefix(), &scope, 250)?;
}
```

Nếu chọn hướng khác (giữ OR ở desktop vì UX tìm-khi-gõ), thì phải ghi rõ khác
biệt vào `docs/adr/` và vào doc comment của `PreparedQuery`, để người đọc không
tưởng hai phía đồng nhất.

**Kiểm chứng.** Test parity: cùng bộ document + cùng query → so top-5 giữa
đường desktop và server, assert độ trùng ≥ ngưỡng đã chốt. Đây là loại test hiện
chưa tồn tại và sẽ bắt được mọi lần rẽ nhánh về sau.

**Ảnh hưởng gate.** Đổi lexical candidate set ⇒ ranking đổi ⇒ chạy lại
`G0-RET-RECALL-AT-5` và `rankingSha256`, giống B3. **Không** đụng index signature.
Nên làm **sau** B3 để chỉ phải rebaseline ranking một lần.

---

## B8 — Chưa có chỗ đặt prefix `query:`/`passage:` cho họ E5

- **Mức độ:** Medium (rủi ro tương lai) · **Effort:** S · **Trạng thái:** Đã xác minh
- **Bằng chứng:**
  - `docs/adr/0005-vietnamese-embedding-model-quality.md` mục 5 (Target runtime):
    self-host vLLM với `BAAI/bge-m3` **và** *"ít nhất một model họ
    multilingual-e5"*, `runtime_path=vllm-local`.
  - `crates/server/src/services/embedding.rs:128-130` — `canonical_input` hard-code
    `format!("{heading}\n{body}")` cho **cả hai** phía (index và query).
  - `crates/knowledge/src/query.rs:34-54` — `PreparedQuery` không có khái niệm prefix.
  - `crates/knowledge/src/embedding.rs:122-186` — `EmbeddingPlan` mang
    provider/model/dimensions/normalized, **không** mang prefix.

**Tác động.** Model hiện tại (`AITeamVN/Vietnamese_Embedding`) dùng encoding đối
xứng nên không cần prefix — hiện đúng. Nhưng họ **E5 bắt buộc** tiền tố bất đối
xứng `"query: "` cho truy vấn và `"passage: "` cho đoạn văn. Thiếu prefix **không
báo lỗi**, chỉ làm chất lượng tụt âm thầm. Và vì prefix là một phần của cách sinh
vector, phát hiện muộn nghĩa là **rebuild lại toàn bộ index** lần nữa.

**Cách xử lý.** Đưa prefix vào `EmbeddingPlan` **ngay bây giờ**, và tính nó vào
index signature để hai generation không bao giờ trộn lẫn:

```rust
// crates/knowledge/src/embedding.rs
pub struct EmbeddingPlan {
    // ... các field hiện có
    /// Tiền tố bất đối xứng. Rỗng cho model đối xứng (bge-m3, AITeamVN);
    /// "query: " / "passage: " cho họ multilingual-e5. Sai prefix KHÔNG báo lỗi,
    /// chỉ tụt chất lượng — nên phải nằm trong index signature.
    pub query_prefix: &'static str,
    pub passage_prefix: &'static str,
}

impl EmbeddingPlan {
    pub fn embed_passage_input(&self, heading: &str, body: &str) -> String {
        format!("{}{heading}\n{body}", self.passage_prefix)
    }

    pub fn embed_query_input(&self, query: &str) -> String {
        format!("{}{query}", self.query_prefix)
    }
}
```

Rồi thêm hai prefix vào `index_signature_unchecked` (`embedding.rs:240-246`) cạnh
`dimensions` và `normalized`, và cho `services/embedding.rs::canonical_input`
uỷ quyền sang `embed_passage_input`.

**Kiểm chứng.**
1. Test: hai plan chỉ khác prefix ⇒ `index_signature` khác nhau.
2. Test: plan hiện tại (prefix rỗng) sinh **đúng signature như trước** — đây là
   điều kiện để thay đổi này không phải là migration.
3. `cargo test -p fileconv-knowledge --all-features` và
   `bash scripts/check-knowledge-features.sh` xanh.

**Ảnh hưởng gate.** Nếu prefix rỗng giữ nguyên signature cũ thì **không** cần
rebuild index và không đụng gate. Đây chính là lý do nên làm sớm: cùng một thay
đổi, làm bây giờ thì miễn phí, làm sau khi có index sản xuất thì phải rebuild.

---

## B9 — Thiếu bước deskew

- **Mức độ:** Low · **Effort:** M · **Trạng thái:** Đã xác minh · **Phụ thuộc:** B4
- **Bằng chứng:** `crates/core/src/image_ocr.rs:327-354` — `preprocess` gồm đúng
  4 bước: `to_luma8` → resize → `unsharpen` → `normalize`. Không có phép quay.
  Doc comment `:4-5` cũng liệt kê đúng 4 bước đó.

**Tác động.** Tài liệu scan flatbed hoặc chụp điện thoại lệch 1–3° là bình thường,
và độ nghiêng làm hỏng bước phân dòng của Tesseract. `[Inference]` Đây thường là
nguyên nhân của chính điểm yếu #1 mà roadmap ghi nhận (*"IN HOA dính chữ"*): khi
dòng nghiêng, ký tự của hai dòng chồng dải chiếu ngang nên bị gộp. Cần B4 để xác
nhận trên dữ liệu của dự án.

**Cách xử lý.** Ước lượng góc bằng phép chiếu ngang (rẻ, không thêm dependency),
chỉ quay khi góc đủ lớn:

```rust
/// Ước lượng góc nghiêng bằng cách tối đa hoá phương sai của horizontal projection.
///
/// Dòng chữ thẳng cho projection nhiều đỉnh/đáy rõ (phương sai cao); dòng nghiêng
/// làm nhoè các đỉnh đó.
fn estimate_skew_degrees(image: &GrayImage, threshold: u8) -> f32 {
    let mut best = (0.0f32, f32::MIN);
    let mut angle = -3.0f32;
    while angle <= 3.0 {
        let score = projection_variance(image, angle, threshold);
        if score > best.1 {
            best = (angle, score);
        }
        angle += 0.25;
    }
    best.0
}
```

rồi trong `preprocess`, sau `normalize`:

```rust
let skew = estimate_skew_degrees(&sharp, ink_threshold(&sharp));
// Dưới 0.3° thì quay chỉ thêm nhoè do nội suy, không lợi.
let sharp = if skew.abs() >= 0.3 { rotate_bilinear(&sharp, -skew) } else { sharp };
```

Đặt **sau** B2/B5 vì `estimate_skew_degrees` cần ngưỡng mực tin cậy.

**Kiểm chứng.**
1. Test: ảnh text tổng hợp quay 2° → `estimate_skew_degrees` trả trong khoảng
   `[1.5, 2.5]`; ảnh không nghiêng trả `|angle| < 0.3`.
2. CER trên nhóm "nghiêng" của corpus B4, trước/sau.
3. `fileconv speed` — deskew thêm một lần quét ảnh, phải đo chi phí.

**Ảnh hưởng gate.** Như B1/B2.

---

## B10 — `ocr_text_score` là heuristic thưởng độ dài; bỏ không dùng confidence Tesseract

- **Mức độ:** Low · **Effort:** M · **Trạng thái:** Đã xác minh · **Phụ thuộc:** B4
- **Bằng chứng:** `crates/core/src/image_ocr.rs:773-792`

```rust
fn ocr_text_score(text: &str) -> i64 {
    letters * 3 + words * 4 - replacements * 30 - glued_penalty * 2
}
```

Dùng để chọn giữa PSM 4 / PSM 6 (`:461-470`), giữa split-cột và toàn trang
(`:480`), và giữa Paddle và Tesseract (`:313`). `build_tesseract_psm_command`
(`:629`) không truyền `tessedit_create_tsv`, nên confidence per-word của Tesseract
chưa bao giờ được đọc.

**Tác động.** Hai vấn đề:
1. **Thưởng độ dài.** `words * 4` nghĩa là output ảo giác nhiều từ rác ghi điểm
   *cao hơn* transcript đúng nhưng thưa. Không có tín hiệu ngôn ngữ nào để phân
   biệt "nhiều từ" với "nhiều từ đúng".
2. **`glued_penalty` quá lỏng.** Chỉ tính token có **> 18** ký tự chữ (`:788`).
   Từ tiếng Việt dài 1–2 âm tiết (≤ 7 ký tự), nên `"thanhtoánchokháchhàng"` (21)
   bị bắt còn `"thanhtoán"` (9) thì không. Với điểm yếu #1 là dính chữ IN HOA,
   ngưỡng 18 bỏ sót phần lớn ca thực tế.

**Cách xử lý.** Thay heuristic bằng confidence có sẵn, miễn phí:

```rust
// build_tesseract_psm_command — thêm output TSV
command.arg("-c").arg("tessedit_create_tsv=1");

/// Mean word-confidence từ TSV của Tesseract (cột 11; -1 là dòng/khối, bỏ qua).
fn mean_word_conf(tsv: &str) -> f32 {
    let confs: Vec<f32> = tsv
        .lines()
        .skip(1)
        .filter_map(|line| line.split('\t').nth(10)?.parse::<f32>().ok())
        .filter(|conf| *conf >= 0.0)
        .collect();
    if confs.is_empty() {
        0.0
    } else {
        confs.iter().sum::<f32>() / confs.len() as f32
    }
}
```

Kèm một tín hiệu tiếng Việt rẻ để bổ trợ (và để dùng được cả với Paddle, vốn
không có TSV): tỉ lệ âm tiết hợp lệ theo bảng âm tiết tiếng Việt. Hạ ngưỡng
`glued_penalty` xuống mức thực tế (≈ 12 ký tự chữ, tương đương ~3 âm tiết) và
kiểm tra bằng corpus B4 trước khi chốt con số.

**Kiểm chứng.**
1. Test: transcript đúng-nhưng-thưa phải ghi điểm **cao hơn** transcript rác-nhưng-dài.
   Test hiện có `quality_score_prefers_separated_clean_text` (`:811`) phải vẫn xanh.
2. Test: `"thanhtoanchokhachhang"` bị phạt.
3. Đo trên corpus B4 — đây là finding **không nên** sửa trước B4, vì không có cách
   biết heuristic mới có tốt hơn thật không.

**Ảnh hưởng gate.** Đổi lựa chọn PSM ⇒ đổi output OCR ⇒ như B1/B2.

---

## B11 — Trang 2 cột spawn Tesseract 3–5 lần

- **Mức độ:** Low · **Effort:** S · **Trạng thái:** Đã xác minh
- **Bằng chứng:** `crates/core/src/image_ocr.rs:444-485`

```rust
let whole = run_tesseract_detailed(whole_path, langs, config)?;   // :450 — VÔ ĐIỀU KIỆN
let ranges = detect_column_ranges(image);                         // :451
if ranges.len() <= 1 { return Ok(whole); }                        // :452-454
for (left, right) in ranges {                                     // :456 — thêm 1 lần/cột
    let automatic = run_tesseract_psm_detailed(path, langs, 4, config);   // :461
    Ok(value) if should_retry_layout(&value) => {
        let block = run_tesseract_psm_detailed(path, langs, 6, config)    // :464 — +1 retry
```

**Tác động.** Trang 2 cột = 1 (toàn trang) + 2 (mỗi cột) + tối đa 2 (retry PSM 6)
= **tới 5 lần spawn Tesseract CLI**. Bản `whole` chỉ dùng làm mốc so sánh ở `:480`.
`[Inference]` Mỗi lần spawn trên trang A4 300 DPI tốn khoảng vài trăm ms tới vài
giây; con số cụ thể cần `fileconv speed` để xác nhận. Lưu ý finding này tương tác
với B1: sau khi bỏ downscale, mỗi lần spawn xử ảnh lớn hơn nên chi phí nhân lên.

**Cách xử lý.** `detect_column_ranges` chỉ cần ảnh, không cần kết quả OCR — nên
hoãn pass toàn trang tới khi biết có nhiều cột:

```rust
fn run_tesseract_with_columns_detailed(
    image: &GrayImage, whole_path: &Path, langs: &str, config: &OcrRunConfig,
) -> Result<String, OcrAttemptError> {
    // detect_column_ranges chỉ cần ảnh: đừng chạy pass toàn trang trước khi biết
    // có nhiều cột hay không (trang 1 cột là ca phổ biến nhất).
    let ranges = detect_column_ranges(image);
    if ranges.len() <= 1 {
        return run_tesseract_detailed(whole_path, langs, config);
    }
    let whole = run_tesseract_detailed(whole_path, langs, config)?;   // vẫn cần làm mốc
    // ... phần còn lại không đổi
}
```

Tiết kiệm được 0 lần spawn cho trang nhiều cột (vẫn cần `whole` làm mốc) nhưng
**không đổi gì** cho trang 1 cột — nên lợi ích thực tế là làm rõ luồng, không phải
tốc độ. Nếu muốn tiết kiệm thật thì phải bỏ so sánh với `whole` và tin vào
`detect_column_ranges`; việc đó cần B5 xong và có số đo từ B4 để chứng minh an toàn.

**Kiểm chứng.** Test đếm số lần spawn: inject binary Tesseract giả (cơ chế đã có
ở `:948 injected_tesseract_binary_opens_ocr_tempfile`) đếm số lần được gọi, assert
1 lần cho trang 1 cột. `fileconv speed` xác nhận không hồi quy.

---

## B12 — `body_token_overlap` chuẩn hoá lại full body mỗi candidate mỗi query

- **Mức độ:** Low · **Effort:** M · **Trạng thái:** Đã xác minh
- **Bằng chứng:** `crates/knowledge/src/rank.rs:55-62` gọi
  `normalized_tokens(body)` → `PreparedQuery::new(body).tokens`
  (`query.rs:61-63`) → `normalize_search_text` (accent-fold toàn bộ) + split +
  `Vec<String>` + `HashSet<String>`. Body tối đa 2 000 ký tự
  (`chunking.rs:11`); candidate limit là 250 (`desktop/service.rs:320,861,938`).

**Tác động.** Tới 250 lần accent-fold + tokenize + cấp phát HashSet trên chuỗi
2 000 ký tự cho **mỗi truy vấn**. `[Inference]` Chưa profile nên chưa biết đây có
phải hot path thật hay không; cần đo trước khi tối ưu.

**Cách xử lý.** Đo trước. Nếu xác nhận là hot path, hai hướng:

1. *(gọn)* Precompute token set lúc index và mang theo candidate — desktop có thể
   lưu vào bảng SQLite cạnh chunk; server đã có `tsv` nên có thể tái dùng.
2. *(rẻ hơn để làm)* Bỏ cấp phát trong vòng lặp: quét `&str` một lượt thay vì
   dựng `Vec<String>` + `HashSet<String>`:

```rust
pub fn body_token_overlap(query_tokens: &[String], body: &str) -> f32 {
    if query_tokens.is_empty() {
        return 0.0;
    }
    let folded = fileconv_core::intelligence::normalize_search_text(body);
    let matched = query_tokens
        .iter()
        .filter(|token| {
            folded
                .split(|c: char| !c.is_alphanumeric())
                .any(|word| word == token.as_str())
        })
        .count();
    matched as f32 / query_tokens.len() as f32
}
```

Lưu ý biến thể này là O(query_tokens × words) thay vì O(words) + hash lookup —
chỉ nhanh hơn khi `query_tokens` nhỏ. **Phải benchmark**, đừng đổi theo cảm giác.

**Kiểm chứng.** `cargo bench` hoặc harness đo latency retrieval trước/sau. Kết quả
ranking phải **giống hệt** (đây là tối ưu thuần, không đổi hành vi) — assert bằng
cách so `rankingSha256` không đổi.

**Ảnh hưởng gate.** Không, **nếu** ranking không đổi. Chính `rankingSha256` là cách
chứng minh điều đó.

---

## B13 — `pre.to_luma8()` clone thừa toàn bộ buffer

- **Mức độ:** Low · **Effort:** S · **Trạng thái:** Đã xác minh
- **Bằng chứng:** `crates/core/src/image_ocr.rs:298-303`

```rust
let pre = preprocess(img);                    // trả DynamicImage::ImageLuma8 (:353)
...
let tesseract = || run_tesseract_with_columns_detailed(&pre.to_luma8(), ...);
//                                                          ^^^^^^^^^ to_ = clone
```

`preprocess` dựng `GrayImage` rồi bọc vào `DynamicImage::ImageLuma8(sharp)`
(`:353`); ở đây lại mở ra bằng `to_luma8()`, mà `to_*` trong crate `image` là
**copy**, không phải borrow.

**Tác động.** Một bản copy toàn buffer mỗi lần OCR. Với trang 1697×2400 hiện tại
là ~4 MB; sau B1 (2480×3508) là ~8.7 MB. Không phải bug, chỉ là rác miễn phí để dọn.

**Cách xử lý.** Cho `preprocess` trả `GrayImage` và bọc vào `DynamicImage` chỉ ở
chỗ thực sự cần (ghi PNG tạm):

```rust
- fn preprocess(img: &DynamicImage) -> DynamicImage {
+ /// Trả GrayImage để caller không phải `to_luma8()` (clone) lại.
+ fn preprocess(img: &DynamicImage, max_long_side: u32) -> GrayImage {
      ...
-     DynamicImage::ImageLuma8(sharp)
+     sharp
  }
```

```rust
- let pre = preprocess(img);
- let tmp = write_ocr_temp_png(&pre)...;
- let tesseract = || run_tesseract_with_columns_detailed(&pre.to_luma8(), tmp_path, langs, config);
+ let pre = preprocess(img, max_long_side);
+ let tmp = write_ocr_temp_gray_png(&pre)...;   // helper đã có sẵn (:256)
+ let tesseract = || run_tesseract_with_columns_detailed(&pre, tmp_path, langs, config);
```

Gộp vào cùng commit với B1 vì cả hai đều sửa signature của `preprocess`.

**Kiểm chứng.** `cargo test -p fileconv-core` xanh. Output OCR phải **không đổi**
(`write_ocr_temp_gray_png` và `write_ocr_temp_png` đều ghi PNG grayscale từ cùng
buffer) — xác nhận bằng test so byte của PNG tạm trước/sau, hoặc so CER trên
fixture hiện có.

---

## B14 — Chunk không có overlap

- **Mức độ:** Low · **Effort:** M · **Trạng thái:** Đã xác minh
- **Bằng chứng:** `crates/core/src/chunk.rs:151-186` — vòng lặp gom paragraph vào
  `cur`, `push_chunk` rồi `cur.clear()` (`:165-166`). Không có phần nào được mang
  sang chunk kế tiếp.

**Tác động.** Một đoạn trả lời câu hỏi mà nằm vắt qua ranh giới chunk sẽ chỉ được
retrieve một nửa. Thực hành phổ biến ở RAG là 10–20% overlap.

Cần đánh giá công bằng: ở đây phần nào đã được bù bằng **heading được prepend vào
embedding input** (`services/embedding.rs:128-130`), nên chunk không mất ngữ cảnh
cha. Với văn bản hành chính có cấu trúc chương/điều rõ — đúng thị trường mục tiêu
— cách này hợp lý và ranh giới chunk thường trùng ranh giới điều khoản. Nên đây là
**đánh đổi có lý**, không phải lỗi rõ ràng, và Recall@5 = 0.9261 cho thấy nó đang
hoạt động tốt trên corpus hiện có.

**Cách xử lý.** Không sửa mù. Đo trước:

1. Phân tích các query miss trong 268 query của corpus vàng: có bao nhiêu miss là
   do câu trả lời vắt ranh chunk? Nếu tỉ lệ thấp thì đóng finding này là
   "đánh đổi đã chấp nhận" và ghi vào doc comment của `chunk_markdown`.
2. Nếu tỉ lệ đáng kể, thêm overlap có kiểm soát:

```rust
/// Số ký tự mang sang từ cuối chunk trước (0 = tắt).
///
/// Overlap giúp ca câu trả lời vắt ranh chunk, nhưng làm phình index và làm
/// citation span chồng nhau — nên phải đo trước khi bật.
const CHUNK_OVERLAP_CHARS: usize = 200;
```

Cảnh báo kèm theo: overlap làm **citation span chồng nhau**, nên phải kiểm tra lại
`locate_chunk_span` (`chunk.rs:70-75`) và `infer_source_anchor` — hai chunk trùng
nội dung sẽ neo vào cùng vùng byte, ảnh hưởng tính năng citation.

**Kiểm chứng.** Recall@5 trước/sau trên corpus vàng; kích thước index trước/sau;
test citation span vẫn đúng và không chồng sai.

**Ảnh hưởng gate.** Như B6 — **đụng index signature**
(`heading-chunks-2000-v1` → v2), phải rebuild vector index. **Gom cùng lô với B6.**

---

# Thứ tự thực hiện đề xuất

Nguyên tắc xếp thứ tự: (1) mở khoá tín hiệu CI trước; (2) sửa các hồi quy im lặng
nhỏ và độc lập; (3) dựng nền bằng chứng trước khi tinh chỉnh những thứ chỉ đo được
mới biết đúng/sai; (4) gom mọi thay đổi đụng index signature vào **một** lô.

## Wave 0 — mở khoá tín hiệu (làm ngay)

| ID | Việc | Lý do đi trước |
|---|---|---|
| A1 | Sửa 3 clippy + bỏ nhánh `--lib` + branch protection | Baseline đỏ làm mọi gate còn lại mất giá trị. Không có việc nào sau đây nên merge lên nền đỏ. |
| A9 | Xoá baseline entry chết + guard path không tồn tại | Cùng file/khu vực với A1, gộp một PR |

## Wave 1 — hồi quy im lặng, nhỏ và độc lập

| ID | Việc | Ghi chú |
|---|---|---|
| B1 + B13 | Trần kích thước riêng cho trang PDF; `preprocess` trả `GrayImage` | Cùng sửa signature `preprocess` → một commit |
| B2 | `normalize()` percentile clip | Điều kiện tiên quyết của B5, B9 |
| B3 | `heading_token_hits` token-match + chuẩn hoá | **Phải** rebaseline `rankingSha256` + chạy `G0-RET-RECALL-AT-5` |
| B8 | `query_prefix`/`passage_prefix` vào `EmbeddingPlan` | Miễn phí bây giờ (prefix rỗng giữ signature cũ); tốn một lần rebuild index nếu để sau |

Cả 4 việc đều nhỏ, không phụ thuộc nhau ngoài cặp B1+B13, và mỗi việc có test bắt
đúng lỗi hiện tại.

## Wave 2 — dựng nền bằng chứng

| ID | Việc | Ghi chú |
|---|---|---|
| B4 | Corpus scan thật + fixture 2481×3508 + metric Recall@5-qua-OCR | Mở khoá việc định lượng B1, B2, B5, B9, B10 |
| A2 | `cargo-deny` + `deny.toml` + Dependabot | Độc lập, chạy song song được |

B4 là việc dài nhất trong report này và là thứ có giá trị lâu dài lớn nhất: nó
biến mọi finding OCR từ `[Inference]` thành đo được.

## Wave 3 — cần số đo mới quyết được

| ID | Việc | Phụ thuộc |
|---|---|---|
| B5 | Ngưỡng mực thích ứng cho tách cột | B2, và B4 để đo |
| B9 | Deskew | B2, B5, và B4 để đo |
| B10 | Confidence Tesseract thay `ocr_text_score`; hạ ngưỡng `glued_penalty` | B4 — không có số đo thì không biết heuristic mới có tốt hơn |
| B12 | Tối ưu `body_token_overlap` (chỉ khi profile xác nhận) | Profile trước |
| B11 | Hoãn pass toàn trang | Sau B5 |

Không nên làm Wave 3 trước Wave 2. Đây đều là những thay đổi mà "trông hợp lý hơn"
không đồng nghĩa với "tốt hơn"; sửa mù thì không có cách nào biết đã cải thiện hay
làm tệ đi.

## Wave 4 — lô đụng index signature (một lần duy nhất)

| ID | Việc |
|---|---|
| B6 | Cắt bảng theo dòng, lặp header row |
| B14 | Overlap (chỉ nếu B4 cho thấy cần) |
| B7 | Thống nhất lexical desktop/server |

Bump `heading-chunks-2000-v1` → `v2`, đi theo đường expand/cutover/contract của
ADR 0006 / ADR 0011, rebuild vector index **một lần** cho cả lô. Chạy lại
`generate_expected_chunks.py`, `G0-RET-RECALL-AT-5`, và `make check-corpus`.

B7 tuy không đụng index signature nhưng đụng ranking, nên gom vào đây để chỉ phải
rebaseline `rankingSha256` một lượt cùng B3 (hoặc đặt B7 ngay sau B3 ở Wave 1 nếu
muốn tách nhỏ hơn).

## Wave 5 — tài liệu và nợ kỹ thuật

| ID | Việc |
|---|---|
| A3 | `CLAUDE.md` + `codebase-summary.md` + `project-roadmap.md` + `AGENTS.md` |
| A4 + A7 | Quota admission sớm; ghi liên kết rate-limit vào risk register |
| A6 | Quyết định control cho `/metrics`, ghi vào convention doc |
| A8 | Fail-closed khi user thuộc nhiều org |
| A5 | Component test desktop (bắt đầu từ `SafeMarkdown`) |
| A10 | Tách `intelligence.rs` |

A3 có thể làm bất cứ lúc nào và nên làm sớm nếu có người mới tham gia — nó không
phụ thuộc gì. Đặt ở Wave 5 vì không chặn việc nào khác.

# Ghi chú về ảnh hưởng gate — tổng hợp

| Loại thay đổi | Finding | Hệ quả |
|---|---|---|
| Không ảnh hưởng | A1, A9, A3, A5, A6, A7, A10, B13 | Chạy gate hiện có là đủ |
| Đổi output OCR → Markdown mới | B1, B2, B5, B9, B10 | Reconvert sinh **version mới** (đúng semantics versioning). **Không** đụng index signature. Cần `make check-corpus`. |
| Đổi ranking (query-time) | B3, B7, B12 | Rebaseline `rankingSha256`, chạy lại `G0-RET-RECALL-AT-5` (gate 0.85, hiện 0.9261). **Không** đụng index signature. |
| **Đổi chunking (index-time)** | **B6, B14** | Bump `heading-chunks-2000-v1` → `v2`, migration ADR 0006/0011, **rebuild vector index**. Gom một lô. |
| Đổi contract HTTP | A4, A8 | Chạy lại `uploads.rs`, `quota.rs`, `auth.rs`, `phase1b_api_contracts.rs` |
| Thêm gate mới | A2, B4 | Cân nhắc non-blocking lần đầu để biết baseline thật |

# Điểm đã xác minh là tốt (đừng sửa khi đi qua)

Ghi lại để các wave trên không vô tình làm hỏng những chỗ đang đúng:

- **`chunk.rs` xử lý CRLF/lone-`\r`** (`:26-31, 49-108`): `normalize_newlines`
  cố ý **không** dùng làm pre-pass trước `lines()` vì `a\r\r\n` sẽ nuốt lone `\r`;
  có test chứng minh (`:242-270`). `locate_chunk_text` khớp `\n`↔`\r\n`, chọn match
  sớm nhất, UTF-8-safe cho tiếng Việt (`:308-315`). Khi sửa B6/B14 **phải** giữ
  toàn bộ tính chất này.
- **Heading có trong embedding input** (`services/embedding.rs:128-130`,
  `workers/embedding.rs:210-211`): `{heading}\n{body}`, dùng chung desktop/server.
  Đây là thứ hay bị làm sai nhất ở RAG và ở đây làm đúng.
- **L2-normalization được cưỡng chế thật**, không chỉ khai báo
  (`knowledge/embedding.rs:299-311`, tolerance 0.001, có test ở `:485-490`) — nên
  `cosine_similarity` viết dạng dot-product trần (`rank.rs:32-37`) là đúng.
- **ADR 0005** là chuẩn mực nên nhân rộng: model pin theo commit hash, Recall@5
  0.9261 lấy min của 3 lần load, kèm bằng chứng loại trừ có số liệu (BKAI 0.7962,
  OpenAI ada-002 0.7752), và chính sách rõ về egress (embedding local, GLM chat-only).
- **ADR 0014** đã ghi nhận chính xác khoảng trống word-segmentation tiếng Việt và
  quyết định hoãn có lý do. Status hiện là `Proposed` — nên chốt thành `Accepted`
  hoặc `Rejected` để không treo.
- **NFC bắt buộc cuối `convert_path`** — đúng một chỗ duy nhất, nên mọi nhánh
  (kể cả Tesseract trả NFD) đều được chuẩn hoá.
- **Chuỗi lỗi OCR có kiểu** (`image_ocr.rs:76-176`): `OcrAttemptError` mang stage +
  io_kind, không dùng TLS/global collector, an toàn với nested call và panic.
- Baseline clippy chỉ **20 warning** toàn workspace, liệt kê theo từng file+lint,
  ratchet chặt (`check-rust-lint-baseline.py:14-29`).

# Nguồn

- Code tại `ae1a1bd` (`HEAD` == `origin/master`); mọi `file:line` trong report đã
  đối chiếu trực tiếp.
- Lệnh đã chạy: xem bảng "Bằng chứng đã chạy" ở đầu report.
- CI: run `30065276505` (master `ae1a1bd`), run `30055166714` (master `60ae6a98`).
- Số đo tham chiếu: `bench/REPORT_ACCURACY.md`, `bench/REPORT_EDGE.md`,
  `docs/adr/0005-vietnamese-embedding-model-quality.md`.
- Review trước: `code-review-260720-1208-markhand-web-phase1b-batch2-report.md`
  (finding #5 quota order vẫn mở → A4; các finding #3, #4, #6, #9, #11, #12 và
  finding bảo mật #2 đã được xác minh là **đã sửa** tại commit này).
