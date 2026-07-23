//! Shared Last-Event-ID / lastEventId cursor parser (P1B-R05).
//!
//! Accepts only `0..=i64::MAX`. Rejects malformed, negative, overflowing, and
//! conflicting query/header pairs with a stable 400 validation error.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastEventIdError {
    Malformed,
    Negative,
    OutOfRange,
    Conflicting,
}

impl LastEventIdError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::Malformed => "Last-Event-ID is malformed",
            Self::Negative => "Last-Event-ID must not be negative",
            Self::OutOfRange => "Last-Event-ID exceeds i64::MAX",
            Self::Conflicting => "lastEventId query and Last-Event-ID header conflict",
        }
    }
}

/// Parse a single cursor token into `0..=i64::MAX`.
pub fn parse_last_event_id_token(raw: &str) -> Result<i64, LastEventIdError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(LastEventIdError::Malformed);
    }
    if trimmed.starts_with('-') {
        return Err(LastEventIdError::Negative);
    }
    if !trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return Err(LastEventIdError::Malformed);
    }
    // Reject values that would overflow i64 even if they fit u64.
    match trimmed.parse::<i64>() {
        Ok(value) if value >= 0 => Ok(value),
        Ok(_) => Err(LastEventIdError::Negative),
        Err(_) => Err(LastEventIdError::OutOfRange),
    }
}

/// Merge optional query (`lastEventId`) and header (`Last-Event-ID`) sources.
///
/// - Both absent → `0`
/// - One present → that value
/// - Both present and equal → that value
/// - Both present and unequal → `Conflicting`
/// - Either malformed → corresponding error
/// - Optional `high_water` rejects future cursors (`> high_water`) as out of range
pub fn resolve_last_event_id(
    query: Option<&str>,
    header: Option<&str>,
    high_water: Option<i64>,
) -> Result<i64, LastEventIdError> {
    let parsed_query = match query {
        Some(raw) => Some(parse_last_event_id_token(raw)?),
        None => None,
    };
    let parsed_header = match header {
        Some(raw) => Some(parse_last_event_id_token(raw)?),
        None => None,
    };
    let value = match (parsed_query, parsed_header) {
        (None, None) => 0,
        (Some(value), None) | (None, Some(value)) => value,
        (Some(left), Some(right)) if left == right => left,
        (Some(_), Some(_)) => return Err(LastEventIdError::Conflicting),
    };
    if let Some(hw) = high_water {
        if value > hw {
            return Err(LastEventIdError::OutOfRange);
        }
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_zero_through_i64_max() {
        assert_eq!(parse_last_event_id_token("0").unwrap(), 0);
        assert_eq!(parse_last_event_id_token("42").unwrap(), 42);
        assert_eq!(
            parse_last_event_id_token(&i64::MAX.to_string()).unwrap(),
            i64::MAX
        );
    }

    #[test]
    fn rejects_malformed_negative_overflow_conflict_future() {
        assert_eq!(
            parse_last_event_id_token("-1"),
            Err(LastEventIdError::Negative)
        );
        assert_eq!(
            parse_last_event_id_token("1.5"),
            Err(LastEventIdError::Malformed)
        );
        assert_eq!(
            parse_last_event_id_token("abc"),
            Err(LastEventIdError::Malformed)
        );
        assert_eq!(
            parse_last_event_id_token("9223372036854775808"),
            Err(LastEventIdError::OutOfRange)
        );
        assert_eq!(
            resolve_last_event_id(Some("3"), Some("4"), None),
            Err(LastEventIdError::Conflicting)
        );
        assert_eq!(
            resolve_last_event_id(Some("3"), Some("3"), None).unwrap(),
            3
        );
        assert_eq!(
            resolve_last_event_id(Some("9"), None, Some(5)),
            Err(LastEventIdError::OutOfRange)
        );
        assert_eq!(resolve_last_event_id(None, None, None).unwrap(), 0);
    }
}
