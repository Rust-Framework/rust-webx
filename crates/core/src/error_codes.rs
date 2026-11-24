//! Stable error codes for `problem+json` responses.
//!
//! Every framework-generated problem carries a machine-readable `code`
//! extension member (an RFC 9457 extension) in addition to the HTTP status:
//! [`Error::code`](crate::error::Error::code) derives it from the error variant,
//! [`default_code_for_status`](crate::error::default_code_for_status) supplies
//! the fallback for a bare status, and an application may override it with
//! [`ProblemDetails::with_code`](crate::problem::ProblemDetails::with_code).
//! Clients branch on `code`, never on the human-readable `detail`.
//!
//! [`ERROR_CODES`] is the registry of the codes the framework itself emits. It
//! exists so the contract can be documented, generated and tested:
//! `crates/core/tests/error_codes_test.rs` asserts that every code reachable
//! through the runtime functions above appears here exactly once with the same
//! status/retryability, so a new code cannot be added to
//! [`Error::code`](crate::error::Error::code) or
//! [`default_code_for_status`](crate::error::default_code_for_status) without
//! registering it.
//!
//! Rules for new entries (see `docs/design/error-codes.md`):
//!
//! * lowercase `snake_case`; stable once released — 2.x adds entries, never
//!   renames or removes them;
//! * one HTTP status per code, and `retryable` must match
//!   [`status_is_retryable`](crate::error::status_is_retryable) for that
//!   status, except for a documented condition-specific code
//!   (`conflict_retryable`: a `409` whose conflict is transient);
//! * application-defined codes (carried through
//!   [`ProblemDetails`](crate::problem::ProblemDetails)) are **not** registered
//!   here: they belong to the application's own contract.

/// One entry of the stable error-code registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorCodeSpec {
    /// The `code` extension value carried by `application/problem+json`.
    pub code: &'static str,
    /// The HTTP status the code normally implies.
    ///
    /// [`FALLBACK_STATUS`] for the generic `error` fallback, whose actual status
    /// is carried by the error itself
    /// ([`Error::Status`](crate::error::Error::Status) or the
    /// [`ProblemDetails`](crate::problem::ProblemDetails) status).
    pub status: u16,
    /// Whether retrying the same request may succeed.
    ///
    /// `true` for transient conditions: every `429`/`502`/`503`/`504` status,
    /// plus [`conflict_retryable`](crate::error::Error::conflict_retryable) —
    /// a `409` that means "wait for the conflicting work, then retry". A
    /// validation failure, a permanent state conflict or a server-side defect is
    /// never retryable.
    pub retryable: bool,
}

/// `status` sentinel for [`ErrorCodeSpec`] entries whose status is carried by
/// the error instead of being fixed by the code.
pub const FALLBACK_STATUS: u16 = 0;

/// Every stable `problem+json` code emitted by the framework.
///
/// Ordered by HTTP status, then by code, for readable diffs and docs. Look a
/// code up with `ERROR_CODES` or a plain `find`; the list is a `const`
/// table, not a runtime map.
///
/// ```
/// use webx_core::error_codes::{ErrorCodeSpec, ERROR_CODES};
///
/// let spec = ERROR_CODES
///     .iter()
///     .find(|spec| spec.code == "service_unavailable")
///     .expect("registered");
/// assert_eq!(
///     *spec,
///     ErrorCodeSpec {
///         code: "service_unavailable",
///         status: 503,
///         retryable: true,
///     }
/// );
/// ```
pub const ERROR_CODES: &[ErrorCodeSpec] = &[
    // --- 4xx: client errors -------------------------------------------------
    ErrorCodeSpec {
        code: "bad_request",
        status: 400,
        retryable: false,
    },
    ErrorCodeSpec {
        code: "invalid_json",
        status: 400,
        retryable: false,
    },
    ErrorCodeSpec {
        code: "validation_failed",
        status: 400,
        retryable: false,
    },
    ErrorCodeSpec {
        code: "unauthorized",
        status: 401,
        retryable: false,
    },
    ErrorCodeSpec {
        code: "forbidden",
        status: 403,
        retryable: false,
    },
    ErrorCodeSpec {
        code: "not_found",
        status: 404,
        retryable: false,
    },
    ErrorCodeSpec {
        code: "route_not_found",
        status: 404,
        retryable: false,
    },
    ErrorCodeSpec {
        code: "method_not_allowed",
        status: 405,
        retryable: false,
    },
    ErrorCodeSpec {
        code: "conflict",
        status: 409,
        retryable: false,
    },
    ErrorCodeSpec {
        code: "conflict_retryable",
        status: 409,
        retryable: true,
    },
    ErrorCodeSpec {
        code: "payload_too_large",
        status: 413,
        retryable: false,
    },
    ErrorCodeSpec {
        code: "unsupported_media_type",
        status: 415,
        retryable: false,
    },
    ErrorCodeSpec {
        code: "unprocessable_entity",
        status: 422,
        retryable: false,
    },
    ErrorCodeSpec {
        code: "too_many_requests",
        status: 429,
        retryable: true,
    },
    // --- 5xx: server errors -------------------------------------------------
    ErrorCodeSpec {
        code: "dependency_injection",
        status: 500,
        retryable: false,
    },
    ErrorCodeSpec {
        code: "internal_error",
        status: 500,
        retryable: false,
    },
    ErrorCodeSpec {
        code: "invalid_state",
        status: 500,
        retryable: false,
    },
    ErrorCodeSpec {
        code: "response_serialization",
        status: 500,
        retryable: false,
    },
    ErrorCodeSpec {
        code: "not_implemented",
        status: 501,
        retryable: false,
    },
    ErrorCodeSpec {
        code: "bad_gateway",
        status: 502,
        retryable: true,
    },
    ErrorCodeSpec {
        code: "service_unavailable",
        status: 503,
        retryable: true,
    },
    ErrorCodeSpec {
        code: "gateway_timeout",
        status: 504,
        retryable: true,
    },
    // --- Fallback -----------------------------------------------------------
    ErrorCodeSpec {
        code: "error",
        status: FALLBACK_STATUS,
        retryable: false,
    },
];
