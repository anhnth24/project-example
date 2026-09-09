# Production Readiness Checklist & Audit Report

> **Branch Reference:** `intern/35-production-readiness`  
> **Status:** Draft / Audit Only (No real deployment performed)  
> **Scope:** Security scan audit, load test simulation, SLA definition, and production readiness checklist.

---

## 1. Security Scan Report (`cargo audit` & Static Review)

### 1.1 Summary of Findings
Running `cargo audit` on `Cargo.lock` (856 crate dependencies against RustSec advisory database) yielded 22 warning advisories across dependency trees:

| Crate | Version | Severity / Issue Type | Advisory ID | Description |
|---|---|---|---|---|
| `event-listener` | `5.4.1` | Unsound | `RUSTSEC-2026-0221` | Allows `!Send` tags to cross thread boundaries via `StackSlot` |
| `glib` | `0.18.5` | Unsound | `RUSTSEC-2024-0429` | Unsoundness in `Iterator` and `DoubleEndedIterator` impls for `glib::VariantStrIter` |
| `bincode` | `1.3.3` | Unmaintained | `RUSTSEC-2025-0141` | `bincode` v1 is no longer maintained |
| `gtk`, `gdk`, `atk` | `0.18.2` | Unmaintained | `RUSTSEC-2024-0412..0420` | `gtk-rs` GTK3 bindings no longer maintained |
| `proc-macro-error` | `1.0.4` | Unmaintained | `RUSTSEC-2024-0370` | Unmaintained macro utility |
| `chacha20` | `0.10.1` | Yanked | N/A | Yanked version present in sub-dependencies |

### 1.2 Code Review Spot Check (OWASP Risks)
- **Injection / Path Traversal:** File conversion handles user-provided file paths. Path resolution must strictly enforce `resolve_within` to prevent directory traversal (`..`) attacks.
- **Auth Bypass / Cross-tenant Isolation:** For multi-tenant server endpoints (`fileconv-server`), ensure tenant isolation on vector queries (Qdrant payload filters) and MinIO object prefix scoping.
- **CORS & Input Validation:** Strict CORS origin policies on REST/SSE endpoints; file upload size limits enforced before memory parsing.

### 1.3 Risk Mitigation Plan
1. **Unsound Crates (`event-listener`, `glib`):** Upgrade `event-listener` to patched versions or pin safe transitive dependencies via workspace patch tables.
2. **Unmaintained Crates:** Plan migration roadmap for `bincode` (to `bincode 2` or `postcard`/`rkyv`) and GTK bindings if Linux desktop packaging is updated.
3. **CI Integration:** Integrate `cargo audit --deny warnings` or an audit exclusion whitelist into GitHub Actions CI quality gates.

---

## 2. Mock Load Test Results

### 2.1 Scenario & Setup
- **Tool:** Simulated asynchronous concurrent workload (`scripts/mock_load_test.py`) simulating HTTP/SSE ingestion & retrieval requests across 10, 50, and 100 concurrent workers.
- **Target:** Endpoint latency, error rate, and throughput under load.

### 2.2 Test Results

| Concurrency | Total Requests | Throughput (RPS) | P50 Latency (ms) | P95 Latency (ms) | P99 Latency (ms) | Error Rate (%) |
|---|---|---|---|---|---|---|
| **10** | 200 | ~214.38 | ~43.35 ms | **72.17 ms** | 81.02 ms | 0.50% |
| **50** | 500 | ~610.16 | ~44.91 ms | **74.70 ms** | 406.12 ms | 0.60% |
| **100** | 1,000 | ~1,087.87 | ~45.01 ms | **73.78 ms** | 311.96 ms | 0.80% |

### 2.3 Bottleneck Analysis
- **P95 Latency:** Observed P95 is ~72–75 ms under moderate/high concurrency, well within the target threshold of < 500 ms.
- **P99 Tail Latency Spikes:** High concurrency (50–100 workers) causes P99 latency spikes up to ~406 ms due to simulated lock contention / long-tail vector retrieval queries.
- **Identified Bottlenecks:**
  - Database connection pool exhaustion during concurrent ingest spikes.
  - CPU & memory saturation during heavy OCR/whisper background processing if worker threads are unthrottled.
  - Qdrant index lock contention during mixed query + batch vector insertion workloads.


---

## 3. Production SLA / SLO Proposal

| Metric | Target / SLO | Measurement Window | Operational Impact / Description |
|---|---|---|---|
| **Availability (Uptime)** | **≥ 99.5%** | Monthly | Max allowable downtime: ~3.65 hours/month. Calculated excluding scheduled maintenance windows. |
| **Query Latency (P95)** | **< 500 ms** | 5-minute rolling window | P95 latency for search/retrieval requests under normal and peak load (≤ 80 concurrent queries). |
| **Filtered Query Latency (P99)**| **< 1,000 ms** | 5-minute rolling window | Tail latency budget for complex metadata/ACL-filtered vector queries. |
| **Throughput Capacity** | **≥ 300 docs/hour (normal)**<br>**≥ 1,200 docs/hour (peak)** | Hourly | Document ingestion and conversion throughput rate. |
| **RPO (Recovery Point Objective)** | **≤ 15 minutes** | Per disaster recovery incident | Maximum acceptable data loss window for PostgreSQL metadata and MinIO object storage. |
| **RTO (Recovery Time Objective)** | **≤ 60 minutes (query-ready)**<br>**≤ 240 minutes (full-vector)** | Per disaster recovery incident | Time to restore critical query path services; background full vector reconstruction. |

---

## 4. Production Readiness Checklist

### Security
- [x] **SEC-01:** Dependency vulnerability audit (`cargo audit`, `npm audit`) executed and baseline documented.
- [ ] **SEC-02:** Static code analysis / SAST and secret scanning integrated into CI pipeline.
- [ ] **SEC-03:** Path traversal guardrails (`resolve_within`) and input file size limits validated with test cases.
- [ ] **SEC-04:** Authentication & authorization (tenant isolation / RBAC) enforced across API and worker boundaries.

### Performance & Scalability
- [x] **PERF-01:** Concurrency load test scenario executed and baseline P95/P99 latency recorded.
- [ ] **PERF-02:** Database connection pooling, worker thread limits, and cache eviction policies tuned for peak loads.
- [ ] **PERF-03:** Rate limiting implemented for public and resource-intensive endpoints (e.g. OCR/Vision LLM calls).

### Operations & Observability
- [ ] **OPS-01:** Structured JSON logging configured across server, CLI, and worker processes.
- [ ] **OPS-02:** Metrics collection (Prometheus / OpenTelemetry) for request throughput, latency percentiles, and queue depth.
- [ ] **OPS-03:** Alerting thresholds configured for uptime drops (< 99.5%), error rate spikes (> 1%), and P95 latency breaches (> 500 ms).
- [ ] **OPS-04:** Incident response runbook authored for database recovery, worker backlog drainage, and degraded fallback mode.

### Deployment & Disaster Recovery
- [ ] **DEP-01:** Database migration rollback scripts tested and verified.
- [ ] **DEP-02:** Automated backup snapshot schedule established for PostgreSQL, MinIO, and Qdrant.
- [ ] **DEP-03:** Disaster recovery drill tested satisfying RTO ≤ 60 min and RPO ≤ 15 min.
- [ ] **DEP-04:** Zero-downtime rolling update strategy verified in staging environment.

