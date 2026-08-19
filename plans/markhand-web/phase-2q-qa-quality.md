# Phase 2Q — Nâng cấp chất lượng Q&A

Workstream chất lượng trả lời (retrieval, grounding, version intelligence,
chunking, vận hành chất lượng) tách từ plan đã red-team
[`../260817-qa-quality-upgrade.md`](../260817-qa-quality-upgrade.md). Phase này
chạy song song với đuôi Phase 2 — không phụ thuộc Phase 2 completion gate vì
không đổi surface auth/ACL; từng issue tự khai dependency riêng.

## Mục tiêu

Nâng chất lượng Q&A đo được bằng hai bộ eval (đa định dạng + version-aware)
mà không phá các bất biến đã có: citation phải có span byte-exact vào canonical
markdown (`verify_dual_spans`), grounding fail-closed, index signature pin.

## Baseline và nguồn

- Baseline sau P0.0/P0.0b/P0.2 (đã giao 2026-08-18, trước khi lập phase):
  eval đa định dạng v2 **92.9**, eval version **89.1**. Chi tiết đo, khoảng
  trống G0–G8 và thiết kế từng hạng mục: xem plan nguồn ở trên — plan nguồn là
  tài liệu thiết kế, catalog issue của phase này là authority về trạng thái.

## Gate của phase

- Mỗi PR: eval quyết định giữ/loại chạy **≥2 lần lấy median**; median không
  giảm quá 1 điểm so baseline gần nhất, hạng mục nhắm tới phải tăng.
- Sau mỗi lần đổi index generation (P2Q-01, P2Q-08 khi áp lên server) phải
  **re-baseline** cả hai eval trước khi đánh giá PR kế tiếp.
- Hạng mục đụng grounding/LLM policy/migration/native binary đi qua review
  bắt buộc theo `.cursor/rules/repository-delivery.mdc`.
- Warning mới phải kèm entry `WARNING_TRANSLATIONS` + test web.

## Ngoài phạm vi phase

Cross-encoder rerank, GraphRAG/knowledge graph, question decomposition đa hop
(lý do loại: xem plan nguồn mục 1); sửa chunk tại chỗ (immutability — chỉ
read-only viewer trong phase này).

Issue catalog: [`backlog/phase-2q/issues/README.md`](backlog/phase-2q/issues/README.md)
