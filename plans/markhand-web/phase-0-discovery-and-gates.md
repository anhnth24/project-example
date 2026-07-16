# Phase 0 — Discovery, benchmark và decision gates

## Outcome

Biến các giả định kiến trúc thành quyết định có số liệu trên phần cứng on-prem dự
kiến. Phase này không tạo production server; đầu ra là corpus, benchmark, threat
model, ADR và ngưỡng chấp nhận dùng để khóa thiết kế 1B.

## P0.1 — Baseline và corpus đánh giá

Tạo `bench/markhand_web/`:

- `golden/documents/`: tài liệu tiếng Việt đại diện cho PDF native/scan, DOCX,
  PPTX, XLSX, CSV, HTML, ảnh OCR, audio và TXT legacy.
- `golden/queries.tsv`: 200–500 câu hỏi đã adjudicate, gồm expected document,
  stable source span/citation và relevance grade. Expected chunk ID chỉ được sinh
  sau khi chốt chunking và phải mang chunking-version.
- Phủ biến thể dấu tiếng Việt, từ viết tắt, tên riêng, bảng, truy vấn dài và câu
  hỏi không có đáp án.
- `adversarial/`: extension giả, MIME mismatch, archive bomb, PDF/page bomb,
  audio dài, path traversal, malformed OOXML và prompt injection.
- Manifest pin SHA-256/BLAKE3 để corpus tái lập được.

Baseline phải ghi lại chất lượng và tốc độ desktop hiện tại để 1A có regression
reference.

## P0.2 — Hạ tầng spike có thể tái lập

Tạo `deploy/compose.spike.yml` và `.env.example` cho:

- PostgreSQL 16 với FTS/unaccent;
- Qdrant với telemetry và snapshot volume;
- MinIO với bucket quarantine/main;
- vLLM embedding endpoint trên GPU dự kiến;
- collector Prometheus/OpenTelemetry tối thiểu.

Không commit credential. Container pin version/digest; script health-check và seed
không phụ thuộc thao tác tay.

## P0.3 — Embedding và retrieval evaluation

So sánh ít nhất `bge-m3` và một model multilingual-e5 phù hợp VRAM:

- Recall@5/10, MRR, nDCG;
- chất lượng theo từng loại tài liệu và truy vấn;
- dimension, normalization, batch size, token truncation;
- throughput, VRAM, queue saturation;
- hybrid PG FTS + vector so với từng leg riêng.

Kết quả chốt:

- model + revision;
- dimension;
- normalization;
- chunking version/size;
- index signature canonical;
- hybrid weights/rerank baseline.

Không chọn model chỉ theo latency. Chất lượng tiếng Việt là ưu tiên.

## P0.4 — Qdrant/PG scale spike

Benchmark trên phân bố org/collection gần thực tế:

- 10–20M vector hoặc quy mô tối đa/org đã thống nhất;
- tổng vector và tenant distribution ở quy mô aggregate production dự kiến nếu
  chọn shared collection (không chỉ benchmark một org);
- query đồng thời ingest/delete;
- filter `org_id` + tập `collection_id` hẹp và rộng;
- P50/P95/P99, recall sau quantization, RAM, disk, compaction;
- delete/update payload, noisy neighbor;
- snapshot và restore;
- PG FTS latency/recall trên cùng chunk corpus.

So sánh:

- một shared Qdrant collection;
- collection theo cohort nếu shared collection không đạt isolation/performance;
- PG no-partition, bounded hash partition; không mặc định partition-per-org.

Kết quả nhỏ rồi extrapolate không được coi là bằng chứng cho scale 20M.

## P0.5 — Ingest capacity

Chạy converter release trên worker hardware:

- thời gian/file và trang theo format;
- OCR native/scan, audio transcription, Office archive;
- CPU/RAM/temp disk peak;
- throughput khi nhiều worker;
- tác động của PDFium global serialization;
- embedding queue khi converter nhanh hơn GPU và ngược lại.

Đầu ra là sizing worker pool, queue limits, timeout và headroom. Mục tiêu tối thiểu:
30% CPU/RAM/disk headroom và khả năng xử lý gấp đôi tải bình thường trong recovery.

## P0.6 — Threat model và upload policy

Tạo `docs/markhand-web-upload-threat-model.md`, phân tích:

- spoof MIME/extension, archive bomb, parser exploit, SSRF;
- tài nguyên cạn kiệt, path/object-key traversal;
- prompt injection và nội dung độc hại;
- cross-tenant access, token theft, quota race;
- compromised converter/embedding worker.

Chốt:

- allowlist format POC;
- max bytes/pages/duration/entries/uncompressed ratio;
- quarantine lifecycle;
- sandbox profile: unprivileged UID, read-only root, no egress mặc định, giới hạn
  CPU/RAM/file/process/wall-clock, process-group kill;
- policy GLM cloud theo phân loại dữ liệu.

Lập inventory model/native dependency gồm nguồn, version, checksum, license và
redistribution constraints. Model chưa rõ license, gồm PhoWhisper, không được bundle
vào image Phase 1B.

## P0.7 — ADR và SLA/DR

Viết ADR cho:

1. canonical document/version/artifact model;
2. tenant isolation và RLS;
3. PG partition strategy;
4. Qdrant topology;
5. auth/session lifecycle;
6. model/index migration;
7. backup authority và recovery order.

Chốt SLA/SLO:

- retrieval P95/P99;
- time-to-first-token;
- ingest throughput/worker;
- queue age;
- availability/degraded modes;
- RPO/RTO.

Chạy restore spike gồm PostgreSQL + MinIO + Qdrant; đo cả thời điểm service query
được và thời điểm vector rebuild hoàn tất.

Tạo gate registry cho mọi phase, mỗi gate ghi:

- metric và corpus/workload;
- numeric threshold;
- command/harness đo;
- hardware/environment;
- approver;
- failure disposition.

## Deliverables

- `bench/markhand_web/*` và các report benchmark.
- `deploy/compose.spike.yml`, health/seed scripts.
- Upload threat model.
- ADRs và `docs/markhand-web-sla-targets.md`.
- Capacity sheet và index signature đã pin.
- Danh sách risk có owner/mitigation.

## Test và gate

Phase 0 chỉ pass khi:

- golden set đủ coverage và review thủ công;
- model được chọn đạt threshold đã duyệt, không kém model tốt nhất quá biên cho phép;
- filtered retrieval đạt SLA dưới mixed load;
- converter/embedding capacity có headroom;
- malicious corpus có disposition rõ ràng;
- restore sạch đạt RTO/RPO đề xuất;
- mọi quyết định mở trong README đã được chốt hoặc ghi rõ là blocker.
- model/dependency bundle đã qua license gate.

## Không thuộc phase

- Không xây API production.
- Không đưa user thật lên spike.
- Không xem compose spike là manifest production.
