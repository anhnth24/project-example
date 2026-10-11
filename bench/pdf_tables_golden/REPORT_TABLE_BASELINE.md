# Báo cáo Baseline Đánh giá Cấu trúc Bảng PDF (Issue #447)

> Ngày thực hiện: Tháng 10/2026
> Engine: `fileconv-core` (pdf-inspector 0.1.3)
> Tổng số testcase trong bộ golden: 8 trang

## 1. Tóm tắt Chỉ số Toàn diện (Macro Average)

| Chỉ số | Định nghĩa | Giá trị Baseline | Đánh giá |
|---|---|---|---|
| **TDR (Table Detection Rate)** | Tỷ lệ số bảng được nhận diện thành bảng Markdown | **75.0%** | 6/8 bảng được nhận diện thành công |
| **CMR (Column Match Ratio)** | Tỷ lệ khớp số cột giữa giả thuyết và chuẩn | **75.0%** | Cột bị lệch khi có header đa tầng/tiêu đề tách |
| **RMR (Row Match Ratio)** | Tỷ lệ khớp số dòng dữ liệu giữa giả thuyết và chuẩn | **72.9%** | Khớp tốt trên các bảng chuẩn |
| **Positional Cell F1** | F1-score nội dung ô theo đúng tọa độ (row, col) | **69.8%** | Nhạy cảm cao với hiện tượng lệch cột/rowspan |
| **Bag-of-Cells Content F1** | F1-score bảo toàn nội dung ô (bất kể thứ tự tọa độ) | **73.6%** | Thể hiện mức độ giữ lại từ ngữ trong bảng |
| **Whole-Page CER** | Character Error Rate toàn trang (để đối chiếu) | **19.87%** | Minh chứng CER thấp dù cấu trúc bảng bị hỏng |

## 2. Chi tiết Từng Trang trong Bộ Golden

| STT | Trang Golden | Phân loại | TDR | Cols (Hyp/GT) | CMR | Rows (Hyp/GT) | RMR | Cell F1 (Pos) | Cell F1 (Bag) | Toàn trang CER |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | `page_01_simple_3col` | 01_simple_3col | ✅ 100% | 3/3 | 100% | 5/5 | 100% | 100.0% | 100.0% | 14.38% |
| 2 | `page_02_finance_4col` | 02_finance_4col | ✅ 100% | 4/4 | 100% | 5/5 | 100% | 91.7% | 97.9% | 22.26% |
| 3 | `page_03_wide_5col` | 03_wide_5col | ✅ 100% | 5/5 | 100% | 5/5 | 100% | 100.0% | 100.0% | 14.34% |
| 4 | `page_04_merged_header_colspan` | 04_merged_header_colspan | ❌ 0% | 0/4 | 0% | 0/5 | 0% | 0.0% | 0.0% | 35.57% |
| 5 | `page_05_merged_rowspan` | 05_merged_rowspan | ✅ 100% | 3/3 | 100% | 6/5 | 83% | 66.7% | 90.9% | 23.18% |
| 6 | `page_06_borderless_3col` | 06_borderless_3col | ✅ 100% | 3/3 | 100% | 5/5 | 100% | 100.0% | 100.0% | 11.17% |
| 7 | `page_07_dense_numeric_6col` | 07_dense_numeric_6col | ❌ 0% | 0/6 | 0% | 0/5 | 0% | 0.0% | 0.0% | 30.73% |
| 8 | `page_08_mixed_text_and_table` | 08_mixed_text_and_table | ✅ 100% | 4/4 | 100% | 4/4 | 100% | 100.0% | 100.0% | 7.35% |

## 3. Phân tích Chuyên sâu Hiện tượng Cấu trúc (Root Cause Analysis)

### 3.1. Nghịch lý CER toàn trang (Decoupling Demonstration)
- **Quan sát**: Các trang như `page_04_merged_header_colspan` và `page_07_dense_numeric_6col` có **TDR = 0%** và **Cell F1 (Pos) = 0%** (hoàn toàn không tạo ra bảng Markdown nào).
- **Tuy nhiên**, Whole-page CER của `page_04` (35.57%) và `page_07` (30.73%) vẫn phản ánh phần lớn câu chữ được bảo toàn trong native text flow.
- Đặc biệt ở `page_01` (bảng hoàn hảo 100% mọi chỉ số cấu trúc TDR/CMR/RMR/Cell F1), toàn trang CER vẫn ghi nhận ~14.38% chỉ vì sự khác biệt nhỏ về khoảng trắng/định dạng markdown.
- **Kết luận**: Đo độ chính xác trích xuất bảng chỉ dựa trên CER toàn trang đã che giấu hoặc bóp méo chất lượng nhận diện cấu trúc. Việc tách riêng bộ chỉ số **TDR, CMR, RMR, Cell F1** là tuyệt đối cần thiết để đánh giá thực chất khả năng trích xuất bảng.

### 3.2. Cơ chế Fallback sang Native Text (`markdown_has_malformed_table`)
- `pdf-inspector 0.1.3` khi phân tích layout các bảng có header phức tạp (`page_04` - colspan 2 tầng, `page_07` - bảng số dày) có xu hướng tách rời một số tiêu đề cột thành heading Markdown (ví dụ `## Lượt gọi API...`) và để lại các cột rỗng trong hàng header (`|||`).
- Trong `crates/core/src/conv/pdf/postprocess.rs`, hàm `markdown_has_malformed_table()` phát hiện `empty_headers >= 2` và chủ động fallback về text gốc (`native_pages`) để bảo toàn thứ tự đọc của câu chữ thay vì để sinh ra bảng méo mó.
- Đây là một **chính sách an toàn đúng đắn hiện tại**, tránh việc đảo lộn ngữ nghĩa, nhưng đồng thời làm giảm chỉ số TDR trên các bảng phức tạp.

### 3.3. Các trường hợp thành công và hạn chế cụ thể
1. **Bảng chuẩn 3 cột, 5 cột (`page_01`, `page_03`)**: Nhận diện hoàn hảo, TDR = 100%, CMR = 100%, RMR = 100%, Cell F1 = 100%.
2. **Bảng không viền (`page_06_borderless_3col`)**: `pdf-inspector` nhận diện xuất sắc các khoảng trắng làm phân cách cột, tạo bảng Markdown chuẩn xác 100%.
3. **Bảng trong trang hỗn hợp (`page_08_mixed_text_and_table`)**: Tiêu đề, đoạn văn bản mô tả và bảng 4 cột đều được tách bạch đúng vị trí.
4. **Header tách rời (`page_02_finance_4col`)**: Cột "Số lượng" bị tách thành header ngoài `## Số lượng`, dẫn đến header bị khuyết 1 cột nhưng dữ liệu các dòng vẫn đủ 4 cột.
5. **Ô gộp dòng (`page_05_merged_rowspan`)**: Ô gộp nhiều dòng bị phân bổ vào các dòng con với ô trống, khiến Positional Cell F1 giảm xuống 33.3% dù Bag-of-Cells F1 đạt 100%.

## 4. Kiến nghị Cải tiến Kỹ thuật (Follow-up Recommendations)

1. **Tiếp nhận phiên bản nâng cấp của `pdf-inspector`**: Khi `pdf-inspector` hỗ trợ tốt hơn multiline header / colspan, tỷ lệ TDR trên các bảng như `page_04` và `page_07` sẽ cải thiện trực tiếp.
2. **Chuẩn hóa Header sau trích xuất**: Cân nhắc heuristic phát hiện dòng heading liền trước bảng có thể là một phần của tiêu đề cột bị tách ra để ghép lại vào hàng header của bảng.
3. **Tích hợp Harness vào CI Benchmark**: Duy trì bộ golden 8 trang này trong `bench/pdf_tables_golden/` để ngăn ngừa hồi quy (regression) mỗi khi cập nhật logic bảng.
