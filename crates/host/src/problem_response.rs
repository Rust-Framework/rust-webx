//! RFC 7807 Problem Details helpers for consistent HTTP error responses.

use rust_webx_core::http::IHttpContext;
use rust_webx_core::problem::ProblemDetails;
use std::collections::HashMap;

/// Standard HTTP status phrase for Problem Details `title`.
pub fn problem_status_title(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Error",
    }
}

/// Build a Problem Details value with type URI and detail.
pub fn build_problem(status: u16, detail: impl Into<String>) -> ProblemDetails {
    ProblemDetails {
        typ: Some(format!("https://httpstatuses.com/{}", status)),
        title: problem_status_title(status).into(),
        status,
        detail: Some(detail.into()),
        instance: None,
        extensions: None,
    }
}

/// Serialize problem details to JSON bytes (for sync response setup).
pub fn problem_to_bytes(problem: &ProblemDetails) -> Vec<u8> {
    serde_json::to_vec(problem).unwrap_or_default()
}

/// Write RFC 7807 problem+json to the response.
pub async fn write_problem_response(ctx: &mut dyn IHttpContext, problem: ProblemDetails) {
    let status = problem.status;
    ctx.response_mut().set_status(status);
    ctx.response_mut()
        .set_header("content-type", "application/problem+json");
    let _ = ctx
        .response_mut()
        .write_bytes(problem_to_bytes(&problem))
        .await;
}

/// Convenience: status + detail string.
pub async fn write_problem(ctx: &mut dyn IHttpContext, status: u16, detail: impl Into<String>) {
    write_problem_response(ctx, build_problem(status, detail)).await;
}

/// 403 with optional extension fields (e.g. `required_role`).
pub async fn write_forbidden(
    ctx: &mut dyn IHttpContext,
    detail: impl Into<String>,
    extensions: HashMap<String, serde_json::Value>,
) {
    let mut problem = build_problem(403, detail);
    if !extensions.is_empty() {
        problem.extensions = Some(extensions);
    }
    write_problem_response(ctx, problem).await;
}
