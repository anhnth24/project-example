# Playbook: Backfill dữ liệu an toàn trên bảng có FORCE ROW LEVEL SECURITY

Tài liệu này ghi lại bài học kinh nghiệm và quy chuẩn thực thi khi viết SQL migration để backfill dữ liệu trên các bảng PostgreSQL có kích hoạt `FORCE ROW LEVEL SECURITY` trong hệ thống Markhand.

---

## 1. Bối cảnh & Khái niệm cốt lõi

### 1.1 Row Level Security (RLS) và FORCE ROW LEVEL SECURITY
Trong kiến trúc đa người thuê (multi-tenant) của Markhand, bảo mật dữ liệu giữa các tổ chức (`org_id`) được bảo đảm ở tầng cơ sở dữ liệu bằng PostgreSQL Row Level Security (RLS):
- `ENABLE ROW LEVEL SECURITY`: Bật chính sách kiểm soát truy cập mức dòng trên bảng. Mặc định theo thiết kế của PostgreSQL, **chủ sở hữu bảng (table owner / role migrator) và superuser sẽ bỏ qua (bypass) các policy này**.
- `FORCE ROW LEVEL SECURITY`: Bắt buộc áp dụng RLS đối với **cả chủ sở hữu bảng**. Bảng dữ liệu của Markhand (như `chunks`, `documents`, `document_versions`, `org_memberships`,...) đều kích hoạt `FORCE ROW LEVEL SECURITY` nhằm ngăn ngừa rò rỉ dữ liệu ngoài ý muốn ngay cả khi truy vấn bằng tài khoản chủ bảng mà quên thiết lập ngữ cảnh tổ chức (`app.org_id`).

### 1.2 Vấn đề phát sinh khi chạy Migration Backfill
Khi triển khai một migration cập nhật lại dữ liệu cũ (ví dụ: đánh lại chỉ mục tìm kiếm `tsv`, cập nhật cột tính toán, v.v.):
- Kết nối thực thi migration (`database.rs`) đăng nhập bằng tài khoản migrator (table owner) nhưng **không thiết lập ngữ cảnh của bất kỳ tenant cụ thể nào** (`markhand_current_org_id()` trả về `NULL`).
- Do bảng có `FORCE ROW LEVEL SECURITY`, lệnh `UPDATE` của migrator vẫn bị chính sách RLS lọc hoặc chặn lại hoàn toàn.

---

## 2. Hai cạm bẫy thường gặp khi Backfill

### Cạm bẫy 1: Không làm gì cả — Âm thầm cập nhật 0 dòng (Silent 0-row Trap)
- **Điển hình:** Migration cũ `crates/server/migrations/0016_expand_chunks_accent_fold_tsv.sql`.
- **Đoạn code:**
  ```sql
  -- Backfill existing rows so query-side accent-fold-v1 matches stored vectors.
  UPDATE chunks
  SET body = body;
  ```
- **Hiện tượng:**
  Lệnh `UPDATE` thực thi thành công mỹ mãn, migration chạy xong với mã thoát `0` (không báo lỗi).
- **Thực tế nguy hiểm:**
  Do chính sách RLS lọc theo `org_id = markhand_current_org_id()` mà session migrator không có `org_id`, truy vấn không nhìn thấy bất kỳ bản ghi nào. Kết quả trả về là `UPDATE 0`. Toàn bộ dữ liệu cũ trong cơ sở dữ liệu **hoàn toàn chưa được cập nhật**, nhưng người thực hiện và CI lại lầm tưởng migration đã thành công.

### Cạm bẫy 2: Dùng `SET LOCAL row_security = off` — Gây lỗi sập migration (SQLSTATE 42501)
- **Điển hình:** Phiên bản ban đầu của migration `crates/server/migrations/0037_expand_chunks_split_file_tokens_tsv.sql`.
- **Sai lầm về khái niệm:**
  Lầm tưởng `SET row_security = off` là cờ để tắt kiểm tra RLS (tương tự như bypass).
- **Thực tế trong PostgreSQL:**
  PostgreSQL định nghĩa `row_security = off` mang ý nghĩa: **"Nếu truy vấn bị ảnh hưởng bởi chính sách RLS thì hãy báo lỗi ngay lập tức thay vì âm thầm lọc dòng"** (raise an error instead of silently filtering rows). Thiết lập này nhằm mục đích phát hiện lỗi truy vấn hoặc ngăn rò rỉ dữ liệu ngoài ý muốn, tuyệt đối không phải là công cụ bypass.
- **Kết quả:**
  Vì bảng đang bật `FORCE ROW LEVEL SECURITY`, lệnh `UPDATE` của owner lập tức bị chặn và quăng ngoại lệ:
  ```text
  ERROR: query would be affected by row-level security policy for table "chunks"
  HINT: To disable the policy for the table's owner, use ALTER TABLE chunks NO FORCE ROW LEVEL SECURITY.
  SQLSTATE: 42501 (ERRCODE_INSUFFICIENT_PRIVILEGE)
  ```
  Migration thất bại hoàn toàn.


---

## 3. Giải pháp chuẩn (Safe Backfill Pattern)

Tuân thủ chỉ dẫn trực tiếp (`HINT`) từ chính PostgreSQL: Table owner tạm thời gỡ bỏ cờ `FORCE` trong phạm vi transaction của migration, thực hiện backfill toàn bộ dữ liệu, sau đó bật lại ngay lập tức.

### 3.1 Mẫu SQL chuẩn (Template)
Tham khảo migration chuẩn đã khắc phục: `crates/server/migrations/0037_expand_chunks_split_file_tokens_tsv.sql`.

```sql
-- 1. Tạm gỡ FORCE để owner (migrator) có thể thao tác đa tenant
ALTER TABLE <table_name> NO FORCE ROW LEVEL SECURITY;

-- 2. Thực hiện cập nhật dữ liệu (backfill)
UPDATE <table_name>
SET <column_name> = <expression>;

-- 3. Khôi phục lại FORCE ROW LEVEL SECURITY ngay lập tức
ALTER TABLE <table_name> FORCE ROW LEVEL SECURITY;
```

### 3.2 Vì sao phương pháp này an toàn tuyệt đối?
1. **An toàn nhờ Transaction (`crates/server/src/database.rs:218-240`):**
   Trong kiến trúc migration engine của Markhand, mỗi file migration được bọc độc lập trong một transaction (`client.transaction().await` ... `transaction.commit().await`). PostgreSQL hỗ trợ **Transactional DDL** hoàn chỉnh. Nếu quá trình `UPDATE` gặp sự cố (lỗi dữ liệu, timeout, thiếu bộ nhớ...), toàn bộ transaction sẽ bị `ROLLBACK`, bao gồm cả lệnh `ALTER TABLE`. Bảng dữ liệu được đảm bảo không bao giờ bị bỏ quên ở trạng thái thiếu `FORCE`.
2. **Không làm rò rỉ phân quyền giữa các tenant:**
   `ENABLE ROW LEVEL SECURITY` vẫn giữ nguyên hiệu lực trong suốt thời gian chạy migration. Lệnh `NO FORCE` chỉ cho phép **chủ sở hữu bảng (table owner)** tạm thời thao tác dữ liệu mà không bị chặn bởi policy; tất cả các kết nối và role người dùng khác (như role ứng dụng `app`) vẫn bị RLS kiểm soát 100%.

---

## 4. Quy trình kiểm tra & Xác minh (Verification)

Tuyệt đối không chỉ nhìn vào dòng trạng thái "migration applied successfully" hoặc exit code `0`. Khi viết và thử nghiệm migration backfill:

1. **Quan sát số dòng bị ảnh hưởng:**
   Xác nhận lệnh `UPDATE` trả về số lượng dòng thực tế (`UPDATE n` với `n > 0`), không phải `UPDATE 0`.
2. **Kiểm tra trạng thái bảo mật sau migration:**
   Đảm bảo cả hai thuộc tính `relrowsecurity` và `relforcerowsecurity` của bảng đều là `true`:
   ```sql
   SELECT relname, relrowsecurity, relforcerowsecurity
   FROM pg_class
   WHERE relname = '<table_name>';
   ```
3. **Kiểm tra dữ liệu thực tế (Spot Check / Recall Check):**
   - Viết test hoặc chạy query kiểm chứng dữ liệu mới được tính toán (ví dụ: kiểm tra tsvector đã sinh ra đủ token mới như trong ca `0037`).
   - Thử nghiệm truy vấn bằng role ứng dụng không có ngữ cảnh `org_id` để đảm bảo RLS vẫn cách ly dữ liệu triệt để (0 dòng được trả về).

---

## 5. Danh mục các bảng đang FORCE ROW LEVEL SECURITY trong Markhand

Khi viết migration động chạm đến các bảng sau, luôn tuân thủ playbook này nếu cần backfill:
- `chunks`
- `documents`
- `document_versions`
- `org_memberships`
- `org_invites`
- `roles` / `role_permissions`
- `groups` / `group_memberships`
- `collections` / `collection_user_access` / `collection_group_access` / `collection_role_access`
- `refresh_tokens`
- `qa_chat_sessions` / `qa_chat_turns`
- `vector_cleanup_intents`
