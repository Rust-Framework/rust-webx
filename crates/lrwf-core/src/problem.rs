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
    #[serde(skip_serializing_if = "Option::is_none")]
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

    pub fn validation(errors: Vec<FieldError>) -> Self {
        let mut ext = HashMap::new();
        ext.insert(
            "errors".into(),
            serde_json::to_value(errors).unwrap_or_default(),
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

    pub fn to_error(self) -> crate::error::Error {
        let title = self.title.clone();
        let fallback = serde_json::to_string(&self).unwrap_or(title);
        crate::error::Error::Http(fallback)
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
