//! Contract test for the stable `problem+json` error-code registry.
//!
//! The registry in `webx_core::error_codes` is only useful if it stays in sync
//! with the runtime functions that produce codes. These tests derive the
//! expected code set from every `Error` variant and from
//! `default_code_for_status`, then require the registry to match exactly — a
//! new code without an entry fails, and a stale entry fails too.

use std::collections::BTreeSet;

use webx_core::error::{default_code_for_status, status_is_retryable, Error};
use webx_core::error_codes::{ErrorCodeSpec, ERROR_CODES, FALLBACK_STATUS};
use webx_core::problem::{FieldError, ProblemDetails};

/// Look up a registered code, failing with the missing code in the message.
fn spec(code: &str) -> &'static ErrorCodeSpec {
    ERROR_CODES
        .iter()
        .find(|spec| spec.code == code)
        .unwrap_or_else(|| panic!("error code `{code}` is not registered in ERROR_CODES"))
}

/// The statuses `default_code_for_status` gives a dedicated code.
const DEFAULT_STATUSES: &[(u16, &str)] = &[
    (400, "bad_request"),
    (401, "unauthorized"),
    (403, "forbidden"),
    (404, "not_found"),
    (405, "method_not_allowed"),
    (409, "conflict"),
    (413, "payload_too_large"),
    (415, "unsupported_media_type"),
    (422, "unprocessable_entity"),
    (429, "too_many_requests"),
    (500, "internal_error"),
    (501, "not_implemented"),
    (502, "bad_gateway"),
    (503, "service_unavailable"),
    (504, "gateway_timeout"),
];

fn serde_error() -> serde_json::Error {
    serde_json::from_str::<serde_json::Value>("{").unwrap_err()
}

/// One representative value for every `Error` variant.
///
/// `assert_variant_covered` below is an exhaustive match over `Error`, so
/// adding a variant fails this test file's build until the variant is listed
/// here; the registry test then fails until its code is registered.
fn representative_errors() -> Vec<Error> {
    let mut errors = vec![
        Error::Http("bad request".into()),
        Error::Unauthorized("missing token".into()),
        Error::Forbidden("policy".into()),
        Error::Di("missing registration".into()),
        Error::Routing("no route".into()),
        Error::Serialization(serde_error()),
        Error::ResponseSerialization(serde_error()),
        Error::State("response already written".into()),
        Error::Internal("boom".into()),
        Error::Message("message".into()),
        Error::Validation("field".into()),
        Error::NotFound("resource".into()),
        Error::Conflict("version".into()),
        Error::PayloadTooLarge("too big".into()),
        Error::UnsupportedMediaType("xml".into()),
        Error::Problem(Box::new(ProblemDetails::new(418, "I'm a teapot"))),
        Error::Problem(Box::new(ProblemDetails::validation(vec![FieldError::new(
            "page",
            "must be positive",
        )]))),
    ];
    // Cover every status with a dedicated fallback code through `Error::Status`.
    for &(status, _) in DEFAULT_STATUSES {
        errors.push(Error::Status(status, "upstream".into()));
    }
    errors
}

/// Compile-time tripwire: the match is exhaustive, so a new `Error` variant
/// breaks the build here until it is covered by `representative_errors`.
fn assert_variant_covered(error: &Error) {
    match error {
        Error::Http(_)
        | Error::Status(_, _)
        | Error::Unauthorized(_)
        | Error::Forbidden(_)
        | Error::Di(_)
        | Error::Routing(_)
        | Error::Serialization(_)
        | Error::ResponseSerialization(_)
        | Error::State(_)
        | Error::Internal(_)
        | Error::Message(_)
        | Error::Validation(_)
        | Error::NotFound(_)
        | Error::Conflict(_)
        | Error::PayloadTooLarge(_)
        | Error::UnsupportedMediaType(_)
        | Error::Problem(_) => {}
    }
    assert!(
        !error.code().is_empty(),
        "every Error variant must carry a stable code"
    );
}

#[test]
fn every_error_variant_code_is_registered() {
    for error in representative_errors() {
        assert_variant_covered(&error);

        let spec = spec(error.code().as_ref());
        if spec.status != FALLBACK_STATUS {
            assert_eq!(
                spec.status,
                error.status_code(),
                "registry status for `{}` must match Error::status_code",
                spec.code
            );
        }
        assert_eq!(
            spec.retryable,
            error.retryable(),
            "registry retryable for `{}` must match Error::retryable",
            spec.code
        );
    }
}

#[test]
fn default_status_codes_are_registered_with_their_status_and_retryability() {
    for &(status, code) in DEFAULT_STATUSES {
        assert_eq!(
            default_code_for_status(status),
            code,
            "unexpected fallback code for status {status}"
        );

        let spec = spec(code);
        assert_eq!(spec.status, status, "wrong status for `{code}`");
        assert_eq!(
            spec.retryable,
            status_is_retryable(status),
            "retryable must mirror status_is_retryable for `{code}`"
        );

        // The runtime `Error::Status` path must agree with the registry.
        let error = Error::Status(status, "upstream".into());
        assert_eq!(error.code().as_ref(), code);
        assert_eq!(error.status_code(), status);
        assert_eq!(error.retryable(), spec.retryable);
    }
}

#[test]
fn unknown_status_falls_back_to_the_registered_error_code() {
    for status in [418u16, 599] {
        assert_eq!(default_code_for_status(status), "error");
        let spec = spec("error");
        assert_eq!(spec.status, FALLBACK_STATUS);
        assert!(
            !spec.retryable,
            "the generic fallback is never retryable (transient statuses have dedicated codes)"
        );

        let error = Error::Status(status, "teapot".into());
        assert_eq!(error.code().as_ref(), "error");
        assert!(!error.retryable());
    }
}

#[test]
fn registry_has_no_duplicates_and_every_entry_is_consistent() {
    let mut seen = BTreeSet::new();
    for spec in ERROR_CODES {
        assert!(!spec.code.is_empty(), "an error code cannot be empty");
        assert!(
            spec.code
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "error code `{}` must be lowercase snake_case",
            spec.code
        );
        assert!(
            seen.insert(spec.code),
            "duplicate error code `{}` in ERROR_CODES",
            spec.code
        );

        if spec.status == FALLBACK_STATUS {
            assert_eq!(
                spec.code, "error",
                "only the generic fallback may use FALLBACK_STATUS"
            );
            assert!(!spec.retryable);
        } else {
            assert!(
                (400..=599).contains(&spec.status),
                "error code `{}` has a non-error HTTP status {}",
                spec.code,
                spec.status
            );
            assert_eq!(
                spec.retryable,
                status_is_retryable(spec.status),
                "retryable for `{}` must mirror status_is_retryable({})",
                spec.code,
                spec.status
            );
        }
    }
}

#[test]
fn registry_covers_exactly_the_runtime_reachable_codes() {
    // Every code the runtime can emit...
    let mut expected: BTreeSet<String> = representative_errors()
        .iter()
        .map(|error| error.code().into_owned())
        .collect();
    for &(status, _) in DEFAULT_STATUSES {
        expected.insert(default_code_for_status(status).to_string());
    }
    expected.insert(default_code_for_status(418).to_string());

    // ...must be registered, and the registry must not grow stale entries.
    let registered: BTreeSet<&str> = ERROR_CODES.iter().map(|spec| spec.code).collect();
    let expected: BTreeSet<&str> = expected.iter().map(String::as_str).collect();
    assert_eq!(
        expected, registered,
        "ERROR_CODES must contain exactly the codes the framework emits"
    );
}

#[test]
fn application_defined_problem_codes_are_not_framework_entries() {
    // Applications may override `code` for their own contract; those codes are
    // not part of ERROR_CODES (the registry documents only what the framework
    // itself emits).
    let error = ProblemDetails::new(429, "Too Many Requests")
        .with_code("rate_limited")
        .with_retryable(true)
        .to_error();
    assert_eq!(error.code().as_ref(), "rate_limited");
    assert!(
        ERROR_CODES.iter().all(|spec| spec.code != "rate_limited"),
        "an application-defined code must not be registered as a framework code"
    );
}
