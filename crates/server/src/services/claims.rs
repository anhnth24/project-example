//! Conservative extraction of explicitly structured, typed Markdown claims.
//!
//! The indexer only accepts a table whose headings name each semantic field.
//! Free-form prose is intentionally not guessed: an incorrect conflict warning
//! is worse than omitting an unstructured claim.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub enum ClaimValue {
    Number(Decimal),
    Money(Decimal),
    Boolean(bool),
    Date(NaiveDate),
    Enum(String),
    Text(String),
}

impl ClaimValue {
    pub const fn value_type(&self) -> &'static str {
        match self {
            Self::Number(_) => "number",
            Self::Money(_) => "money",
            Self::Boolean(_) => "boolean",
            Self::Date(_) => "date",
            Self::Enum(_) => "enum",
            Self::Text(_) => "text",
        }
    }

    fn canonical(&self) -> String {
        match self {
            Self::Number(value) | Self::Money(value) => value.normalize().to_string(),
            Self::Boolean(value) => value.to_string(),
            Self::Date(value) => value.to_string(),
            Self::Enum(value) | Self::Text(value) => value.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedClaim {
    pub id: Uuid,
    pub claim_key: String,
    pub subject: String,
    pub predicate: String,
    pub value: ClaimValue,
    pub unit: Option<String>,
    pub scope: String,
    pub citation_quote: String,
    pub citation_span_start: i32,
    pub citation_span_end: i32,
}

/// Extracts rows from Markdown tables with the required columns:
/// `claim_key`, `subject`, `predicate`, and `value`.
///
/// Optional `value_type`/`type`, `unit`, and `scope` columns preserve the
/// remaining typed-claim dimensions. Invalid rows are skipped rather than
/// guessed, keeping conflict candidates deterministic and high-confidence.
pub fn extract_typed_claims(
    markdown: &str,
    version_id: Uuid,
    source_identity: &str,
) -> Vec<ExtractedClaim> {
    let lines = numbered_lines(markdown);
    let mut claims = Vec::new();

    for index in 0..lines.len().saturating_sub(1) {
        let (header_offset, header) = lines[index];
        let (_, separator) = lines[index + 1];
        let Some(headers) = parse_table_row(header) else {
            continue;
        };
        if !is_table_separator(separator) {
            continue;
        }
        let Some(columns) = ClaimColumns::from_headers(&headers) else {
            continue;
        };

        for &(row_offset, row) in lines.iter().skip(index + 2) {
            let Some(values) = parse_table_row(row) else {
                break;
            };
            if values.len() != headers.len() {
                break;
            }
            let Some(mut claim) = parse_claim_row(&columns, &values) else {
                continue;
            };
            let row_end = row_offset.saturating_add(row.len());
            claim.citation_quote = row.trim().to_string();
            claim.citation_span_start = bounded_i32(row_offset);
            claim.citation_span_end = bounded_i32(row_end);
            claim.id = deterministic_claim_id(version_id, source_identity, claims.len(), &claim);
            claims.push(claim);
        }

        // No later line in this table can be another header. Advancing by the
        // table width is unnecessary for correctness and keeps parsing simple.
        let _ = header_offset;
    }

    claims
}

#[derive(Debug, Clone, Copy)]
struct ClaimColumns {
    claim_key: usize,
    subject: usize,
    predicate: usize,
    value: usize,
    value_type: Option<usize>,
    unit: Option<usize>,
    scope: Option<usize>,
}

impl ClaimColumns {
    fn from_headers(headers: &[String]) -> Option<Self> {
        let position = |names: &[&str]| {
            headers.iter().position(|header| {
                let normalized = normalize_header(header);
                names.contains(&normalized.as_str())
            })
        };
        Some(Self {
            claim_key: position(&["claim_key", "key"])?,
            subject: position(&["subject"])?,
            predicate: position(&["predicate"])?,
            value: position(&["value"])?,
            value_type: position(&["value_type", "type"]),
            unit: position(&["unit"]),
            scope: position(&["scope"]),
        })
    }
}

fn parse_claim_row(columns: &ClaimColumns, values: &[String]) -> Option<ExtractedClaim> {
    let get = |index: usize| values.get(index).map(String::as_str).map(str::trim);
    let claim_key = get(columns.claim_key)?.to_string();
    let subject = get(columns.subject)?.to_string();
    let predicate = get(columns.predicate)?.to_string();
    let raw_value = get(columns.value)?;
    if claim_key.is_empty() || subject.is_empty() || predicate.is_empty() || raw_value.is_empty() {
        return None;
    }
    let value_type = columns
        .value_type
        .and_then(get)
        .filter(|value| !value.is_empty());
    let unit = columns
        .unit
        .and_then(get)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let scope = columns
        .scope
        .and_then(get)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string();
    let value = parse_value(raw_value, value_type, unit.as_deref())?;

    Some(ExtractedClaim {
        id: Uuid::nil(),
        claim_key,
        subject,
        predicate,
        value,
        unit,
        scope,
        citation_quote: String::new(),
        citation_span_start: 0,
        citation_span_end: 0,
    })
}

fn parse_value(raw: &str, explicit_type: Option<&str>, unit: Option<&str>) -> Option<ClaimValue> {
    let kind = explicit_type.map(|value| value.trim().to_ascii_lowercase());
    match kind.as_deref() {
        Some("number") => parse_decimal(raw).map(ClaimValue::Number),
        Some("money") => parse_decimal(raw).map(ClaimValue::Money),
        Some("boolean") => parse_boolean(raw).map(ClaimValue::Boolean),
        Some("date") => NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d")
            .ok()
            .map(ClaimValue::Date),
        Some("enum") => Some(ClaimValue::Enum(raw.trim().to_string())),
        Some("text") => Some(ClaimValue::Text(raw.trim().to_string())),
        Some(_) => None,
        None => infer_value(raw, unit),
    }
}

fn infer_value(raw: &str, unit: Option<&str>) -> Option<ClaimValue> {
    if let Some(value) = parse_boolean(raw) {
        return Some(ClaimValue::Boolean(value));
    }
    if let Ok(value) = NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d") {
        return Some(ClaimValue::Date(value));
    }
    if let Some(value) = parse_decimal(raw) {
        return Some(if is_money_unit(unit) {
            ClaimValue::Money(value)
        } else {
            ClaimValue::Number(value)
        });
    }
    Some(ClaimValue::Text(raw.trim().to_string()))
}

fn parse_boolean(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "có" => Some(true),
        "false" | "no" | "không" => Some(false),
        _ => None,
    }
}

fn parse_decimal(raw: &str) -> Option<Decimal> {
    let compact = raw
        .trim()
        .chars()
        .filter(|character| *character != '_' && !character.is_whitespace())
        .collect::<String>();
    if compact.is_empty() {
        return None;
    }
    let comma_count = compact.matches(',').count();
    let dot_count = compact.matches('.').count();
    let decimal_separator = match (comma_count, dot_count) {
        (0, 0) => None,
        // A single separator followed by exactly three digits has no reliable
        // locale: `1.000` can mean one thousand or one with three decimal
        // places. Claims must fail closed rather than changing their meaning.
        (1, 0) if !is_ambiguous_single_separator(&compact, ',') => compact.rfind(','),
        (1, 0) => return None,
        (count, 0) if count > 1 && !separators_are_three_digit_groups(&compact, ',') => {
            compact.rfind(',')
        }
        (count, 0) if count > 1 => None,
        (0, 1) if !is_ambiguous_single_separator(&compact, '.') => compact.rfind('.'),
        (0, 1) => return None,
        (0, count) if count > 1 && !separators_are_three_digit_groups(&compact, '.') => {
            compact.rfind('.')
        }
        (0, count) if count > 1 => None,
        (_, _) => compact
            .rfind(',')
            .into_iter()
            .chain(compact.rfind('.'))
            .max(),
    };
    let normalized = compact
        .char_indices()
        .filter_map(|(index, character)| match character {
            ',' | '.' if Some(index) == decimal_separator => Some('.'),
            ',' | '.' => None,
            _ => Some(character),
        })
        .collect::<String>();
    normalized.parse().ok()
}

fn is_ambiguous_single_separator(value: &str, separator: char) -> bool {
    let Some((whole, fractional)) = value.split_once(separator) else {
        return false;
    };
    let whole = whole.trim_start_matches(['+', '-']);
    !whole.is_empty()
        && whole.chars().all(|character| character.is_ascii_digit())
        && fractional.len() == 3
        && fractional
            .chars()
            .all(|character| character.is_ascii_digit())
}

fn separators_are_three_digit_groups(value: &str, separator: char) -> bool {
    let mut groups = value.split(separator);
    let Some(first) = groups.next() else {
        return false;
    };
    let first = first.trim_start_matches(['+', '-']);
    !first.is_empty()
        && first.chars().all(|character| character.is_ascii_digit())
        && groups.all(|group| {
            group.len() == 3 && group.chars().all(|character| character.is_ascii_digit())
        })
}

fn is_money_unit(unit: Option<&str>) -> bool {
    matches!(
        unit.map(|value| value.trim().to_ascii_uppercase()),
        Some(value) if matches!(value.as_str(), "VND" | "USD" | "EUR" | "GBP" | "₫")
    )
}

fn numbered_lines(markdown: &str) -> Vec<(usize, &str)> {
    let mut offset: usize = 0;
    markdown
        .split_inclusive('\n')
        .map(|line| {
            let start = offset;
            offset = offset.saturating_add(line.len());
            (start, line.trim_end_matches(['\r', '\n']))
        })
        .collect()
}

fn parse_table_row(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if !trimmed.starts_with('|') || !trimmed.ends_with('|') {
        return None;
    }
    let values = trimmed[1..trimmed.len() - 1]
        .split('|')
        .map(|value| value.trim().to_string())
        .collect::<Vec<_>>();
    (!values.is_empty()).then_some(values)
}

fn is_table_separator(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|')
        && trimmed.ends_with('|')
        && trimmed.contains('-')
        && trimmed
            .chars()
            .all(|character| matches!(character, '|' | '-' | ':' | ' ' | '\t'))
}

fn normalize_header(header: &str) -> String {
    let mut normalized = String::new();
    let mut previous_separator = false;
    for character in header.trim().chars() {
        if character.is_ascii_alphanumeric() {
            normalized.push(character.to_ascii_lowercase());
            previous_separator = false;
        } else if !previous_separator {
            normalized.push('_');
            previous_separator = true;
        }
    }
    normalized.trim_matches('_').to_string()
}

fn deterministic_claim_id(
    version_id: Uuid,
    source_identity: &str,
    ordinal: usize,
    claim: &ExtractedClaim,
) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(b"markhand-claim-v1");
    for field in [
        version_id.to_string(),
        source_identity.to_string(),
        ordinal.to_string(),
        claim.claim_key.clone(),
        claim.subject.clone(),
        claim.predicate.clone(),
        claim.value.value_type().to_string(),
        claim.value.canonical(),
        claim.unit.clone().unwrap_or_default(),
        claim.scope.clone(),
    ] {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field.as_bytes());
    }
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::new_v8(bytes)
}

fn bounded_i32(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "\
| claim_key | subject | predicate | value | value_type | unit | scope |
| --- | --- | --- | --- | --- | --- | --- |
| budget | API | monthly_limit | 1000000 | money | VND | production |
| enabled | API | required | true | boolean | | production |";

    #[test]
    fn extracts_typed_claims_with_deterministic_identity() {
        let version = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
        let first = extract_typed_claims(TABLE, version, "chunk-a");
        let second = extract_typed_claims(TABLE, version, "chunk-a");

        assert_eq!(first, second);
        assert_eq!(first.len(), 2);
        assert_eq!(
            first[0].value,
            ClaimValue::Money(Decimal::new(1_000_000, 0))
        );
        assert_eq!(first[0].unit.as_deref(), Some("VND"));
        assert_eq!(first[0].scope, "production");
        assert_eq!(first[1].value, ClaimValue::Boolean(true));
        assert_ne!(first[0].id, first[1].id);
    }

    #[test]
    fn ignores_unstructured_or_invalid_claim_rows() {
        let markdown = "\
budget is one million

| claim_key | subject | predicate | value | value_type |
| --- | --- | --- | --- | --- |
| budget | | monthly_limit | 1000000 | money |";
        assert!(extract_typed_claims(markdown, Uuid::new_v4(), "chunk-a").is_empty());
    }

    #[test]
    fn parses_vietnamese_decimal_commas_without_losing_the_fraction() {
        assert_eq!(parse_decimal("1,5"), Some(Decimal::new(15, 1)));
        assert_eq!(parse_decimal("1.234,56"), Some(Decimal::new(123_456, 2)));
        assert_eq!(parse_decimal("1,000,000"), Some(Decimal::new(1_000_000, 0)));
    }

    #[test]
    fn rejects_single_three_digit_separator_with_ambiguous_locale() {
        assert_eq!(parse_decimal("1.000"), None);
        assert_eq!(parse_decimal("1,000"), None);
        assert_eq!(parse_decimal("1.00"), Some(Decimal::new(100, 2)));
    }
}
