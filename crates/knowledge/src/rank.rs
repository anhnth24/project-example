use std::cmp::Ordering;
use std::collections::HashSet;

use crate::query::normalized_tokens;
use crate::types::HybridSearchHit;

pub const RRF_K: f32 = 60.0;
pub const RRF_RERANK_SCALE: f32 = 30.0;
pub const VECTOR_WEIGHT: f32 = 0.55;
pub const BODY_OVERLAP_WEIGHT: f32 = 0.35;
pub const HEADING_HIT_WEIGHT: f32 = 0.1;

#[derive(Debug, Clone, PartialEq)]
pub struct RankedCandidate {
    pub id: String,
    pub score: f32,
}

pub fn stable_score_order(candidates: &mut [RankedCandidate]) {
    candidates.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
}

/// Dot product for vectors already normalized by the embedding layer.
///
/// Length mismatch and empty vectors retain the desktop fallback score of zero.
pub fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

pub fn reciprocal_rank_fusion(lexical_rank: Option<usize>, vector_rank: Option<usize>) -> f32 {
    lexical_rank
        .into_iter()
        .chain(vector_rank)
        .map(|rank| 1.0 / (RRF_K + rank as f32))
        .sum()
}

pub fn heading_token_hits(query_tokens: &[String], heading: &str) -> f32 {
    let normalized = fileconv_core::intelligence::normalize_search_text(heading);
    query_tokens
        .iter()
        .filter(|token| normalized.contains(*token))
        .count() as f32
}

pub fn body_token_overlap(query_tokens: &[String], body: &str) -> f32 {
    let body_tokens: HashSet<String> = normalized_tokens(body).into_iter().collect();
    query_tokens
        .iter()
        .filter(|token| body_tokens.contains(*token))
        .count() as f32
        / query_tokens.len().max(1) as f32
}

pub fn hybrid_rerank_score(
    lexical_rank: Option<usize>,
    vector_rank: Option<usize>,
    vector_score: f32,
    query_tokens: &[String],
    heading: &str,
    body: &str,
) -> f32 {
    reciprocal_rank_fusion(lexical_rank, vector_rank) * RRF_RERANK_SCALE
        + vector_score.max(0.0) * VECTOR_WEIGHT
        + body_token_overlap(query_tokens, body) * BODY_OVERLAP_WEIGHT
        + heading_token_hits(query_tokens, heading) * HEADING_HIT_WEIGHT
}

/// Preserve the frozen desktop ordering: score descending, then `chunk_id` ascending
/// as a deterministic tie-break (also covers NaN, which `partial_cmp` treats as equal
/// to everything). The tie-break only ever fires when the primary comparison is
/// `Equal`/`None`, so distinct scores keep their original relative order.
pub fn sort_hybrid_hits(hits: &mut [HybridSearchHit]) {
    hits.sort_by(|left, right| {
        right
            .rerank_score
            .partial_cmp(&left.rerank_score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.chunk_id.cmp(&right.chunk_id))
    });
}

#[cfg(test)]
mod tests {
    use super::{
        body_token_overlap, cosine_similarity, hybrid_rerank_score, reciprocal_rank_fusion,
        sort_hybrid_hits, stable_score_order, RankedCandidate,
    };
    use crate::types::{HybridSearchHit, SourceAnchor};

    fn hit(id: &str, score: f32) -> HybridSearchHit {
        HybridSearchHit {
            chunk_id: id.into(),
            source_rel: format!("{id}.pdf"),
            md_rel: format!("{id}.pdf.md"),
            heading: String::new(),
            snippet: String::new(),
            lexical_score: 0.0,
            vector_score: 0.0,
            rerank_score: score,
            anchor: SourceAnchor {
                page: None,
                slide: None,
                sheet: None,
                start: 0,
                end: 0,
            },
        }
    }

    #[test]
    fn ties_use_stable_identifier_order() {
        let mut candidates = vec![
            RankedCandidate {
                id: "b".into(),
                score: 1.0,
            },
            RankedCandidate {
                id: "a".into(),
                score: 1.0,
            },
        ];
        stable_score_order(&mut candidates);
        assert_eq!(candidates[0].id, "a");
    }

    #[test]
    fn cosine_keeps_desktop_mismatch_fallback() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine_similarity(&[0.6, 0.8], &[0.6, 0.8]), 1.0);
    }

    #[test]
    fn rrf_and_rerank_match_frozen_golden_score() {
        let tokens = vec!["doi".into(), "soat".into(), "giao".into(), "dich".into()];
        let score = hybrid_rerank_score(
            Some(0),
            Some(0),
            0.75,
            &tokens,
            "Đối soát",
            "Đối soát giao theo ngày",
        );
        assert!((reciprocal_rank_fusion(Some(0), Some(0)) - 2.0 / 60.0).abs() < f32::EPSILON);
        assert!((body_token_overlap(&tokens, "Đối soát giao theo ngày") - 0.75).abs() < 0.0001);
        assert!((score - 1.875).abs() < 0.0001);
    }

    #[test]
    fn negative_vector_score_and_empty_tokens_are_safe() {
        assert_eq!(body_token_overlap(&[], "nội dung"), 0.0);
        let score = hybrid_rerank_score(None, Some(0), -1.0, &[], "", "");
        assert!((score - 0.5).abs() < 0.0001);
    }

    #[test]
    fn hybrid_hit_sort_breaks_ties_by_chunk_id() {
        let mut hits = vec![
            hit("low", 0.5),
            hit("tie-b", 1.0),
            hit("tie-a", 1.0),
            hit("nan", f32::NAN),
        ];
        sort_hybrid_hits(&mut hits);
        let order: Vec<_> = hits
            .iter()
            .map(|candidate| candidate.chunk_id.as_str())
            .collect();
        // Equal (and NaN) scores are broken deterministically by ascending chunk_id
        // instead of depending on input/insertion order.
        assert_eq!(order, ["tie-a", "tie-b", "low", "nan"]);
    }

    #[test]
    fn hybrid_hit_sort_is_independent_of_insertion_order() {
        let scored = [
            ("z-tie", 2.0f32),
            ("a-tie", 2.0f32),
            ("m-tie", 2.0f32),
            ("solo-high", 3.0f32),
            ("solo-low", 1.0f32),
        ];
        let base: Vec<HybridSearchHit> = scored.iter().map(|(id, score)| hit(id, *score)).collect();

        // Same candidates, several different insertion/collection orders (as if they
        // came out of randomized HashSet iteration upstream).
        let orderings: [&[usize]; 3] = [&[0, 1, 2, 3, 4], &[4, 3, 2, 1, 0], &[2, 0, 4, 1, 3]];

        let mut first_result: Option<Vec<String>> = None;
        for order in orderings {
            let mut hits: Vec<HybridSearchHit> =
                order.iter().map(|&index| base[index].clone()).collect();
            sort_hybrid_hits(&mut hits);
            // Also verify determinism exactly at a truncation/limit boundary, where a
            // non-deterministic tie-break would visibly change which hit survives.
            hits.truncate(3);
            let ids: Vec<String> = hits.iter().map(|hit| hit.chunk_id.clone()).collect();
            match &first_result {
                None => first_result = Some(ids),
                Some(expected) => assert_eq!(&ids, expected),
            }
        }
        assert_eq!(
            first_result.unwrap(),
            vec!["solo-high", "a-tie", "m-tie"],
            "truncation boundary must keep the lexicographically-smallest tied ids"
        );
    }
}
