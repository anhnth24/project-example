use std::cmp::Ordering;

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

#[cfg(test)]
mod tests {
    use super::{stable_score_order, RankedCandidate};

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
}
