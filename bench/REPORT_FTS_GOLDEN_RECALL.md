# Báo cáo ĐỘ CHÍNH XÁC TRUY HỒI FTS TIẾNG VIỆT (Recall@k & MRR) — fileconv-server

> **Mã công việc:** inter-v3-05  
> **Issue liên kết:** [#438 - Bộ câu hỏi golden (30 câu) + cổng recall@k cho FTS tiếng Việt](https://github.com/anhnth24/project-example/issues/438)  
> **Branch thực hiện:** `inter-v3/05-fts-golden-recall`  
> **Cơ chế đo:** Full-Text Search (PostgreSQL `to_tsvector` + `plainto_tsquery` theo Migration 0037 và `search::fts_search`), chạy độc lập không cần `FILECONV_OCR_API_KEY` hay khoá OpenRouter.

Đo lường chất lượng truy hồi của nhánh FTS trên bộ câu hỏi golden (30 câu) tiếng Việt với ground truth xác định từ fixture chuẩn hóa.

- **Corpus fixture:** `bench/fixtures/fts_golden_document.md` (34 chunks chia theo heading bởi `fileconv_core::chunk::chunk_markdown`).
- **Bộ câu hỏi golden:** `bench/fixtures/fts_golden_queries.json` (30 câu hỏi, phủ đủ 4 nhóm).
- **Ngưỡng kiểm soát hồi quy (CI Gating Constants):**
  - `MIN_FTS_GOLDEN_RECALL_AT_5 = 0.80` (80.0%)
  - `MIN_FTS_GOLDEN_MRR = 0.70` (0.7000)

---

## 1. Kết quả chi tiết từng câu hỏi (N = 30)

| ID | Nhóm | Thứ hạng đích | Recall@5 | Reciprocal Rank (RR) | Câu hỏi truy vấn |
|---|---|---:|---:|---:|---|
| q01 | doc_number | 1 | 1.0 | 1.0000 | Quyết định số 88/QĐ-UBND ban hành quy chế đối soát dữ liệu |
| q02 | doc_number | 1 | 1.0 | 1.0000 | Công văn số 1502/CV-CNTT hướng dẫn nâng cấp hạ tầng máy chủ |
| q03 | doc_number | 1 | 1.0 | 1.0000 | Nghị quyết số 2026/NQ-CP ban hành chương trình phát triển kinh tế số |
| q04 | doc_number | 1 | 1.0 | 1.0000 | Thông tư số 45/2026/TT-BTTTT quy chuẩn kỹ thuật bảo mật thông tin |
| q05 | doc_number | 1 | 1.0 | 1.0000 | Nghị định số 12/2025/NĐ-CP quy định chi tiết thi hành Luật Viễn thông |
| q06 | doc_number | 1 | 1.0 | 1.0000 | Thông báo số 99/TB-VPCP kết luận phiên họp Ủy ban Quốc gia về chuyển đổi số |
| q07 | doc_number | 1 | 1.0 | 1.0000 | Chỉ thị số 05/CT-TTg phòng chống mã độc bảo đảm an ninh mạng |
| q08 | doc_number | 1 | 1.0 | 1.0000 | Quyết định số 18/QĐ-BTC định mức tiêu chuẩn phân bổ dự toán kinh phí |
| q09 | date | 3 | 1.0 | 0.3333 | Hội đồng nghiệm thu bàn giao hệ thống trung tâm dữ liệu ngày 27/08/2026 |
| q10 | date | 2 | 1.0 | 0.5000 | Thời hạn nghiệm thu giai đoạn một dự án phần mềm ngày 15/03/2025 |
| q11 | date | 4 | 1.0 | 0.2500 | Hoàn thành triển khai nền tảng tích hợp chia sẻ dữ liệu ngày 01/01/2026 |
| q12 | date | 5 | 1.0 | 0.2000 | Hạn cuối tiếp nhận hồ sơ quyết toán thanh lý hợp đồng ngày 30/04/2025 |
| q13 | date | 1 | 1.0 | 1.0000 | Lễ công bố trao giải thưởng ngày 02/09/2026 tại Trung tâm Hội nghị Quốc gia |
| q14 | date | 1 | 1.0 | 1.0000 | Đoàn thanh tra kiểm toán tiến hành kiểm tra ngày 19/08/2026 tại các đơn vị thành viên |
| q15 | date | 1 | 1.0 | 1.0000 | Văn bản chính thức có hiệu lực thi hành kể từ ngày 10/10/2026 |
| q16 | article_chapter | 1 | 1.0 | 1.0000 | Điều 01 Phạm vi điều chỉnh hoạt động quản lý vận hành |
| q17 | article_chapter | 1 | 1.0 | 1.0000 | Điều 02 Giải thích từ ngữ thuật ngữ kỹ thuật chuyên ngành |
| q18 | article_chapter | 1 | 1.0 | 1.0000 | Điều 10 Thẩm quyền phê duyệt ngân sách dự toán kinh phí |
| q19 | article_chapter | 1 | 1.0 | 1.0000 | Điều 12 Trách nhiệm cơ quan thẩm định đánh giá tính khả thi |
| q20 | article_chapter | 1 | 1.0 | 1.0000 | Điều 15 Chế độ báo cáo định kỳ tình hình sử dụng ngân sách |
| q21 | article_chapter | 1 | 1.0 | 1.0000 | Điều 18 Quyền và nghĩa vụ các bên tham gia hợp đồng kinh tế |
| q22 | article_chapter | 1 | 1.0 | 1.0000 | Điều 20 Thời hạn giải quyết khiếu nại của đối tác và khách hàng |
| q23 | article_chapter | 1 | 1.0 | 1.0000 | Điều 24 Quy định xử lý kỷ luật vi phạm bồi thường thiệt hại |
| q24 | natural_query | 1 | 1.0 | 1.0000 | Quy trình đối soát dữ liệu giao dịch thanh toán điện tử |
| q25 | natural_query | 1 | 1.0 | 1.0000 | Tiêu chuẩn kỹ thuật mã hóa và lưu trữ an toàn cơ sở dữ liệu |
| q26 | natural_query | 1 | 1.0 | 1.0000 | Thủ tục đăng ký cấp thẻ kiểm soát ra vào tòa nhà |
| q27 | natural_query | 1 | 1.0 | 1.0000 | Trách nhiệm bàn giao và bảo quản hiện trạng tài sản công sau thanh lý |
| q28 | natural_query | 1 | 1.0 | 1.0000 | Nghĩa vụ bảo vệ thông tin đời sống riêng tư và dữ liệu người dùng |
| q29 | natural_query | 1 | 1.0 | 1.0000 | Cơ chế phối hợp liên ngành phòng chống tấn công mạng |
| q30 | natural_query | 1 | 1.0 | 1.0000 | Nguyên tắc quản lý rủi ro và kiểm soát nội bộ doanh nghiệp |

---

## 2. Trung bình theo nhóm câu hỏi

| Nhóm câu hỏi | Đặc điểm bài toán | Số câu | Recall@5 | MRR | Đánh giá cổng CI |
|---|---|---:|---:|---:|:---:|
| `doc_number` | Số hiệu văn bản hành chính chứa `/` (`CV-CNTT`, `QĐ-UBND`) | 8 | **1.0000** | **1.0000** | **ĐẠT** |
| `date` | Mốc ngày tháng (`27/08/2026`, `15/03/2025`, `01/01/2026`) | 7 | **1.0000** | **0.6119** | **ĐẠT** |
| `article_chapter` | Cấu trúc Điều / Chương pháp lý (`Điều 01`, `Điều 10`, `Điều 12`) | 8 | **1.0000** | **1.0000** | **ĐẠT** |
| `natural_query` | Câu hỏi ngữ nghĩa tự nhiên không chứa số | 7 | **1.0000** | **1.0000** | **ĐẠT** |
| **Tổng cộng** | **Toàn bộ 4 nhóm tiêu chuẩn** | **30** | **1.0000** | **0.9094** | **VƯỢT NGƯỠNG** |

---

## 3. Nhận xét và Kết luận

1. **Hiệu quả của Migration 0037:**
   - Các số hiệu có chứa dấu gạch chéo `/` như `1502/CV-CNTT`, `88/QĐ-UBND`, `2026/NQ-CP` đều đạt **Recall@5 = 100%** và **MRR = 1.0000**.
   - Việc tách sub-tokens và lập chỉ mục 2 luồng (nguyên thể + thay `/` bằng khoảng trắng) đã giải quyết triệt để lỗi phân tách của PostgreSQL simple parser.

2. **Chất lượng truy hồi nhóm Ngày tháng:**
   - Tất cả 7 câu hỏi ngày tháng đều truy hồi đúng chunk đích trong Top 5 (Recall@5 = 1.0000).
   - Một số mốc ngày xuất hiện ở nhiều văn bản khác nhau (vd: `27/08/2026` vừa có ở biên bản vừa có ở công văn) dẫn tới cạnh tranh thứ hạng trong top 5, làm MRR đạt 0.6119, nhưng vẫn bảo đảm tài liệu cần tìm nằm trong vùng hiển thị ưu tiên.

3. **Cổng kiểm soát hồi quy (Safety Net):**
   - Bài test `vietnamese_fts_golden_recall_at_5_gate` trong `crates/server/tests/retrieval.rs` tự động kiểm tra `recall_at_5 >= 0.80` và `mrr >= 0.70`.
   - Nếu bất kỳ commit nào trong tương lai làm hỏng tokenizer, bộ lọc stop-word, hoặc cấu trúc bảng `chunks.tsv`, test sẽ kích hoạt lỗi `panic!` và chặn merge trên CI.

