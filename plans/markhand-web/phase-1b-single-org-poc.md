# Phase 1B — Secure single-org vertical slice

## Outcome

Một POC chạy on-prem cho một org và vài account:

```text
login → upload/quarantine → convert → index → search/Q&A → citation
```

POC phải có nền tenancy, isolation worker, durable jobs, observability,
reconciliation và restore; không phải server demo chỉ chạy happy path.

## P1B.1 — Extend server skeleton và POC deployment

Phase F đã tạo compileable skeleton và dev environment. Phase 1B mở rộng bằng
runtime/business modules và deployment POC:

```text
crates/server/
├── src/{auth,config,db,storage,jobs,routes,services,telemetry}
├── migrations/
└── tests/
deploy/
├── Dockerfile.server
├── Dockerfile.worker
├── compose.poc.yml
└── .env.example
```

Runtime:

- axum + tokio;
- sqlx/PostgreSQL;
- Qdrant client;
- S3-compatible client cho MinIO;
- OpenTelemetry/tracing;
- API và worker deploy độc lập.

Configuration validate lúc startup, secret qua mounted secret/env; không lưu
credential vào DB hoặc commit.

## P1B.2 — Multi-org-ready schema

Migration đầu tạo:

- `orgs`, `users`, `org_memberships`, `refresh_tokens`;
- `org_invites` với token hash, single-use và expiry;
- `roles`, `role_permissions`;
- `groups`, `group_memberships`;
- `collections` với `owner_user_id`, `collection_access` với principal
  user/group/role rõ ràng;
- `documents`, `document_versions`, `derived_artifacts`;
- `chunks` + FTS;
- `jobs`, `outbox_events`;
- `org_quotas`, `usage_counters`, `quota_reservations`;
- `audit_log`, `index_metadata`.

Mọi bảng nghiệp vụ có `org_id`. POC seed một org nhưng mọi repository method bắt
buộc nhận:

```rust
OrgContext {
    org_id,
    user_id,
    permissions,
    allowed_collection_ids,
}
```

Không expose query repository thiếu context. Đánh giá RLS từ Phase 0; nếu dùng,
test connection-pool không rò session org.

Document state:

```text
uploaded → converting → converted → indexing → indexed
                         └───────────────┴──────→ failed
indexed/deleted → tombstoned → purged
```

Version là immutable; original, Markdown và artifact dùng opaque key có
org/document-version identity. `documents.current_version_id` chỉ trỏ phiên bản đã
publish và đang hiệu lực, không mặc định là upload/draft mới nhất. Version giữ
`parent_version_id`, `version_number`, content hash, effective interval và deterministic
change summary.

## P1B.3 — Auth/session POC

- Dùng thư viện chuẩn cho Argon2 và JWT; không tự viết crypto.
- Pin algorithm, issuer, audience, expiry, clock skew và key ID.
- Access token ngắn hạn.
- Refresh token hash trong DB, rotate mỗi lần dùng, revoke cả family khi phát hiện
  reuse.
- Auth provider interface tách riêng để Phase 4 thêm OIDC.
- Seed owner/admin/editor/viewer; POC có thể chỉ expose owner/editor nhưng schema và
  permission constants đầy đủ.
- Audit login thành công/thất bại mà không log token/password.

## P1B.4 — Upload quarantine và validation

Pipeline:

1. Stream multipart với size limit; không buffer toàn file trong RAM.
2. Ghi MinIO quarantine bằng opaque key.
3. Kiểm magic bytes + extension allowlist.
4. Với OOXML: entry count, single-entry size, total uncompressed size, compression
   ratio và nested archive policy.
5. Kiểm page/duration/format limits.
6. Reserve quota trong transaction.
7. Tạo `documents(uploaded)` và outbox convert job; object vẫn ở quarantine.
8. Worker convert đọc quarantine. Khi convert thành công, saga idempotent
   copy/promote original + Markdown vào trusted prefixes, commit document version
   và outbox index job, rồi mới xóa quarantine bất đồng bộ.
9. Mỗi cross-system step có compensation; reject/failure thì giữ hoặc xóa quarantine
   theo retention policy và refund reservation.

File name chỉ là metadata đã sanitize; không dùng làm path/object key.

## P1B.5 — Isolated converter worker

Worker convert chạy tách API:

- unprivileged UID, read-only root filesystem;
- ephemeral working directory;
- no network egress mặc định;
- CPU/RAM/temp/file/process/wall-clock limits;
- kill process group/cgroup khi timeout;
- download object quarantine → convert → upload Markdown;
- không mount bucket credential có quyền ngoài prefix cần thiết.

Vì `Converter::convert_path` định dạng theo extension, worker materialize file bằng
canonical extension do server suy ra từ magic-byte + allowlist đã validate, không
dùng extension hoặc tên do người dùng cung cấp.

`Converter::convert_path` vẫn là engine. Wrapper worker chịu lease, timeout,
cancellation và cleanup. PDF/OCR/audio native dependency được đóng gói rõ trong
worker image.

## P1B.6 — Durable job engine

Job types:

- `convert`;
- `index`;
- `delete`;
- `reconcile`;
- `embedding_batch`.

Mỗi job có payload version, lease owner/expiry, heartbeat, attempts, retry class,
checkpoint, idempotency key, cancellation và dead-letter state. Claim bằng
`FOR UPDATE SKIP LOCKED`.

Outbox bảo đảm DB commit và enqueue không bị tách rời. Conversion và embedding dùng
queue/concurrency riêng để OCR không làm nghẽn GPU và ngược lại. Index checkpoint
theo batch; replay tối đa một batch và upsert không tạo duplicate.

## P1B.7 — Storage adapters và consistency

### PostgreSQL

- System of record cho metadata, chunk text, FTS, ACL, job, audit.
- BLAKE3/SHA-256 deterministic identities.
- Candidate hydration luôn kiểm document state và org.

### Qdrant

- Topology theo ADR Phase 0.
- Payload gồm org, collection, document, version và chunk.
- Adapter từ chối search khi thiếu org/filter collection.
- Point ID deterministic; collection versioned theo index signature.

### MinIO

- Prefix/bucket policy tách quarantine/original/markdown/artifact.
- Versioning và encryption theo deployment policy.
- Không trả public object key; download qua endpoint đã authorize hoặc signed URL
  TTL ngắn, single-purpose.

Consistency:

- PostgreSQL là nguồn sự thật.
- Delete/revoke tombstone trước; read path không trả nội dung ngay lập tức.
- Reconcile phát hiện và sửa orphan/stale giữa ba hệ.

## P1B.8 — Index, retrieval và Q&A

Index:

- chunk heading-aware từ `fileconv-core`;
- batch embedding qua vLLM;
- pin index signature;
- chunk/point identity luôn chứa immutable `version_id`; Qdrant payload có logical
  document, version number, effective time và `is_current`;
- ghi chunk/FTS vào PG và vector vào Qdrant idempotently.

Query:

1. Resolve org, allowed collections và version mode: current mặc định, hoặc
   `as_of`/`compare`/`history` khi caller có quyền.
2. Embed query.
3. Chạy Qdrant và PG FTS song song.
4. Merge/rerank bằng `fileconv-knowledge`.
5. Hydrate text từ PG, kiểm lại state/ACL.
6. Tạo citation pin logical document + immutable version/hash/span.
7. GLM trả lời chỉ trên retrieved passages.
8. Provider lỗi → extractive answer.

Citation endpoint luôn authorize lại. Prompt injection trong tài liệu không được
gọi tool, thay system policy hoặc mở rộng scope.

Current answer không được cite version cũ trừ khi kèm version note. Câu hỏi thay đổi/
lịch sử phải cite cả old+new và trả delta, effective dates, current version; không trộn
claim giữa version mà thiếu nhãn.

## P1B.9 — API và SSE

API tối thiểu:

- `POST /api/v1/auth/login|refresh|logout`;
- `GET /api/v1/me`;
- CRUD collection POC;
- upload/list/get/preview/delete/reindex document;
- job status và SSE job events;
- `POST /api/v1/search`;
- `POST /api/v1/ask`;
- `POST /api/v1/ask/stream`;
- document version list/diff và citation resolve theo `version_id`;
- health/readiness.

SSE event có version và sequence; reconnect dùng last-event ID hoặc snapshot +
resume. Stream bị đóng khi auth hết hạn/revoke. Error response có
`code/message/requestId`, không lộ nội bộ.

Định nghĩa OpenAPI và contract fixtures; Phase 2 dùng chúng làm mock.

## P1B.10 — Observability, audit và operations

Từ phase này phải có:

- trace API → job → converter → embedding → PG/Qdrant → GLM;
- request/job/document-version/index-signature correlation;
- metrics request latency, queue depth/age, conversion, timeout, embedding batch,
  retrieval legs, reconciliation drift, quota reservation, backup age;
- alert SLO burn, queue growth, disk, backup failure, drift và auth anomaly;
- audit upload/delete/ask/config/deny, không ghi document/prompt content.

Runbook:

- stuck/dead jobs;
- converter outbreak;
- vLLM/Qdrant/PG/MinIO outage;
- rebuild vector;
- disk exhaustion;
- GLM fallback;
- leaked credential/key rotation.

## P1B.11 — Backup, restore và migration

- PostgreSQL encrypted backup + PITR.
- MinIO versioning + backup/replication.
- Qdrant snapshot để restore nhanh; vẫn rebuild được từ PG chunks.
- Backup manifest ghi PG recovery point, MinIO marker, Qdrant snapshot,
  app/migration/index version.
- Backup định nghĩa consistency window/fence và ordering. Sau restore phải chạy
  reconciliation trước readiness; manifest chứa object/version inventory đủ để
  phát hiện missing/orphan.
- Migration immutable, lock khi chạy; CI test DB rỗng và upgrade path.
- Application rollback không yêu cầu DB rollback.

## Tests

- Upload: spoof, bomb, oversize, malformed và interrupted stream.
- State machine: invalid transition bị từ chối.
- Job: kill ở từng checkpoint, lease expiry, retry/dead-letter, cancel.
- Adapter: thiếu org/filter phải fail closed.
- Vertical slice mỗi format POC.
- Citation: deleted/revoked doc không xuất hiện.
- Reconciliation: orphan/stale cả PG/Qdrant/MinIO.
- Restore trên environment sạch.
- Mixed ingest/query soak, kiểm memory/temp/connection/queue growth.

## Gate

- Mỗi format allowlist chạy upload → indexed → query/citation thành công.
- Worker kill chỉ replay tối đa một batch, không duplicate visible data.
- Malicious corpus bị reject hoặc contained.
- Delete biến mất tức thời khỏi mọi read path. Minimum POC revoke gồm suspend user
  và remove collection membership; ACL chi tiết và in-flight revoke thuộc 1C.
- Reconcile sửa được drift có chủ đích.
- Clean restore đạt RPO/RTO.
- Soak test không có tăng trưởng tài nguyên vô hạn.
- Không API/repository/adapter nào chạy thiếu org context.

## Không thuộc phase

- Mở nhiều org cho người dùng thật.
- UI web hoàn chỉnh.
- BRD/PRD/PII trên web.
- OIDC/SSO.
