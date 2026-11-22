//! Stripe wire helpers: form encoding and timestamps.

use std::time::{SystemTime, UNIX_EPOCH};

/// Encode `pairs` as `application/x-www-form-urlencoded`.
///
/// Stripe's v1 API is form-encoded; nested parameters use bracket keys
/// (`metadata[order_id]`), which this encoder percent-encodes exactly like the
/// official SDKs.
pub fn encode_form_pairs(pairs: &[(String, String)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.extend_pairs(pairs);
    serializer.finish()
}

/// Percent-encode a value for use inside a query string.
pub fn encode_query_value(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// Current Unix time in seconds.
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_encoding_matches_stripe_bracket_syntax() {
        let encoded = encode_form_pairs(&[
            ("amount".to_string(), "9900".to_string()),
            ("metadata[order_id]".to_string(), "A B/C".to_string()),
        ]);
        assert_eq!(encoded, "amount=9900&metadata%5Border_id%5D=A+B%2FC");
    }

    #[test]
    fn query_encoding_escapes_quotes_and_spaces() {
        assert_eq!(
            encode_query_value("metadata['order_id']:'A 1'"),
            "metadata%5B%27order_id%27%5D%3A%27A+1%27"
        );
    }
}
