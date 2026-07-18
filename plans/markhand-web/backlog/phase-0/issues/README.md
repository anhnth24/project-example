# Phase 0 issues — Discovery và decision gates

Parent plan: [`../../../phase-0-discovery-and-gates.md`](../../../phase-0-discovery-and-gates.md)

<!-- roadmap-default-status: blocked -->

Phase F exit gate đã đạt. P0-01 được activate; các issue sau vẫn theo dependency graph.

## Dependency

```text
P0-01 ─┬─> P0-02 ─> P0-03 ─────────────┐
       ├─> P0-04 ─┬─> P0-05 ─> P0-06 ─┤
       │          ├─> P0-07 ───────────┤
       │          └─> P0-08 ───────────┤
       └─> P0-09 ──────────────────────┤
                                       └─> P0-10
P1A-01 ──────────> P0-03
```

## P0-01 — Khóa workload, hardware và gate registry

- **Status:** Done — approved Profile B, numeric targets and fail-closed validators
  merged to `master`.
- **Objective:** Thay giả định scale/SLA bằng workload envelope, hardware profile và
  gate schema được duyệt.
- **Plan:** Ghi org/collection/document/vector, ingest/query/recovery load; CPU/RAM/
  disk/GPU/network; tạo registry gồm metric, workload, threshold, command,
  environment, approver và failure disposition.
- **Files:** `bench/markhand_web/{README.md,workload-profile.yaml,gates.yaml}`,
  `bench/markhand_web/environments/`, `docs/adr/README.md`.
- **Dependencies/blocks:** Cần input sản phẩm/vận hành; block mọi benchmark.
- **Acceptance:** Normal/peak/recovery/aggregate load đầy đủ; mọi open decision có
  owner; gate thiếu trường bị schema validator từ chối.
- **Tests/evidence:** Validate YAML/schema; mọi report sau emit environment
  fingerprint.
- **Security/migration:** Không ghi credential, hostname nội bộ hoặc tên khách hàng.
- **Out of scope:** Chọn model và tuyên bố đạt SLA.

## P0-02 — Golden corpus tiếng Việt và adversarial corpus

- **Status:** Done — deterministic version/conflict corpus, dual adjudication and
  strict reproducibility gates passed.
- **Objective:** Dataset tái lập cho conversion, retrieval, citation và upload attack.
- **Plan:** Thêm mọi format; 200–500 query với expected document/source span/
  relevance/no-answer; multi-document và immutable multi-version citations
  (`current`/`as_of`/`compare`/`history`); BA/design/dev cross-document conflict
  lifecycle (open/resolved/history) với cited claims; sample spoof/bomb/malformed/
  traversal/prompt injection; pin checksum và provenance/license.
- **Files:** `bench/markhand_web/golden/`, `adversarial/`,
  `manifest.lock.json`, `scripts/validate_corpus.py`.
- **Dependencies/blocks:** P0-01; fixture phải redistributable.
- **Acceptance:** Coverage đủ category; source/version span ổn định; current fact không
  trỏ version cũ; compare/history cite đủ old+new và delta; conflict current/history
  cite đủ hai phía + resolution versions; validator bắt checksum, duplicate ID, invalid
  span/version/conflict lineage và missing license; mỗi attack có expected disposition.
- **Tests/evidence:** Clean-checkout reproducibility; dual review + adjudication.
- **Security/migration:** Synthetic/de-identified; bomb fixture chỉ chạy trong limits.
- **Out of scope:** Customer data và expected chunk ID trước khi chốt chunking.

## P0-03 — Mở rộng desktop baseline trên corpus Phase 0

- **Status:** Done — real release conversion/local-RAG baseline and independently
  recomputed evidence accepted as the current-state reference.
- **Objective:** Mở rộng parity baseline P1A-01 lên corpus/metrics Phase 0; P1A-01 là
  baseline authoritative để việc extraction không phải đợi toàn bộ corpus.
- **Plan:** Tái dùng fixtures/harness P1A-01; chạy release conversion; snapshot top-k,
  scores, anchors, answer modes,
  warnings, stats, provider fallback và signature mismatch.
- **Files:** `bench/markhand_web/scripts/run_desktop_baseline.sh`,
  `baselines/desktop-v1/`, `reports/desktop-baseline.md`.
- **Dependencies/blocks:** P0-02 + P1A-01 authoritative parity harness; provider run
  cần config/model pin.
- **Acceptance:** Mọi format/query có raw machine-readable result; offline chạy không
  cần LLM; đủ dữ liệu so parity 1A.
- **Tests/evidence:** CER/WER/time, Recall@5/10, MRR, nDCG, citation correctness;
  deterministic rerun.
- **Security/migration:** Redact key/prompt/absolute path.
- **Out of scope:** Sửa defect ranking/performance.

## P0-04 — Spike infrastructure tái lập

- **Status:** Done — reproducible stack, pinned images, three-store lifecycle and bound
  CPU-smoke evidence passed; Profile B GPU/IOPS measurements remain downstream gates.
- **Objective:** Stack disposable PG/Qdrant/MinIO/vLLM/telemetry cho benchmark.
- **Plan:** Tái dùng compose/services/scripts base từ F-08; thêm benchmark-specific
  override với isolated volumes/data, vLLM/GPU profile, workload sizing, image digest
  và environment fingerprint. Không fork dev stack.
- **Files:** `deploy/compose.spike.yml`, `deploy/spike/`, base `deploy/dev/`,
  `bench/markhand_web/scripts/spike-{health,reset}.sh`.
- **Dependencies/blocks:** Phase F/F-08 + P0-01; target hardware để đóng issue.
- **Acceptance:** Một command boot từ empty volumes; không thao tác console; restart/
  reset đúng semantics.
- **Tests/evidence:** Clean-machine startup, service health, version/GPU/telemetry
  fingerprint.
- **Security/migration:** Bind private/localhost; non-production secrets ngoài Git.
- **Out of scope:** HA/TLS/production orchestration.

## P0-05 — Đánh giá embedding tiếng Việt

- **Status:** Ready — interim GLM cloud path được duyệt (ADR 0004); local dense
  quality smoke recorded (ADR 0005 Proposed: AITeamVN PASS / BKAI FAIL on CPU).
  Target GPU/vLLM remains cutover, not a coding blocker.
- **Objective:** Chốt provider/model/revision/dimension/normalization đủ để lập
  trình Phase 0→1B; giữ đường cắt sang on-prem vLLM.
- **Plan:** Interim: so GLM `embedding-3` (và `embedding-2` nếu cần) qua
  OpenAI-compatible API + `FILECONV_EMBEDDING_API_KEY`; pin tokenizer/batch/
  truncation/dimensions/normalize; đo theo category và API latency. Parallel local
  dense evidence: `AITeamVN/Vietnamese_Embedding` vs `bkai` bi-encoder.
  Target (sau): so `bge-m3` và multilingual-e5 trên Profile B GPU/vLLM.
- **Files:** `bench/markhand_web/embedding/`, `scripts/run_embedding_eval.py`,
  `reports/embedding-evaluation.md`, `docs/adr/0004-interim-glm-cloud-embedding.md`,
  `docs/adr/0005-vietnamese-embedding-model-quality.md`.
- **Dependencies/blocks:** Corpus + spike + GLM credential; không còn bắt buộc
  target GPU để đóng interim. Cutover vLLM vẫn cần Profile B + approved model download.
- **Acceptance (interim — đủ đóng P0-05 cho coding/POC/DEMO):** ≥2 cấu hình GLM
  (model hoặc dimension) cùng golden corpus; cấu hình chọn đạt gate quality và không
  kém best vượt margin đã duyệt; config/signature immutable trong report; ghi rõ
  runtime=`glm-cloud-interim`.
- **Acceptance (target — deferred cutover, không chặn Phase 1B):** ≥2 model family
  local trên cùng Profile B hardware; có VRAM/throughput/saturation.
- **Tests/evidence:** Interim: Recall/MRR/nDCG, API P50/P95/P99, vectors/s ước lượng,
  failure rate; ≥2 runs. Target: thêm VRAM/saturation/cold-warm ≥3 runs.
- **Security/migration:** Chỉ synthetic/de-identified corpus lên GLM; customer/
  restricted data không ra cloud. Index signature phân biệt `glm-cloud` vs
  `vllm-local`; cắt sang vLLM = rebuild generation mới. License trước khi bundle
  local weights.
- **Out of scope:** Autoscaling; đổi desktop local-hash fallback mặc định.

## P0-06 — Chunking, hybrid tuning và index signature

- **Status:** Done — identity schema v2 + expected-chunks + golden `chunkId` fill;
  neural hybrid (AITeamVN CPU / `local-neural`) on `local-cpu-quality`; frozen RRF
  `VECTOR_WEIGHT=0.55`; version-citation P/R scored as top-k `(doc,version,chunkId)`;
  temporal/change/conflict gates via deterministic offline rules. Closes on P0-05
  CPU quality evidence (ADR 0005 may remain Proposed until product model acceptance);
  does not require Profile B / vLLM cutover.
- **Objective:** Chốt chunking/hybrid parameters và canonical signature.
- **Plan:** So chunk sizes; FTS/vector/hybrid; tune RRF; định nghĩa length-delimited
  signature gồm model/revision/dim/normalize/chunk/text-normalization version;
  version-aware identity và query modes current/as-of/compare/history; chuẩn hoá typed
  claim key/value/unit/scope/effective interval và deterministic numeric/enum/date/
  MUST-vs-MUST-NOT conflict candidates.
- **Files:** `bench/markhand_web/retrieval/`, `expected-chunks.tsv`,
  `reports/retrieval-evaluation.md`, ADR index signature.
- **Dependencies/blocks:** Desktop baseline + embedding result.
- **Acceptance:** So sánh identical candidates; source span vẫn resolve; signature
  test vector ổn định; chunk ID có document-version; temporal/current accuracy và
  version-citation precision/recall đạt gate; conflict precision/recall, current-warning
  và resolved-history accuracy có cited evidence.
- **Tests/evidence:** Recall/MRR/nDCG/citation/no-answer + variance; cross-run signature.
- **Security/migration:** Signature đổi tạo index generation mới, không trộn vector.
- **Out of scope:** Server adapters/ACL.

## P0-07 — PG/Qdrant target-scale topology

- **Status:** Done — 1B POC topology selected by offline/synthetic harness:
  Qdrant shared collection with mandatory `org_id` filter and PG no-partition
  for the single-org POC. Profile B `G0-SLO-QUERY-P99` / 20M mixed-load
  evidence still blocks production aggregate scale.
- **Objective:** Chọn Qdrant topology và PG partition strategy bằng mixed-load evidence.
- **Plan:** Generate realistic tenants; compare shared/cohort collection and
  PG no-partition/bounded hash offline with query+ingest+delete+snapshot
  markers. Re-run live on Profile B before production aggregate scale.
- **Files:** `bench/markhand_web/scale/`, `reports/scale-topology.md`, ADR Qdrant/PG.
- **Dependencies/blocks:** POC decision unblocked; production aggregate scale remains
  blocked by hardware/storage scale thật. Không chấp nhận offline extrapolation as
  Profile B evidence.
- **Acceptance:** 1B POC recommendation recorded in ADR 0008/0009 and
  `bench/markhand_web/scale/summary.json`; `productionScaleBlocked=true` until
  Profile B validates filtered P95/P99/recall and restore behavior.
- **Tests/evidence:** Offline harness self-test + full run. Deferred production
  evidence: latency, throughput, quantized recall, RAM/disk, compaction,
  noisy-neighbor, FTS on Profile B.
- **Security/migration:** Synthetic tenant; mọi query vẫn có tenant filter.
- **Out of scope:** Production RLS.

## P0-08 — Sizing converter và ingest backpressure

- **Status:** Done — interim local-cpu sizing harness/report closes P0-08
  deliverables with `targetMatch=false`; Profile B `G0-CAP-INGEST-THROUGHPUT`
  and production headroom remain blocked until measured on `on-prem-reference`.
- **Objective:** Chốt worker count, limits, timeout, queue và recovery headroom.
- **Plan:** Benchmark từng format native/scan/audio; single/concurrent; CPU/RAM/temp;
  PDFium serialization; converter-vs-GPU bottleneck.
- **Files:** `bench/markhand_web/ingest/`, `scripts/run_ingest_capacity.sh`,
  `reports/ingest-capacity.md`.
- **Dependencies/blocks:** Golden files + native deps available for local-cpu smoke;
  production capacity remains blocked by Profile B hardware.
- **Acceptance:** Mọi POC format có sizing/timeout and simulated queue-age evidence;
  ≥30% production resource headroom is not claimed from local-cpu.
- **Tests/evidence:** Harness self-test + full local-cpu run writes
  `bench/markhand_web/ingest/summary.json` and
  `bench/markhand_web/reports/ingest-capacity.md`; rerun on Profile B for
  gate pass evidence.
- **Security/migration:** Malformed input chỉ chạy dưới limits.
- **Out of scope:** Production job engine.

## P0-09 — Upload threat model, sandbox và license inventory

- **Status:** Done — local-cpu policy/sandbox smoke evidence closes upload
  threat model, adversarial disposition, and runtime license inventory. This
  does not claim Profile B malware scanner coverage.
- **Objective:** Security policy thực thi được trước khi nhận upload.
- **Plan:** Threat model spoof/bomb/parser/SSRF/exhaustion/traversal/injection/token/
  quota/tenant/compromised worker; chốt allowlist/limits/quarantine/sandbox; inventory
  source/version/checksum/license.
- **Files:** `docs/markhand-web-{upload-threat-model,upload-policy}.md`,
  `docs/markhand-web-model-license-inventory.md`, adversarial disposition YAML.
- **Dependencies/blocks:** P0-02/P0-08 evidence available for local Phase 0
  closure; production scanner/runtime hardening remains a later gate.
- **Acceptance:** Mỗi threat có prevention/detection/owner; sandbox non-root,
  read-only, no egress, resource/process/wall limits; unresolved model bị exclude.
- **Tests/evidence:** `python3 bench/markhand_web/scripts/run_upload_security.py`
  writes `bench/markhand_web/security/summary.json` and
  `bench/markhand_web/reports/upload-security.md`; policy linter and in-process
  sandbox smoke deny egress/traversal/fork/timeout. Runtime license checker
  passes with PhoWhisper excluded/not bundled.
- **Security/migration:** GLM policy theo data classification.
- **Out of scope:** Production malware scanner.

## P0-10 — ADR, SLO/RPO/RTO và Phase 0 gate

- **Status:** Blocked bởi P0-05…P0-09.
- **Objective:** Chuyển evidence thành quyết định và clean restore proof.
- **Plan:** ADR document/artifact, tenancy/RLS, partition, Qdrant, auth/session,
  index migration, backup order; chốt SLO; backup/restore ba hệ; close registry.
- **Files:** `docs/adr/`, `docs/markhand-web-{sla-targets,risk-register}.md`,
  `bench/markhand_web/reports/restore-drill.md`.
- **Dependencies/blocks:** P0-01…P0-09 + approvers.
- **Acceptance:** Mọi decision được duyệt hoặc block 1B; clean restore đạt RPO/RTO;
  gate link raw evidence; không high/critical/license issue thiếu disposition.
- **Tests/evidence:** Independent gate rerun; component-loss restore; checksum/
  query-ready/full-rebuild timing.
- **Security/migration:** PG authority; MinIO originals không reconstruct được;
  migration expand/cutover/contract.
- **Out of scope:** Production HA và user onboarding.

## Exit gate

P0-10 chỉ đóng khi quality, target-scale mixed load, capacity headroom, adversarial
disposition, clean restore, architecture decisions và license inventory đều đạt
numeric gate trong registry.
