use fileconv_knowledge::ask::{extractive_answer, valid_citation_ids};
use fileconv_knowledge::citation::{
    extract_snippet, infer_source_anchor, validate_grounded_answer,
};
use fileconv_knowledge::query::normalized_tokens;
use fileconv_knowledge::rank::{cosine_similarity, hybrid_rerank_score, sort_hybrid_hits};
use fileconv_knowledge::types::HybridSearchHit;

#[test]
fn frozen_hybrid_pipeline_keeps_score_anchor_and_grounding() {
    let tokens = normalized_tokens("đối soát giao dịch");
    let body = "Đối soát giao theo ngày";
    let query_vector = [1.0, 0.0];
    let primary_vector = [0.75, (1.0_f32 - 0.75_f32.powi(2)).sqrt()];
    let primary_vector_score = cosine_similarity(&query_vector, &primary_vector);
    let score = hybrid_rerank_score(
        Some(0),
        Some(0),
        primary_vector_score,
        &tokens,
        "Đối soát",
        body,
    );
    let anchor = infer_source_anchor("pdf", "Đối soát", Some(7), 12, 68);
    let secondary_body = "API được bảo vệ bằng xác thực.";
    let mut hits = vec![
        HybridSearchHit {
            chunk_id: "fixture-chunk-security-0001".into(),
            source_rel: "security.docx".into(),
            md_rel: "security.docx.md".into(),
            heading: "Bảo mật".into(),
            snippet: extract_snippet(secondary_body, &tokens),
            lexical_score: 0.5,
            vector_score: 0.5,
            rerank_score: hybrid_rerank_score(
                Some(1),
                Some(1),
                0.5,
                &tokens,
                "Bảo mật",
                secondary_body,
            ),
            anchor: infer_source_anchor("docx", "Bảo mật", None, 0, 34),
        },
        HybridSearchHit {
            chunk_id: "fixture-chunk-payments-0001".into(),
            source_rel: "payments.pdf".into(),
            md_rel: "payments.pdf.md".into(),
            heading: "Đối soát".into(),
            snippet: extract_snippet(body, &tokens),
            lexical_score: 1.25,
            vector_score: primary_vector_score,
            rerank_score: score,
            anchor,
        },
    ];
    sort_hybrid_hits(&mut hits);

    assert_eq!(
        hits.iter()
            .map(|hit| hit.source_rel.as_str())
            .collect::<Vec<_>>(),
        ["payments.pdf", "security.docx"]
    );
    assert!((primary_vector_score - 0.75).abs() < 0.0001);
    assert!((hits[0].rerank_score - 1.875).abs() < 0.0001);
    assert_eq!(hits[0].anchor.page, Some(7));
    assert_eq!((hits[0].anchor.start, hits[0].anchor.end), (12, 68));

    let answer = extractive_answer("Đối soát khi nào?", &hits);
    assert_eq!(
        answer,
        "## Trả lời trích xuất\n\nCâu hỏi: **Đối soát khi nào?**\n\n\
         1. Đối soát giao theo ngày [CITE-0001]\n\n\
         2. API được bảo vệ bằng xác thực. [CITE-0002]\n\n"
    );
    assert!(validate_grounded_answer(&answer, &valid_citation_ids(hits.len())).is_ok());
}
