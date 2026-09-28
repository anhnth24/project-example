# Báo cáo: Đánh giá chi phí double-fold tsvector của Migration 0037

> **Task ID:** inter-v3-04  
> **Issue:** [#437 - Đo chi phí double-fold tsvector của migration 0037 (dung lượng + độ trễ)](https://github.com/anhnth24/project-example/issues/437)  
> **Branch:** `inter-v3/04-tsv-cost-analysis`  
> **Ngày thực hiện:** 28/09/2026  

---

## 1. Mục tiêu và Bối cảnh

Migration `0037_expand_chunks_split_file_tokens_tsv.sql` đã được merge nhằm giải quyết bài toán tìm kiếm văn bản hành chính Việt Nam (chứa các số hiệu như `1502/CV-CNTT`, `88/QĐ-CNTT` hoặc ngày tháng `27/08/2026`). Do PostgreSQL parser `simple` coi chuỗi chứa `/` là một token nguyên khối kiểu `file`, người dùng tìm kiếm từ khóa con (sub-token) như `1502` hay `CV-CNTT` không khớp trong FTS (Full Text Search).

Giải pháp trong Migration 0037 là nối hai `to_tsvector`:
```sql
NEW.tsv := to_tsvector('simple', folded)
    || to_tsvector('simple', translate(folded, '/', ' '));
```
Thay đổi này cải thiện đáng kể Recall, tuy nhiên việc nhân đôi `to_tsvector` một cách vô điều kiện (unconditional double-fold) có thể gây lãng phí CPU, tăng kích thước dữ liệu và ảnh hưởng tới độ trễ truy vấn FTS. Báo cáo này lượng hóa đánh đổi thực tế trên môi trường chuẩn và so sánh với phương án fold có điều kiện (conditional fold).

---

## 2. Cấu hình môi trường đo (Environment Specs)

- **Hệ điều hành:** Linux 6.6.87.1-microsoft-standard-WSL2 (Ubuntu 26.04 LTS x86_64)
- **CPU:** 11th Gen Intel(R) Core(TM) i9-11950H @ 2.60GHz (8 cores, 16 threads, Turbo boost)
- **RAM:** 11 GB DDR4
- **Lưu trữ:** NVMe SSD (Host backing disk)
- **Database Engine:** PostgreSQL 18.4 (Debian 18.4-1.pgdg12+1) running in Docker
- **Quy mô Corpus:** 10,000 chunks (tương đương ~25 MB văn bản, mỗi chunk chứa 3–5 câu tiếng Việt, mã số và đường dẫn heading path)
- **Tỉ lệ có chứa ký tự `/`:** 20.0% (2,000 / 10,000 chunks), phản ánh đúng tỉ lệ thực tế của văn bản hành chính/kỹ thuật.
- **Quy chuẩn đo đạc:**
  - Mỗi kịch bản đều thực hiện `VACUUM FULL chunks`, `REINDEX TABLE chunks` và `ANALYZE chunks` trước khi đo để đảm bảo tính xác định và loại trừ dead tuples.
  - Mỗi câu query FTS được chạy khởi động (warmup) 3 lần, sau đó đo lường chính thức 10 lần (`EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)`) để tính toán `p50`, `p95`, `mean`, `min`, `max`.

---

## 3. Các kịch bản so sánh

1. **Pre-0037 (Single fold - Migration 0016):**
   Chỉ fold accent 1 lần, không tách dấu `/`:
   ```sql
   NEW.tsv := to_tsvector('simple', markhand_accent_fold(...));
   ```

2. **Migration 0037 (Unconditional double fold - Hiện tại):**
   Nối 2 `to_tsvector` cho mọi chunk bất kể có `/` hay không:
   ```sql
   folded := markhand_accent_fold(...);
   NEW.tsv := to_tsvector('simple', folded)
       || to_tsvector('simple', translate(folded, '/', ' '));
   ```

3. **Phương án đề xuất (Conditional fold):**
   Chỉ chạy fold thứ 2 khi đoạn text có chứa dấu `/`:
   ```sql
   folded := markhand_accent_fold(...);
   IF position('/' in folded) > 0 THEN
       NEW.tsv := to_tsvector('simple', folded)
           || to_tsvector('simple', translate(folded, '/', ' '));
   ELSE
       NEW.tsv := to_tsvector('simple', folded);
   END IF;
   ```

---

## 4. Kết quả đo lường thực tế

### 4.1. Dung lượng lưu trữ (Storage Footprint)

| Chỉ số đo | Pre-0037 (Single) | Migration 0037 (Double) | Proposed (Conditional) | Chênh lệch 0037 vs Pre | Chênh lệch Proposed vs 0037 |
|---|---:|---:|---:|---:|---:|
| **GIN Index (`idx_chunks__tsv`)** | **2.21 MB** (2,318,336 B) | **2.20 MB** (2,310,144 B) | **2.20 MB** (2,310,144 B) | -0.35% | 0.00% |
| **Cột `tsv` trong bảng (`chunks.tsv`)** | **8.48 MB** (8,890,250 B) | **10.36 MB** (10,864,284 B) | **8.71 MB** (9,131,234 B) | **+22.20% (+1.88 MB)** | **-15.95% (-1.65 MB)** |
| **Tổng bảng `chunks` (Data + Indexes)** | **24.11 MB** (25,280,512 B) | **26.12 MB** (27,394,048 B) | **24.28 MB** (25,460,736 B) | **+8.36% (+2.01 MB)** | **-7.06% (-1.84 MB)** |
| **Số lượng Lexemes duy nhất** | 689,284 (68.9 / chunk) | 693,419 (69.3 / chunk) | 693,419 (69.3 / chunk) | +0.60% | 0.00% |
| **Thời gian tính toán Trigger (10k rows)** | 2.37s | 2.91s | 2.57s | +22.78% | -11.68% |

#### Nhận xét quan trọng về cơ chế lưu trữ:
- **Tại sao GIN Index size gần như không đổi?**  
  Index GIN (Generalized Inverted Index) lưu trữ danh sách các lexeme duy nhất kèm mảng con trỏ (TID) trỏ tới các hàng chứa từ đó. Với 80% văn bản không có dấu `/`, chuỗi `translate(folded, '/', ' ')` hoàn toàn trùng khớp với chuỗi `folded` ban đầu, do đó không hề phát sinh thêm bất kỳ lexeme mới nào trong posting list của GIN index.
- **Tại sao dung lượng bảng `chunks` và cột `tsv` tăng vọt (+22.2%) ở 0037?**  
  Khi thực hiện phép nối hai vector `to_tsvector(...) || to_tsvector(...)` trong PostgreSQL, kiểu dữ liệu `tsvector` ghép nối mảng vị trí (positions) của các từ lại với nhau thay vì deduplicate hoàn toàn (ví dụ: `'van':3` trở thành `'van':3,52`). Điều này khiến dữ liệu nhị phân của cột `tsv` bị phình to ~22% trên toàn bảng đối với mọi dòng dù không có dấu `/`. Phương án **Conditional fold** đã loại bỏ hoàn toàn sự phình to này cho 80% số hàng, tiết kiệm ~1.65 MB trên 10k rows.

---

### 4.2. Độ trễ truy vấn FTS (Query Latency)

*Đo đạc 10 lần chạy chính thức (sau 3 lần warmup) với `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)`:*

| Query FTS | Phân loại | Số dòng match | Pre-0037 p50 / p95 (ms) | Migration 0037 p50 / p95 (ms) | Proposed Conditional p50 / p95 (ms) |
|---|---|---:|---:|---:|---:|
| `'1502'` | Sub-token số hiệu công văn | 122 (0 ở Pre) | 0.011 / 0.013 | **0.077 / 0.088** | **0.064 / 0.091** |
| `'cv-cntt'` | Sub-token mã đơn vị | 122 (0 ở Pre) | 0.011 / 0.019 | **0.205 / 0.281** | **0.244 / 0.396** |
| `'27 & 08'` | Sub-token ngày tháng | 95 (0 ở Pre) | 0.011 / 0.013 | **0.068 / 0.086** | **0.058 / 0.086** |
| `'van & ban'` | Cụm từ phổ biến (tần suất cao) | 5,905 | 8.463 / 11.108 | 13.172 / 14.672 | **10.187 / 13.902** |
| `'an & toan'` | Cụm từ kỹ thuật (tần suất vừa) | 600 | 0.247 / 0.284 | 0.326 / 0.501 | **0.266 / 0.412** |
| `'he & thong & quan & ly'` | Cụm 4 từ (tần suất cao) | 5,670 | 8.587 / 9.705 | 15.611 / 24.407 | **11.831 / 13.339** |

#### Nhận xét về Recall và Độ trễ:
1. **Recall:**
   - Pre-0037 có **0 kết quả** cho các từ khóa con chứa dấu gạch chéo (`1502`, `cv-cntt`, `27/08`), đúng như vấn đề đã nêu trong Issue.
   - Cả Migration 0037 và Conditional fold đều đạt **100% recall** (khớp chính xác 122 kết quả cho `1502`, 122 cho `cv-cntt` và 95 cho ngày tháng).
2. **Độ trễ:**
   - Đối với các query FTS nhiều từ (multi-word boolean AND như `'he & thong & quan & ly'` và `'van & ban'`), Migration 0037 khiến độ trễ tăng từ **8.5ms lên 15.6ms (tăng ~82%)**. Nguyên nhân do PostgreSQL phải duyệt mảng position lớn gấp đôi của từng từ để kiểm tra sự tương quan trong bitmap scan.
   - Phương án **Conditional fold** giúp hạ độ trễ p50 của `'he & thong & quan & ly'` xuống **11.8ms (nhanh hơn ~24% so với 0037)** do 80% bản ghi không bị nhân đôi danh sách vị trí token.
---

## 5. Kết luận và Đề xuất (Conclusions & Recommendations)

### 5.1. Kết luận đánh đổi
1. **Dung lượng Index:** Migration 0037 **không làm tăng dung lượng GIN index** (`2.20 MB` so với `2.21 MB`), vì GIN chỉ lưu trữ lexeme duy nhất.
2. **Dung lượng Bảng (Table Bloat):** Migration 0037 làm tăng dung lượng cột `tsv` **+22.2%** và tổng bảng `chunks` **+8.4%** do trùng lặp mảng `positions` trong kiểu dữ liệu `tsvector`.
3. **Độ trễ truy vấn:** Các truy vấn FTS tổ hợp nhiều từ (multi-word boolean query) bị chậm đi từ **20% đến 80%** trên tập dữ liệu do phải quét mảng vị trí dài hơn.
4. **Chi phí Trigger CPU:** Thời gian chạy trigger / backfill tăng **~23%** do phải parse `to_tsvector` 2 lần vô điều kiện cho mọi chunk.

### 5.2. Đề xuất hành động
- **Khuyến nghị:** **Đề xuất nâng cấp lên Conditional Fold** trong một PR migration riêng biệt (ví dụ migration `0038_optimize_chunks_conditional_tsv.sql`).
- **Lý do:**
  1. Bảo toàn **100% recall** cho các từ khóa chứa dấu gạch chéo (`1502/CV-CNTT`, ngày tháng).
  2. Tiết kiệm **~16% dung lượng cột `tsv`** (~1.65 MB trên 10k dòng, dự kiến tiết kiệm 165 MB trên 1M dòng và hàng chục GB ở quy mô production).
  3. Cải thiện độ trễ truy vấn FTS từ **15% đến 25%** cho các câu hỏi phổ biến nhiều từ.
  4. Giảm thời gian trigger insert/update cho 80% tài liệu thông thường.
- **Tuân thủ Scope:** Không tự ý sửa migration `0037` trong PR này (do `0037` đã merge).

---

## 6. Hướng dẫn chạy lại Benchmark (Reproduction)

```bash
# 1. Khởi động PostgreSQL test container (nếu chưa chạy)
docker run -d --name markhand-bench-postgres \
  -e POSTGRES_DB=markhand \
  -e POSTGRES_USER=markhand \
  -e POSTGRES_PASSWORD=markhand_dev_only \
  -p 127.0.0.1:54329:5432 \
  postgres:18.4-bookworm

# 2. Tạo database benchmark và áp dụng migrations
MARKHAND_MIGRATOR_DATABASE_URL="postgres://markhand:markhand_dev_only@127.0.0.1:54329/markhand_bench" \
  cargo run -p fileconv-server --bin fileconv-server -- --migrate-only

# 3. Chạy script đo lường
python3 bench/measure_tsv_cost.py
```

