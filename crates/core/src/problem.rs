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
