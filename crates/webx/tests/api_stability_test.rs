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
        Error::Validation("x".into()),
        Error::NotFound("x".into()),
        Error::Conflict("x".into()),
        Error::Unauthorized("x".into()),
        Error::Forbidden("x".into()),
        Error::Internal("x".into()),
        Error::Status(503, "down".into()),
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

    // Pagination (2.0 contract).
    let paging = PagedRequest::from_parts(Some(2), Some(50));
    assert_eq!(paging.page, 2);
    assert_eq!(paging.page_size, 50);

    // Rate limiting compatibility wrapper (kept by 2.2).
    let limiter = RateLimiter::new(10.0, 20, 1_000);
    assert_eq!(limiter.retry_after_secs(), 1);

    // In-memory limiter + port.
    let store = InMemoryRateLimiter::new(10.0, 20, 1_000);
    let _: &dyn IRateLimiter = &store;
}
