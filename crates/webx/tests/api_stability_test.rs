//! Public-API stability smoke test.
//!
//! This is not a semver checker; it pins the *documented* 2.x surface that
//! downstream code actually uses (struct literals, enum variants, constructors,
//! builder methods). An accidental layout/rename change fails here before it
//! reaches users. `cargo-semver-checks` (CI, `2.2-8b`) covers the rest.

use webx::*;

#[test]
fn problem_details_layout_is_stable() {
    // Constructed as a literal on purpose: adding/removing a public field or
    // renaming one would break downstream struct literals.
    let problem = ProblemDetails {
        typ: None,
        title: "Bad Request".to_string(),
        status: 400,
        detail: None,
        instance: None,
        extensions: None,
    };
    assert_eq!(problem.status, 400);
    assert!(problem.code().is_none());
}

#[test]
fn error_variants_and_accessors_are_stable() {
    let errors = [
        Error::validation("x"),
        Error::not_found("x"),
        Error::conflict("x"),
        Error::unauthorized("x"),
        Error::forbidden("x"),
        Error::internal("x"),
        Error::status(503, "down"),
    ];
    for error in errors {
        let _ = error.status_code();
        let _ = error.public_detail();
        let _ = error.code();
        let _ = error.retryable();
    }
}

#[test]
fn documented_constructors_still_exist() {
    let _ = ProblemDetails::new(404, "Not Found");
    let _ = ProblemDetails::not_found("user", "abc");
    let _ = ProblemDetails::validation(vec![FieldError::new("field", "message")])
        .with_code("custom")
        .with_retryable(true)
        .with_detail("detail")
        .with_instance("/x");

    // Pagination contract (2.9.1: plain fields; binding owns defaults/validation).
    let paging = PagedRequest {
        page: 2,
        page_size: 50,
    };
    assert_eq!(paging.page, 2);
    assert_eq!(paging.page_size, 50);
    assert_eq!(paging.offset(), 50);

    // Rate limiting compatibility wrapper (kept by 2.2).
    let limiter = RateLimiter::new(10.0, 20, 1_000);
    assert_eq!(limiter.retry_after_secs(), 1);

    // In-memory limiter + port.
    let store = InMemoryRateLimiter::new(10.0, 20, 1_000);
    let _: &dyn IRateLimiter = &store;

    // 2.9.1 typed errors: rate limiting carries its `Retry-After` value, and a
    // waitable conflict is distinct from the permanent `Error::conflict`.
    assert_eq!(Error::too_many_requests(5, "slow down").status_code(), 429);
    assert_eq!(
        Error::conflict_retryable("in flight").code(),
        "conflict_retryable"
    );

    // 2.9.1 email fields: the documented construction path is `new` + builders.
    let email = EmailMessage::new("a@example.com", "subject", "text")
        .with_from("billing@example.com")
        .with_reply_to("support@example.com")
        .with_header("X-Test", "value");
    assert!(email.validate().is_ok());

    // Idempotency (2.8): the value types are `#[non_exhaustive]`, so these
    // constructors are the documented construction path for custom stores.
    let response = IdempotentResponse::new(
        201,
        Some("application/json".to_string()),
        None,
        b"{}".to_vec(),
    )
    .with_headers(vec![("etag".to_string(), "\"v1\"".to_string())]);
    assert_eq!(response.status, 201);
    assert_eq!(response.headers.len(), 1);
    assert_eq!(IdempotencyLease::new("token").token, "token");
    let started = IdempotencyBegin::Started;
    assert_eq!(started, IdempotencyBegin::Started);
}
