//! RFC 7807 / RFC 9457 Problem Details for HTTP APIs.
//!
//! Standard error response format for machine-readable errors.
//!
//! `code` and `retryable` are RFC 9457 **extension members** carried in
//! [`ProblemDetails::extensions`], not new struct fields: keeping the struct
//! layout unchanged is what makes Error 2.0 a semver-minor change for
//! downstream code that constructs or destructures `ProblemDetails`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use rust_core::error_codes::{default_code_for_status, status_is_retryable};
use rust_core::{AppError, ErrorKind, FieldProblem};

/// Reserved extension key carrying the verbatim `title` of a pre-built
/// [`ProblemDetails`] (the marker of the verbatim mode).
pub const X_WEBX_TITLE: &str = "x-webx-title";
/// Reserved extension key carrying the verbatim `type` member.
pub const X_WEBX_TYPE: &str = "x-webx-type";
/// Reserved extension key carrying the verbatim `instance` member.
pub const X_WEBX_INSTANCE: &str = "x-webx-instance";
/// Reserved extension key carrying a typed `429` retry-after value.
pub const X_WEBX_RETRY_AFTER: &str = "x-webx-retry-after";
/// Reserved prefix; never write these into a response body or a log line.
pub const X_WEBX_PREFIX: &str = "x-webx-";

/// Whether `key` is a framework-reserved extension member.
pub fn is_reserved_extension(key: &str) -> bool {
    key.starts_with(X_WEBX_PREFIX)
}

/// Iterator over the extension members that may be written on the wire.
///
/// The `x-webx-*` keys are internal carriers (verbatim title/type/instance and
/// the typed retry-after value); a custom [`IErrorResponseWriter`] that flattens
/// extensions must use this filter rather than `AppError::extensions()`.
///
/// [`IErrorResponseWriter`]: crate::error::IErrorResponseWriter
pub fn extensions_for_wire(
    error: &AppError,
) -> impl Iterator<Item = (&String, &serde_json::Value)> {
    error
        .extensions()
        .iter()
        .filter(|(key, _)| !is_reserved_extension(key))
}

/// The typed `429` retry-after value carried by an [`AppError`], if any.
pub fn retry_after_of(error: &AppError) -> Option<u64> {
    error
        .extensions()
        .get(X_WEBX_RETRY_AFTER)
        .and_then(serde_json::Value::as_u64)
}

/// Standard HTTP status phrase for Problem Details `title`.
fn status_title(status: u16) -> &'static str {
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
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Error",
    }
}

/// Convert a pre-built RFC 9457 problem into the shared carrier.
///
/// The `title` / `type` / `instance` members move into the reserved
/// `x-webx-*` extensions so the renderer can reproduce the original wire
/// exactly (including a deliberately absent `type`). `code` / `retryable` /
/// `errors` become typed carrier fields; every other extension stays verbatim.
impl From<ProblemDetails> for AppError {
    fn from(problem: ProblemDetails) -> Self {
        let status = problem.status;
        let code = problem
            .code()
            .map(str::to_string)
            .unwrap_or_else(|| default_code_for_status(status).to_string());
        let retryable = problem
            .retryable()
            .unwrap_or_else(|| status_is_retryable(status));

        let mut error = AppError::new(ErrorKind::from_status(status), code)
            .with_status(status)
            .with_retryable(retryable);
        if let Some(detail) = problem.detail.clone() {
            error = error.with_detail(detail);
        }

        // Per-field `errors` are a typed member of the carrier; everything else
        // is an extension.
        if let Some(extensions) = problem.extensions.as_ref() {
            if let Some(serde_json::Value::Object(errors)) = extensions.get("errors") {
                for (field, messages) in errors {
                    let messages: Vec<String> = match messages {
                        serde_json::Value::Array(values) => values
                            .iter()
                            .filter_map(|value| value.as_str().map(str::to_string))
                            .collect(),
                        serde_json::Value::String(message) => vec![message.clone()],
                        _ => continue,
                    };
                    for message in messages {
                        error = error.with_field(FieldProblem::new(field.clone(), message));
                    }
                }
            }
            for (key, value) in extensions {
                if matches!(key.as_str(), "code" | "retryable" | "errors")
                    || is_reserved_extension(key)
                {
                    continue;
                }
                error = error.with_extension(key.clone(), value.clone());
            }
        }

        error = error.with_extension(X_WEBX_TITLE, problem.title);
        if let Some(typ) = problem.typ {
            error = error.with_extension(X_WEBX_TYPE, typ);
        }
        if let Some(instance) = problem.instance {
            error = error.with_extension(X_WEBX_INSTANCE, instance);
        }
        error
    }
}

impl ProblemDetails {
    /// Render an [`AppError`] as RFC 9457.
    ///
    /// * **verbatim mode** (an `x-webx-title` extension is present): restore the
    ///   original `title`/`type`/`instance` exactly, including an absent `type`;
    ///   5xx details are *not* sanitized (the caller pre-built the problem).
    /// * **derived mode**: derive `title`/`type`, sanitize 5xx details, and fill
    ///   `instance` with the request path.
    ///
    /// `code`, `retryable`, `errors` and `traceId` are typed/extensions members
    /// written back from the carrier; reserved `x-webx-*` keys never reach the
    /// body.
    pub fn from_app_error(error: &AppError, path: &str, trace_id: Option<&str>) -> Self {
        let status = error.status_code();
        let extensions = error.extensions();
        let verbatim_title = extensions
            .get(X_WEBX_TITLE)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);

        let mut body_extensions: HashMap<String, serde_json::Value> = HashMap::new();
        for (key, value) in extensions {
            if !is_reserved_extension(key) {
                body_extensions.insert(key.clone(), value.clone());
            }
        }
        body_extensions.insert(
            "code".into(),
            serde_json::Value::String(error.code().to_string()),
        );
        body_extensions.insert(
            "retryable".into(),
            serde_json::Value::Bool(error.retryable()),
        );
        if !error.fields().is_empty() {
            let mut grouped: std::collections::BTreeMap<String, Vec<&str>> =
                std::collections::BTreeMap::new();
            for field in error.fields() {
                grouped
                    .entry(field.field.clone())
                    .or_default()
                    .push(&field.message);
            }
            body_extensions.insert(
                "errors".into(),
                serde_json::to_value(grouped).unwrap_or_default(),
            );
        }
        if let Some(trace_id) = trace_id {
            body_extensions.insert(
                "traceId".into(),
                serde_json::Value::String(trace_id.to_string()),
            );
        }

        let (typ, title, detail, instance) = match verbatim_title {
            Some(title) => (
                extensions
                    .get(X_WEBX_TYPE)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                title,
                error.detail().map(str::to_string),
                extensions
                    .get(X_WEBX_INSTANCE)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            ),
            None => {
                let detail = if status >= 500 {
                    Some("an unexpected error occurred".to_string())
                } else {
                    error.public_detail().map(str::to_string)
                };
                (
                    Some(format!("https://httpstatuses.com/{status}")),
                    status_title(status).to_string(),
                    detail,
                    (!path.is_empty()).then(|| path.to_string()),
                )
            }
        };

        Self {
            typ,
            title,
            status,
            detail,
            instance,
            extensions: Some(body_extensions),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProblemDetails {
    #[serde(skip_serializing_if = "Option::is_none", rename = "type")]
    pub typ: Option<String>,
    pub title: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", flatten)]
    pub extensions: Option<HashMap<String, serde_json::Value>>,
}

/// Extension key for the stable machine-readable error code.
const CODE_KEY: &str = "code";
/// Extension key for "may the client retry the same request?".
const RETRYABLE_KEY: &str = "retryable";
/// Extension key mirroring the `x-trace-id` response header.
const TRACE_ID_KEY: &str = "traceId";

impl ProblemDetails {
    pub fn new(status: u16, title: impl Into<String>) -> Self {
        let mut problem = Self {
            typ: None,
            title: title.into(),
            status,
            detail: None,
            instance: None,
            extensions: None,
        };
        problem.complete(None, None);
        problem
    }

    pub fn not_found(resource: impl Into<String>, id: impl std::fmt::Display) -> Self {
        let r = resource.into();
        let is = format!("{}", id);
        Self {
            typ: Some("https://httpstatuses.com/404".into()),
            title: "Resource Not Found".into(),
            status: 404,
            detail: Some(format!("{} '{}' not found", r, is)),
            instance: Some(format!("/{}s/{}", r.to_lowercase(), is)),
            extensions: Some(HashMap::from([
                (
                    CODE_KEY.to_string(),
                    serde_json::Value::String("not_found".into()),
                ),
                (RETRYABLE_KEY.to_string(), serde_json::Value::Bool(false)),
            ])),
        }
    }

    /// Build a 400 validation problem from field-level failures.
    ///
    /// `errors` is serialized as ASP.NET Core does — an object keyed by field
    /// name, each holding every message for that field:
    ///
    /// ```json
    /// { "errors": { "page": ["The `page` field must be a valid integer (got `x`)."] } }
    /// ```
    pub fn validation(errors: Vec<FieldError>) -> Self {
        let mut grouped: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for error in errors {
            grouped.entry(error.field).or_default().push(error.message);
        }

        let mut extensions = HashMap::new();
        extensions.insert(
            "errors".into(),
            serde_json::to_value(grouped).unwrap_or_default(),
        );
        extensions.insert(
            CODE_KEY.to_string(),
            serde_json::Value::String("validation_failed".into()),
        );
        extensions.insert(RETRYABLE_KEY.to_string(), serde_json::Value::Bool(false));
        Self {
            typ: Some("https://httpstatuses.com/400".into()),
            title: "Validation Failed".into(),
            status: 400,
            detail: Some("One or more validation errors occurred".into()),
            instance: None,
            extensions: Some(extensions),
        }
    }

    pub fn with_detail(mut self, d: impl Into<String>) -> Self {
        self.detail = Some(d.into());
        self
    }

    pub fn with_instance(mut self, i: impl Into<String>) -> Self {
        self.instance = Some(i.into());
        self
    }

    /// The stable machine-readable code, when one was supplied.
    pub fn code(&self) -> Option<&str> {
        self.extensions.as_ref()?.get(CODE_KEY)?.as_str()
    }

    /// Whether the client may retry, when the caller declared it.
    pub fn retryable(&self) -> Option<bool> {
        self.extensions.as_ref()?.get(RETRYABLE_KEY)?.as_bool()
    }

    /// Override the machine-readable code.
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        let extensions = self.extensions.get_or_insert_with(HashMap::new);
        extensions.insert(CODE_KEY.to_string(), serde_json::Value::String(code.into()));
        self
    }

    /// Declare whether the client may retry.
    pub fn with_retryable(mut self, retryable: bool) -> Self {
        let extensions = self.extensions.get_or_insert_with(HashMap::new);
        extensions.insert(
            RETRYABLE_KEY.to_string(),
            serde_json::Value::Bool(retryable),
        );
        self
    }

    /// Fill the extension members the framework guarantees when the caller did
    /// not supply them: `code`, `retryable`, `instance` and `traceId`.
    ///
    /// `path` is the request path (used for `instance` when the problem has
    /// none); `trace_id` mirrors the `x-trace-id` response header.
    pub fn complete(&mut self, path: Option<&str>, trace_id: Option<&str>) {
        let default_code = crate::error::default_code_for_status(self.status);
        let default_retryable = crate::error::status_is_retryable(self.status);
        {
            let extensions = self.extensions.get_or_insert_with(HashMap::new);
            extensions
                .entry(CODE_KEY.to_string())
                .or_insert_with(|| serde_json::Value::String(default_code.to_string()));
            extensions
                .entry(RETRYABLE_KEY.to_string())
                .or_insert_with(|| serde_json::Value::Bool(default_retryable));
            if let Some(trace_id) = trace_id {
                extensions
                    .entry(TRACE_ID_KEY.to_string())
                    .or_insert_with(|| serde_json::Value::String(trace_id.to_string()));
            }
        }
        if self.instance.is_none() {
            self.instance = path.map(str::to_string);
        }
    }

    /// Carry this problem through the error pipeline unchanged.
    ///
    /// The host writes it verbatim (status, `application/problem+json`, and the
    /// `errors` extension), so binding failures keep their per-field detail.
    pub fn to_error(self) -> crate::error::Error {
        crate::error::Error::Problem(Box::new(self))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

impl FieldError {
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_problem_round_trips_through_app_error() {
        let original = ProblemDetails::validation(vec![
            FieldError::new("page", "must be at least 1"),
            FieldError::new("page_size", "must be at most 100"),
        ]);
        let wire_before = serde_json::to_value(&original).expect("serialize original");

        let error: AppError = original.into();
        assert_eq!(error.status_code(), 400);
        assert_eq!(error.code(), "validation_failed");
        assert_eq!(error.fields().len(), 2);

        let restored = ProblemDetails::from_app_error(&error, "/api/orders", None);
        let wire_after = serde_json::to_value(&restored).expect("serialize restored");
        assert_eq!(wire_after, wire_before, "verbatim problem must not change");
    }

    #[test]
    fn prebuilt_problem_keeps_absent_type_and_verbatim_detail() {
        let original = ProblemDetails::new(418, "I'm a teapot")
            .with_detail("short and stout")
            .with_instance("/orders/7");
        let error: AppError = original.into();
        let restored = ProblemDetails::from_app_error(&error, "/api/orders/7", None);
        let value = serde_json::to_value(&restored).expect("serialize");

        assert_eq!(value["status"], 418);
        assert_eq!(value["title"], "I'm a teapot");
        assert_eq!(value["detail"], "short and stout");
        assert_eq!(value["instance"], "/orders/7");
        assert!(
            value.get("type").is_none(),
            "a pre-built problem without a type must not gain one"
        );
    }

    #[test]
    fn prebuilt_500_problem_is_not_sanitized() {
        let error: AppError = ProblemDetails::new(500, "Internal Server Error")
            .with_detail("db down")
            .into();
        let restored = ProblemDetails::from_app_error(&error, "/api/x", None);
        assert_eq!(restored.detail.as_deref(), Some("db down"));
    }

    #[test]
    fn derived_error_gets_status_title_and_sanitized_500_detail() {
        let derived = ProblemDetails::from_app_error(
            &AppError::internal("connection string leaked here"),
            "/api/orders",
            Some("trace-1"),
        );
        let value = serde_json::to_value(&derived).expect("serialize");
        assert_eq!(value["status"], 500);
        assert_eq!(value["title"], "Internal Server Error");
        assert_eq!(value["detail"], "an unexpected error occurred");
        assert_eq!(value["instance"], "/api/orders");
        assert_eq!(value["traceId"], "trace-1");
        assert_eq!(value["code"], "internal_error");
    }

    #[test]
    fn reserved_extensions_never_reach_the_wire_and_retry_after_is_typed() {
        let error = AppError::too_many_requests(7, "slow down");
        assert_eq!(retry_after_of(&error), Some(7));
        assert!(extensions_for_wire(&error).next().is_none());

        let rendered = ProblemDetails::from_app_error(&error, "/api/x", None);
        let value = serde_json::to_value(&rendered).expect("serialize");
        assert_eq!(value["status"], 429);
        assert_eq!(value["detail"], "slow down");
        assert!(
            !value
                .as_object()
                .expect("object")
                .keys()
                .any(|k| k.starts_with(X_WEBX_PREFIX)),
            "reserved keys leaked: {value}"
        );
    }
}
