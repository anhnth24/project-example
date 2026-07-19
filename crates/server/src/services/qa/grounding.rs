//! Citation subset validation for grounded Q&A answers.

use std::collections::BTreeSet;

use fileconv_knowledge::ask::valid_citation_ids;
use fileconv_knowledge::citation::validate_grounded_answer;

pub fn validate(answer: &str, hit_count: usize) -> Result<(), Vec<String>> {
    validate_grounded_answer(answer, &valid_citation_ids(hit_count))
}

pub fn cited_hit_indices(answer: &str, hit_count: usize) -> Vec<usize> {
    cited_ids(answer)
        .into_iter()
        .filter_map(|id| citation_index(&id))
        .filter(|index| *index < hit_count)
        .collect()
}

pub fn cited_ids(answer: &str) -> Vec<String> {
    answer
        .split(|character: char| {
            character.is_whitespace() || matches!(character, '[' | ']' | '(' | ')' | ',' | '.')
        })
        .filter(|part| part.starts_with("CITE-"))
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn citation_index(id: &str) -> Option<usize> {
    let value = id.strip_prefix("CITE-")?.parse::<usize>().ok()?;
    value.checked_sub(1)
}

#[cfg(test)]
mod tests {
    use super::{cited_hit_indices, validate};

    #[test]
    fn validates_citations_as_subset_of_retrieved_ids() {
        assert!(validate("Có căn cứ rõ ràng. [CITE-0001]", 1).is_ok());
        assert!(validate("Có citation ngoài tập retrieval. [CITE-9999]", 1).is_err());
        assert!(validate("Không có citation nào trong câu trả lời đủ dài này.", 1).is_err());
    }

    #[test]
    fn maps_cited_ids_back_to_ordered_hit_indices() {
        let indices = cited_hit_indices("A [CITE-0002], B [CITE-0001].", 3);
        assert_eq!(indices, vec![0, 1]);
        assert!(cited_hit_indices("A [CITE-9999]", 3).is_empty());
    }
}
