//! RFC 7807 / RFC 9457 Problem Details for HTTP APIs.
//!
//! Standard error response format for machine-readable errors.

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

impl ProblemDetails {
    pub fn new(status: u16, title: impl Into<String>) -> Self {
        Self {
            typ: None,
            title: title.into(),
            status,
            detail: None,
            instance: None,
            extensions: None,
        }
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
            extensions: None,
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

        let mut ext = HashMap::new();
        ext.insert(
            "errors".into(),
            serde_json::to_value(grouped).unwrap_or_default(),
        );
        Self {
            typ: Some("https://httpstatuses.com/400".into()),
            title: "Validation Failed".into(),
            status: 400,
            detail: Some("One or more validation errors occurred".into()),
            instance: None,
            extensions: Some(ext),
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
