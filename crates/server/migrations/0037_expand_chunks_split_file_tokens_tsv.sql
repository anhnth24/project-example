-- Expand: split PostgreSQL `file` tokens in chunks.tsv (QA eval 2026-08-18).
-- Lock/data risk: rewrites chunks.tsv via trigger-compatible UPDATE; no schema drop.
-- Rollback compatibility: function/trigger replaceable; previous single-fold tsv regenerable.
--
-- Vì sao: văn bản hành chính VN đầy số hiệu dạng "1502/CV-CNTT", "88/QĐ-CNTT" và
-- ngày "27/08/2026". Parser 'simple' của PostgreSQL coi cả cụm là MỘT token kiểu
-- `file`, nên câu hỏi "công văn 1502" (token uint "1502") không bao giờ match FTS
-- — tài liệu chỉ còn nhờ vector leg và thường rớt khỏi top-k. Fix: tsv là hợp của
-- HAI folding — bản gốc (giữ match nguyên khối "27/08/2026") và bản thay '/' bằng
-- khoảng trắng (thêm sub-token "1502", "cv-cntt", "27", "08", "2026").

CREATE OR REPLACE FUNCTION chunks_set_tsv()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    folded text;
BEGIN
    folded := markhand_accent_fold(
        coalesce(array_to_string(NEW.heading_path, ' '), '') || ' ' || NEW.body
    );
    NEW.tsv := to_tsvector('simple', folded)
        || to_tsvector('simple', translate(folded, '/', ' '));
    RETURN NEW;
END;
$$;

-- Backfill existing rows so slash-bearing tokens are searchable by sub-token.
-- chunks có FORCE ROW LEVEL SECURITY nên cả owner (migrator) cũng bị policy
-- org-scope chặn UPDATE backfill. Lưu ý: `SET row_security = off` KHÔNG bypass
-- RLS — Postgres định nghĩa nó là "báo lỗi thay vì lọc ngầm", nên nó làm
-- migration fail 42501. Cách đúng (theo hint của Postgres) là owner tạm gỡ
-- FORCE trong đúng transaction migration này rồi bật lại ngay: migration chạy
-- trong một transaction (database.rs), nên nếu backfill lỗi thì ALTER cũng
-- rollback và bảng giữ nguyên FORCE ROW LEVEL SECURITY. Policy vẫn áp dụng
-- đầy đủ cho mọi role không phải owner trong suốt quá trình.
ALTER TABLE chunks NO FORCE ROW LEVEL SECURITY;
UPDATE chunks
SET body = body;
ALTER TABLE chunks FORCE ROW LEVEL SECURITY;
