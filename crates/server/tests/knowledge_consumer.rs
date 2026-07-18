use std::collections::HashSet;

use fileconv_knowledge::ask::{extractive_answer, valid_citation_ids};
use fileconv_knowledge::citation::{infer_source_anchor, validate_grounded_answer};
use fileconv_knowledge::query::PreparedQuery;
use fileconv_knowledge::rank::hybrid_rerank_score;
use fileconv_knowledge::types::{HybridSearchHit, SourceAnchor};

#[test]
fn server_consumes_default_knowledge_api_without_desktop_features() {
    let query = PreparedQuery::new("đối soát giao dịch");
    let score = hybrid_rerank_score(
        Some(0),
        Some(0),
        0.75,
        &query.tokens,
        "Đối soát",
        "Đối soát giao theo ngày",
    );
    let anchor = infer_source_anchor("pdf", "Đối soát", Some(7), 12, 68);
    let hits = vec![HybridSearchHit {
        chunk_id: "server-chunk-1".into(),
        source_rel: "payments.pdf".into(),
        md_rel: "payments.pdf.md".into(),
        heading: "Đối soát".into(),
        snippet: "Đối soát giao theo ngày".into(),
        lexical_score: 1.25,
        vector_score: 0.75,
        rerank_score: score,
        anchor,
    }];
    let answer = extractive_answer("Đối soát khi nào?", &hits);

    assert!((score - 1.875).abs() < 0.0001);
    assert_eq!(
        hits[0].anchor,
        SourceAnchor {
            page: Some(7),
            slide: None,
            sheet: None,
            start: 12,
            end: 68,
        }
    );
    assert!(validate_grounded_answer(&answer, &valid_citation_ids(1)).is_ok());
    assert_eq!(
        valid_citation_ids(1),
        HashSet::from(["CITE-0001".to_string()])
    );
}
