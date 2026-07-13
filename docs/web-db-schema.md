# Markhand Web — PG schema DDL (spec cho task 1.7)

> Chi tiết hoá mục 4 của [`web-architecture.md`](web-architecture.md). Đây là spec để viết
> migration sqlx — đúng thứ tự nhóm dưới đây, mỗi nhóm 1 migration/PR. PG 16.
> Quy ước: PK `UUID DEFAULT gen_random_uuid()`; timestamp `TIMESTAMPTZ`; mọi bảng nghiệp vụ
> có `org_id NOT NULL` (trừ bảng global); enum nghiệp vụ = `TEXT + CHECK` (đổi giá trị không
> cần ALTER TYPE); soft-state qua cột, không xoá row khi còn tham chiếu.

## Extension (migration 0)

```sql
CREATE EXTENSION IF NOT EXISTS unaccent;

-- unaccent() mặc định STABLE, không dùng được trong GENERATED column → wrapper IMMUTABLE
-- (workaround phổ biến; an toàn vì dictionary unaccent không đổi runtime)
CREATE OR REPLACE FUNCTION immutable_unaccent(text) RETURNS text AS
$$ SELECT public.unaccent('public.unaccent', $1) $$
LANGUAGE sql IMMUTABLE PARALLEL SAFE STRICT;
```

## Nhóm 1 — Tenancy & Auth (chặn 2.2, 2.3)

```sql
CREATE TABLE orgs (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    slug        TEXT NOT NULL UNIQUE,           -- dùng trong URL/log, không đổi
    name        TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','suspended')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE users (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email         TEXT NOT NULL,
    password_hash TEXT NOT NULL,                -- argon2id
    display_name  TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','disabled')),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX users_email_lower ON users (lower(email));

-- Role: org_id NULL = system role (seed owner/admin/editor/viewer);
-- org_id NOT NULL = custom role tương lai (schema sẵn đường nâng, POC không làm UI)
CREATE TABLE roles (
    id      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id  UUID REFERENCES orgs(id) ON DELETE CASCADE,
    key     TEXT NOT NULL,                      -- 'owner'|'admin'|'editor'|'viewer'|custom
    name    TEXT NOT NULL
);
CREATE UNIQUE INDEX roles_org_key ON roles (COALESCE(org_id, '00000000-0000-0000-0000-000000000000'::uuid), key);

CREATE TABLE role_permissions (
    role_id    UUID NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    permission TEXT NOT NULL,                   -- xem bảng seed cuối file
    PRIMARY KEY (role_id, permission)
);

CREATE TABLE org_memberships (
    org_id     UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role_id    UUID NOT NULL REFERENCES roles(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, user_id)
);
CREATE INDEX org_memberships_user ON org_memberships (user_id);

CREATE TABLE refresh_tokens (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,            -- CHỈ lưu hash (SHA-256), không lưu token thô
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    user_agent TEXT,
    ip         INET,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX refresh_tokens_user ON refresh_tokens (user_id) WHERE revoked_at IS NULL;
```

## Nhóm 2 — Collections & Documents & Chunks (chặn 2.5, 2.7, 3.2)

```sql
CREATE TABLE collections (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id     UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    visibility TEXT NOT NULL DEFAULT 'private' CHECK (visibility IN ('private','org','groups')),
    created_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, name)
);

-- ACL cấp user cho visibility='private'|'groups' (POC: gán trực tiếp user; groups để sau)
CREATE TABLE collection_access (
    collection_id UUID NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    user_id       UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    level         TEXT NOT NULL CHECK (level IN ('read','write','manage')),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (collection_id, user_id)
);

CREATE TABLE documents (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    collection_id   UUID NOT NULL REFERENCES collections(id),
    title           TEXT NOT NULL,               -- heading đầu hoặc tên file
    filename        TEXT NOT NULL,               -- tên gốc đã sanitize (chống traversal)
    format          TEXT NOT NULL,               -- FormatKind::as_str: 'pdf'|'docx'|'xlsx'|'csv'|'md'|'txt'|'image'
    status          TEXT NOT NULL DEFAULT 'uploaded' CHECK (status IN
                      ('uploaded','converting','converted','indexing','indexed','failed')),
    error           TEXT,                        -- message khi failed (user đọc được)
    content_hash    TEXT NOT NULL,               -- BLAKE3 hex của bytes gốc (dedup + idempotency)
    size_bytes      BIGINT NOT NULL,
    minio_key       TEXT NOT NULL,               -- object file gốc
    markdown_key    TEXT,                        -- object .md sau convert (NULL trước converted)
    current_version INT NOT NULL DEFAULT 1,
    deleted_at      TIMESTAMPTZ,                 -- TOMBSTONE: set trước, job cleanup xoá Qdrant/MinIO sau.
                                                 -- MỌI query đọc (search/citation/preview) PHẢI lọc deleted_at IS NULL
    created_by      UUID NOT NULL REFERENCES users(id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX documents_org_coll_status ON documents (org_id, collection_id, status) WHERE deleted_at IS NULL;
CREATE INDEX documents_org_hash        ON documents (org_id, content_hash);

CREATE TABLE document_versions (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id       UUID NOT NULL,
    document_id  UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    version      INT  NOT NULL,
    content_hash TEXT NOT NULL,
    minio_key    TEXT NOT NULL,
    markdown_key TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (document_id, version)
);

-- chunks: nguồn sự thật cho rebuild index. POC: 1 bảng + index org_id.
-- Partition-ready: unique key CHỨA org_id để sau này PARTITION BY HASH (org_id)
-- không phải làm lại constraint (partition key bắt buộc nằm trong unique).
CREATE TABLE chunks (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),  -- id này ghi vào payload Qdrant
    org_id       UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    document_id  UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    version      INT  NOT NULL,
    chunk_index  INT  NOT NULL,
    heading_path TEXT NOT NULL DEFAULT '',       -- 'H1 > H2 > H3' (chunk.rs)
    text         TEXT NOT NULL,
    chars        INT  NOT NULL,
    tsv          tsvector GENERATED ALWAYS AS
                   (to_tsvector('simple', immutable_unaccent(text))) STORED,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, document_id, version, chunk_index)
);
CREATE INDEX chunks_tsv_gin  ON chunks USING GIN (tsv);
CREATE INDEX chunks_org_doc  ON chunks (org_id, document_id);
```

## Nhóm 3 — Jobs & Embedding signature (chặn 2.6, 3.1)

```sql
CREATE TABLE jobs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    document_id     UUID REFERENCES documents(id) ON DELETE CASCADE,  -- NULL cho reconcile
    type            TEXT NOT NULL CHECK (type IN ('convert','index','delete_cleanup','reconcile')),
    status          TEXT NOT NULL DEFAULT 'queued' CHECK (status IN
                      ('queued','running','done','failed','canceled')),
    attempts        INT  NOT NULL DEFAULT 0,
    max_attempts    INT  NOT NULL DEFAULT 3,
    locked_by       TEXT,                        -- worker id (hostname+pid)
    locked_at       TIMESTAMPTZ,
    checkpoint      JSONB,                       -- vd index: {"batch": 12, "last_chunk_index": 767}
    idempotency_key TEXT NOT NULL UNIQUE,        -- vd 'index:<document_id>:v<version>'
    error           TEXT,
    run_after       TIMESTAMPTZ NOT NULL DEFAULT now(),   -- backoff retry
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- Claim: SELECT ... WHERE status='queued' AND run_after<=now()
--        ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1
CREATE INDEX jobs_claim ON jobs (run_after, created_at) WHERE status = 'queued';
CREATE INDEX jobs_doc   ON jobs (document_id);

-- Pin model+dimension+version của embedding (pattern index signature desktop).
-- Qdrant collection đặt tên theo signature id → đổi model = signature mới + reindex,
-- không ghi đè collection cũ.
CREATE TABLE embedding_signatures (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    provider   TEXT NOT NULL,                    -- 'glm' | 'vllm' | 'hash-local'
    model      TEXT NOT NULL,
    dimension  INT  NOT NULL,
    version    INT  NOT NULL DEFAULT 1,
    active     BOOLEAN NOT NULL DEFAULT false,   -- chỉ 1 active tại một thời điểm
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (provider, model, dimension, version)
);
CREATE UNIQUE INDEX embedding_signatures_one_active ON embedding_signatures (active) WHERE active;
```

## Nhóm 4 — Quota & Audit (chặn 2.8)

```sql
CREATE TABLE org_quotas (
    org_id                UUID PRIMARY KEY REFERENCES orgs(id) ON DELETE CASCADE,
    storage_bytes_limit   BIGINT,                -- NULL = không giới hạn
    llm_tokens_month_limit BIGINT,
    upload_bytes_month_limit BIGINT,
    concurrent_jobs_limit INT NOT NULL DEFAULT 4,
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Đếm usage theo tháng; cộng dồn khi finalize reservation
CREATE TABLE usage_counters (
    org_id  UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    period  DATE NOT NULL,                       -- ngày đầu tháng
    metric  TEXT NOT NULL CHECK (metric IN ('storage_bytes','upload_bytes','llm_tokens','jobs_run')),
    used    BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (org_id, period, metric)
);

-- Reserve→finalize/refund atomic (KHÔNG check-then-act):
--   reserve : 1 transaction — khoá row org_quotas (SELECT FOR UPDATE), tính
--             used + SUM(reserved đang treo) + amount <= limit → INSERT reservation
--   finalize: reservation → finalized + cộng usage_counters (1 transaction)
--   refund  : reservation → refunded (job fail/hết hạn)
CREATE TABLE quota_reservations (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id     UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    user_id    UUID REFERENCES users(id),
    metric     TEXT NOT NULL,
    amount     BIGINT NOT NULL CHECK (amount > 0),
    status     TEXT NOT NULL DEFAULT 'reserved' CHECK (status IN ('reserved','finalized','refunded')),
    job_id     UUID REFERENCES jobs(id),
    expires_at TIMESTAMPTZ NOT NULL,             -- TTL: reconcile refund reservation mồ côi
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX quota_reservations_pending ON quota_reservations (org_id, metric) WHERE status = 'reserved';

CREATE TABLE audit_log (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    org_id      UUID,                            -- NULL cho sự kiện hệ thống
    user_id     UUID,
    action      TEXT NOT NULL,                   -- 'auth.login'|'auth.login_failed'|'doc.upload'|'doc.delete'|'member.role_change'|'acl.change'|'export.run'|...
    target_type TEXT,                            -- 'document'|'collection'|'user'|...
    target_id   TEXT,
    detail      JSONB,                           -- KHÔNG chứa secret/nội dung tài liệu
    ip          INET,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX audit_log_org_time ON audit_log (org_id, created_at);
```

## Nhóm 5 — Q&A sessions & RAG log (chặn 3.4, 4.3; 3.5 đọc để eval)

Nguyên tắc: log RAG chứa **trích đoạn nội dung tài liệu** → org-scoped, cascade khi xoá
org, retention policy per-org (config sau — POC giữ vô hạn). `audit_log` KHÔNG chứa
excerpt (chỉ hành vi); nội dung nằm ở nhóm này.

```sql
CREATE TABLE qa_sessions (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id        UUID NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
    user_id       UUID NOT NULL REFERENCES users(id),
    collection_id UUID REFERENCES collections(id),  -- NULL = hỏi trên mọi collection được phép
    title         TEXT NOT NULL DEFAULT '',          -- câu hỏi đầu tiên, cắt ngắn
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX qa_sessions_org_user ON qa_sessions (org_id, user_id, updated_at DESC);

CREATE TABLE qa_messages (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id        UUID NOT NULL REFERENCES qa_sessions(id) ON DELETE CASCADE,
    org_id            UUID NOT NULL,
    role              TEXT NOT NULL CHECK (role IN ('user','assistant')),
    content           TEXT NOT NULL,          -- câu hỏi, hoặc answer hoàn chỉnh sau khi stream xong
    status            TEXT NOT NULL DEFAULT 'done' CHECK (status IN ('done','fallback','error')),
                                              -- 'fallback' = LLM lỗi, trả trích đoạn (3.4)
    model             TEXT,                   -- model chat tại thời điểm trả lời
    prompt_tokens     INT,                    -- usage thật từ response (nối quota 2.8)
    completion_tokens INT,
    retrieval         JSONB,                  -- log retrieval để debug/eval (3.5):
                                              -- {latency_ms, k, fts_ms, vec_ms,
                                              --  candidates:[{chunk_id, score_vec, score_fts, rank_final}]}
    error             TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX qa_messages_session ON qa_messages (session_id, created_at);

-- Citation snapshot: PHẢI sống sót qua reindex/tombstone để trace được "lúc đó trả lời
-- dựa trên gì" → denormalize, KHÔNG FK cứng sang chunks (chunks bị thay khi reindex).
-- Fetch live (mở citation trên UI) đi đường document_id → re-check ACL + tombstone.
CREATE TABLE qa_citations (
    id           BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    message_id   UUID NOT NULL REFERENCES qa_messages(id) ON DELETE CASCADE,
    org_id       UUID NOT NULL,
    document_id  UUID NOT NULL,               -- không FK: giữ trace kể cả khi doc purge
    chunk_id     UUID,                        -- id chunk tại thời điểm trả lời (tra live nếu còn)
    version      INT  NOT NULL,
    chunk_index  INT  NOT NULL,
    heading_path TEXT NOT NULL,
    excerpt      TEXT NOT NULL,               -- trích đoạn đã đưa vào prompt
    score        REAL,
    rank         INT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX qa_citations_msg ON qa_citations (message_id);
CREATE INDEX qa_citations_doc ON qa_citations (org_id, document_id);
```

Log vận hành khác KHÔNG vào DB (YAGNI): pipeline status đã có `jobs.checkpoint/error`,
hành vi nhạy cảm đã có `audit_log`, log kỹ thuật đi `tracing` ra stdout/file — không
dựng bảng log riêng ở POC.

## Thiết kế Qdrant (spec cho task 2.4 / 3.1 / 3.2)

**Collection & versioning theo embedding signature**
- Tên collection: `chunks_<8-hex-đầu-signature-id>` — gắn 1-1 với row `embedding_signatures`.
- Đổi model/dimension = tạo signature mới + collection mới + reindex từ PG `chunks`,
  switch `active` khi xong, xoá collection cũ sau — KHÔNG ghi đè collection đang chạy.

**Point**
- `id` = `chunks.id` (UUID) — trùng khoá PG để đối chiếu/reconcile 1-1.
- Vector: `dimension` theo signature, distance **Cosine**.

**Payload (tối thiểu — text nằm ở PG, lấy theo chunk id)**

| Field | Kiểu | Payload index |
|---|---|---|
| `org_id` | uuid/keyword | ✅ bắt buộc |
| `collection_id` | uuid/keyword | ✅ bắt buộc |
| `document_id` | uuid/keyword | ✅ (phục vụ delete theo tài liệu) |
| `version` | integer | — |

Payload index tạo NGAY khi init collection — thiếu index thì filter quét toàn collection
(đây là điều 1.10 phải đo để xác nhận).

**Config khởi điểm (1.10 benchmark xong chỉnh lại)**
- Scalar quantization int8, `quantile 0.99`, `always_ram: true`; vector gốc `on_disk: true`.
- HNSW mặc định (`m=16`, `ef_construct=100`); `ef` search tune theo 1.10.

**Truy vấn & ghi**
- Search: `must [org_id == ctx.org, collection_id IN allowed_ids]` — adapter **từ chối**
  query thiếu 1 trong 2 điều kiện (2.4), không trả kết quả rỗng âm thầm.
- Upsert (3.2): batch 64-256 điểm, `wait=true` để checkpoint per-batch chính xác
  (chậm hơn nhưng resume đúng; nếu 1.10 cho thấy quá chậm mới cân nhắc wait=false + verify).
- Delete: theo filter `document_id` (tombstone cleanup) và theo `version` cũ sau reindex.
- Snapshot per-collection (7.1); restore node sạch → verify count khớp PG → switch.

**Nhất quán PG ↔ Qdrant**
- PG `chunks` là nguồn sự thật; Qdrant rebuild được toàn bộ từ PG (drill 7.1).
- Reconcile (2.7): so `COUNT chunks` theo document vs điểm Qdrant theo `document_id`
  filter — điểm mồ côi (doc đã purge) xoá, chunk thiếu điểm → re-embed.

## State machine `documents.status` (spec cho task 2.7)

| Từ | Sang | Trigger | Side effect (idempotent) |
|---|---|---|---|
| — | `uploaded` | upload qua hết lớp chặn 2.5 | object vào MinIO, tạo job `convert` (idempotency `convert:<doc>:v<n>`) |
| `uploaded` | `converting` | worker claim job convert | — |
| `converting` | `converted` | convert OK | ghi `markdown_key` MinIO, tạo job `index` |
| `converting` | `failed` | lỗi/timeout, attempts ≥ max | `error` set, refund quota reservation |
| `converted` | `indexing` | worker claim job index | — |
| `indexing` | `indexed` | mọi batch checkpoint xong | finalize quota; vector đủ trong Qdrant + rows `chunks` |
| `indexing` | `failed` | lỗi, attempts ≥ max | vector đã ghi giữ nguyên (reconcile dọn), refund phần chưa finalize |
| `failed` | `converting`/`indexing` | user bấm retry (job mới, attempts reset) | — |
| bất kỳ | tombstone (`deleted_at`) | user xóa | tạo job `delete_cleanup` xoá Qdrant points + MinIO objects; row PG giữ lại |

Bất biến: transition khác bảng trên → reject (log + error). Reader (search/citation/preview/
download) lọc `deleted_at IS NULL AND status='indexed'` (preview markdown cho phép từ `converted`).

## Seed (migration cuối nhóm 1)

Role system (org_id NULL) × permission:

| permission | owner | admin | editor | viewer |
|---|---|---|---|---|
| `doc.upload` | ✅ | ✅ | ✅ | — |
| `doc.delete` | ✅ | ✅ | ✅ (của mình) | — |
| `qa.query` | ✅ | ✅ | ✅ | ✅ |
| `intel.use` | ✅ | ✅ | ✅ | — |
| `export.run` | ✅ | ✅ | ✅ | — |
| `member.manage` | ✅ | ✅ | — | — |
| `settings.manage` | ✅ | ✅ | — | — |
| `audit.view` | ✅ | ✅ | — | — |

("của mình" enforce ở repo layer bằng `created_by`, không thêm permission string riêng ở POC.)

Seed dev (ngoài migration — script riêng): 1 org `demo`, user owner + viewer, 1 collection `chung` visibility `org`.

## Bảng ↔ task sử dụng

| Bảng | Tạo ở | Dùng chính ở |
|---|---|---|
| orgs/users/roles/role_permissions/org_memberships/refresh_tokens | 1.7 nhóm 1 | 2.2, 2.3, 6.1 |
| collections/collection_access | 1.7 nhóm 2 | 2.2, 3.3 (ACL filter), 5.2, 6.1 |
| documents/document_versions | 1.7 nhóm 2 | 2.5, 2.7, 4.2 |
| chunks | 1.7 nhóm 2 | 3.2 (ghi), 3.3 (FTS), 7.1 (rebuild) |
| jobs | 1.7 nhóm 3 | 2.6, 2.7, 3.2, 5.2 (reindex) |
| embedding_signatures | 1.7 nhóm 3 | 3.1, 2.4 (tên collection Qdrant) |
| org_quotas/usage_counters/quota_reservations | 1.7 nhóm 4 | 2.8, 3.4 (token), 5.1/6.2 (dashboard) |
| audit_log | 1.7 nhóm 4 | 2.3 (login), 2.5, 6.1, 7.2 |
| qa_sessions/qa_messages/qa_citations | 1.7 nhóm 5 | 3.4 (ghi), 4.3 (lịch sử chat), 3.5 (eval đọc retrieval/citations) |
| Qdrant collection (ngoài PG) | 2.4 init theo spec trên | 3.1/3.2 (ghi), 3.3 (search), 2.7 (reconcile), 7.1 (snapshot) |

## Ghi chú cho người viết migration (task 1.7)

- Thứ tự migration: extension → nhóm 1 → 2 → 3 → 4 → 5 → seed role. Mỗi nhóm 1 PR.
- `sqlx migrate add -r <tên>` (có down migration ở POC cho dễ làm lại DB dev).
- KHÔNG dùng `DefaultHasher`/bigint cho `content_hash` — BLAKE3 hex TEXT (bài học desktop).
- `chunks.tsv` là GENERATED — insert chỉ ghi `text`, đừng ghi tsv thủ công.
- Mọi FK sang `orgs` đều `ON DELETE CASCADE`; sang `documents` cũng CASCADE — nhưng xóa
  nghiệp vụ đi đường tombstone + job, KHÔNG `DELETE FROM documents` trực tiếp (chỉ purge
  định kỳ sau khi cleanup xác nhận xong).
