//! Declarative request-validation primitives.
//!
//! The `#[validate(...)]` attribute on a `WebxRequestMeta` field emits the
//! checks below after binding; failures land in the same `400` `errors` map as
//! binding failures (one response shape for the client):
//!
//! ```ignore
//! #[derive(WebxRequestMeta)]
//! struct CreateUserRequest {
//!     #[validate(length(min = 3, max = 64))]
//!     name: String,
//!     #[validate(email)]
//!     email: String,
//!     #[validate(range(min = 18, max = 120))]
//!     age: u8,
//!     #[validate(custom = "validate_slug")] // fn(&T) -> Result<(), String>
//!     slug: String,
//! }
//! ```
//!
//! Rules are compiled into the derive output (no runtime reflection): `length`
//! for strings (characters) and collections (items), `range` for numeric
//! scalars, `email`, and `custom` for arbitrary checks. An `Option<T>` field is
//! validated only when present.

/// Characters in `value` (Unicode scalar values, not bytes).
///
/// `length` counts characters for user-facing limits: a 3-character name may be
/// 3 or 9 bytes.
pub fn char_len(value: &str) -> usize {
    value.chars().count()
}

/// Whether `value` looks like a deliverable email address.
///
/// Deliberately a shape check, not RFC 5322: exactly one `@`, a non-empty local
/// part of at most 64 characters, a dotted domain without whitespace, and a
/// total length of at most 254. Deliverability is proven by sending the mail,
/// not by a regex.
pub fn email_is_valid(value: &str) -> bool {
    if value.len() > 254 || value.chars().any(char::is_whitespace) {
        return false;
    }
    let mut parts = value.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    if local.is_empty() || local.chars().count() > 64 {
        return false;
    }
    if local.starts_with('.') || local.ends_with('.') || local.contains("..") {
        return false;
    }
    !domain.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains("..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_length_counts_characters_not_bytes() {
        assert_eq!(char_len("abc"), 3);
        assert_eq!(char_len("订单"), 2);
        assert_eq!(char_len("a订单b"), 4);
        assert_eq!(char_len(""), 0);
    }

    #[test]
    fn email_shape_is_checked_without_regex() {
        for valid in [
            "alice@example.com",
            "a.b+tag@sub.example.co",
            "用户@例子.公司",
        ] {
            assert!(email_is_valid(valid), "{valid} should be valid");
        }
        for invalid in [
            "alice",
            "@example.com",
            "alice@",
            "alice@example",
            "alice@@example.com",
            "alice @example.com",
            "alice@example..com",
            ".alice@example.com",
            "alice@.example.com",
            "alice@example.com.",
        ] {
            assert!(!email_is_valid(invalid), "{invalid} should be invalid");
        }
        let long_local = format!("{}@example.com", "a".repeat(65));
        assert!(!email_is_valid(&long_local));
        let long_total = format!("{}@example.com", "a".repeat(250));
        assert!(!email_is_valid(&long_total));
    }
}
