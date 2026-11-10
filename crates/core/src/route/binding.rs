//! Request model binding — the runtime half of `#[derive(WebxRequestMeta)]`.
//!
//! A request struct declares where each field comes from (`#[frompath]`,
//! `#[fromquery]`, `#[frombody]`, `#[fromform]`, `#[fromheader]`,
//! `#[fromclaim]`). The derive compiles that declaration into a
//! [`RequestModel::bind`] implementation; this module provides the value
//! providers and coercion helpers that implementation calls.
//!
//! # Why not `serde` for query/path/header/claim
//!
//! HTTP values are *strings*, often repeated (`?id=1&id=2`), and a failure in
//! one field must not hide the others. `serde_json` models documents, not HTTP
//! values, so it cannot coerce `"2"` into `u32`, cannot represent repeated keys,
//! and fails at the first error. Here every scalar is parsed with [`FromStr`],
//! lists keep every occurrence, and failures accumulate in a [`Binder`] so the
//! response can report all of them at once.
//!
//! JSON bodies stay with `serde`: the derive generates a hidden body struct
//! containing only the body-bound fields, so `rename`, `flatten`, `default` and
//! custom deserializers keep working exactly as written.

use crate::auth::IClaims;
use crate::error::{Error, Result};
use crate::form::{FormFile, MultipartForm};
use crate::http::IHttpContext;
use crate::problem::{FieldError, ProblemDetails};
use crate::route::scan::ParamMeta;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;

/// Where a request field's value comes from.
///
/// [`Source::Auto`] is the absence of an explicit marker; the generated binder
/// then resolves it from the request method:
///
/// * `GET` / `HEAD` / `DELETE` / `OPTIONS` — query string, then the route;
/// * `POST` / `PUT` / `PATCH` — JSON body, then form, then query string;
/// * `multipart/form-data` — form, then query string.
///
/// Headers and claims are never implicit: a header or token that happens to
/// share a field name must not silently win.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    /// A `{name}` segment of the route template.
    Path,
    /// A query-string key (`?name=value`, repeatable).
    Query,
    /// A `multipart/form-data` part (text or file).
    Form,
    /// An HTTP header, looked up case-insensitively.
    Header,
    /// A claim from the authenticated principal.
    Claim,
    /// The JSON request body.
    Body,
    /// No marker: inferred from the request method.
    Auto,
}

impl Source {
    /// The OpenAPI `in` value (`""` for [`Source::Auto`], resolved per route).
    pub const fn as_str(self) -> &'static str {
        match self {
            Source::Path => "path",
            Source::Query => "query",
            Source::Form => "form",
            Source::Header => "header",
            Source::Claim => "claim",
            Source::Body => "body",
            Source::Auto => "",
        }
    }

    /// Whether an explicitly marked field must be present.
    pub const fn is_explicit(self) -> bool {
        !matches!(self, Source::Auto)
    }
}

/// Whether `method` carries a JSON body by default (ASP.NET Core's rule).
pub fn is_body_method(method: &str) -> bool {
    matches!(method, "POST" | "PUT" | "PATCH")
}

/// A type that can be constructed from an HTTP request.
///
/// Implemented by `#[derive(WebxRequestMeta)]`. The endpoint macros call
/// [`RequestModel::bind`] before dispatching to the handler, so a request type
/// without the derive does not compile.
///
/// ```ignore
/// #[derive(Default, Deserialize, WebxRequestMeta)]
/// pub struct GetUserRequest {
///     #[frompath]
///     pub id: String,
///     #[fromquery]
///     pub expand: Option<String>,
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be bound from an HTTP request",
    label = "add `#[derive(WebxRequestMeta)]` to this request struct",
    note = "the endpoint macros call `RequestModel::bind`; the derive is what implements it"
)]
pub trait RequestModel: Sized + Send + 'static {
    /// OpenAPI parameter metadata for this request type.
    ///
    /// Sources are `"path"`, `"query"`, `"header"`, `"form"`, `"body"`,
    /// `"claim"`, or `""` for an unmarked field resolved by the route method.
    fn param_meta() -> &'static [ParamMeta];

    /// Bind an instance from the live request context.
    fn bind<'a>(
        ctx: &'a mut dyn IHttpContext,
    ) -> Pin<Box<dyn Future<Output = Result<Self>> + Send + 'a>>;
}

/// The value providers a generated [`RequestModel::bind`] reads from.
///
/// Borrowed from the request context: binding never copies the route, query or
/// header maps, and the form is shared through the request's cached parse.
pub struct BindInput<'a> {
    /// Route parameters captured by the router (`{}` segments).
    pub path: &'a HashMap<String, String>,
    /// Query string, last value per key (the compatibility view).
    pub query: &'a HashMap<String, String>,
    /// Query string, every value per key; `None` when the host does not
    /// provide multi-value parsing.
    pub query_multi: Option<&'a HashMap<String, Vec<String>>>,
    /// The authenticated principal, when the request is authenticated.
    pub claims: Option<&'a dyn IClaims>,
    /// The parsed `multipart/form-data` body, when the request is multipart.
    pub form: Option<&'a MultipartForm>,
    /// Whether the route method is `POST`, `PUT` or `PATCH`.
    pub is_body: bool,
    /// Whether the request declares `multipart/form-data`.
    pub is_multipart: bool,
}

impl<'a> BindInput<'a> {
    /// The route value for `name`.
    pub fn path_one(&self, name: &str) -> Option<&'a str> {
        self.path.get(name).map(String::as_str)
    }

    /// Every route value for `name`, comma-separated lists expanded.
    ///
    /// A route segment is a single value, but `{ids}` accepting `1,2,3` is a
    /// convenient way to bind `Vec<T>` from the path — the same rule query
    /// values follow.
    pub fn path_list(&self, name: &str) -> Vec<&'a str> {
        match self.path.get(name) {
            Some(value) => split_one(value),
            None => Vec::new(),
        }
    }

    /// The last query value for `name`.
    pub fn query_one(&self, name: &str) -> Option<&'a str> {
        if let Some(multi) = self.query_multi {
            if let Some(values) = multi.get(name) {
                return values.last().map(String::as_str);
            }
        }
        self.query.get(name).map(String::as_str)
    }

    /// Every query value for `name`, comma-separated lists expanded.
    pub fn query_list(&self, name: &str) -> Vec<&'a str> {
        if let Some(multi) = self.query_multi {
            if let Some(values) = multi.get(name) {
                return split_values(values);
            }
        }
        match self.query.get(name) {
            Some(value) => split_one(value),
            None => Vec::new(),
        }
    }

    /// The first text part named `name` in the form.
    pub fn form_one(&self, name: &str) -> Option<&'a str> {
        self.form?.text(name)
    }

    /// Every text part named `name`, comma-separated lists expanded.
    pub fn form_list(&self, name: &str) -> Vec<&'a str> {
        match self.form {
            Some(form) => form.texts(name).flat_map(split_one).collect(),
            None => Vec::new(),
        }
    }

    /// The first file part named `name`.
    pub fn file(&self, name: &str) -> Option<&'a FormFile> {
        self.form?.file(name)
    }

    /// Every file part named `name`, in order.
    pub fn files(&self, name: &str) -> Vec<&'a FormFile> {
        match self.form {
            Some(form) => form.files_named(name).collect(),
            None => Vec::new(),
        }
    }

    /// A claim value by key.
    ///
    /// The raw claim map wins, so a token that literally carries `user_id` or
    /// `tenant` keeps that value. The dedicated accessors are only a fallback
    /// when the raw map has no such key: `sub` / `subject` resolve to the
    /// subject, `tenant_id` / `tenant` to the tenant helper.
    pub fn claim(&self, key: &str) -> Option<&'a str> {
        let claims = self.claims?;
        if let Some(value) = claims.claims().get(key) {
            return Some(value.as_str());
        }
        match key {
            "sub" | "subject" | "user_id" => Some(claims.subject()),
            "tenant_id" | "tenant" => claims.get_tenantid(),
            _ => None,
        }
    }

    /// A claim as a list: the raw claim map wins (comma-separated);
    /// `roles` / `permissions` fall back to their dedicated slots when the raw
    /// map has no such key.
    pub fn claim_list(&self, key: &str) -> Vec<&'a str> {
        let Some(claims) = self.claims else {
            return Vec::new();
        };
        if let Some(value) = claims.claims().get(key) {
            return split_one(value);
        }
        match key {
            "roles" => claims.roles().iter().map(String::as_str).collect(),
            "permissions" => claims.permissions().iter().map(String::as_str).collect(),
            _ => Vec::new(),
        }
    }
}

/// Convert a JSON body deserialization failure into a field-level 400.
///
/// `serde_json` reports the offending field for the common cases; those become
/// `errors.<field>` entries, and everything else is reported at the body level
/// under the empty key — the same convention ASP.NET Core uses for a body
/// model that cannot be bound at all.
pub fn body_error(error: serde_json::Error) -> Error {
    let message = error.to_string();
    let field = extract_error_field(&message)
        .unwrap_or_default()
        .to_string();
    ProblemDetails::validation(vec![FieldError::new(field, message)]).to_error()
}

fn extract_error_field(message: &str) -> Option<&str> {
    for marker in ["missing field `", "unknown field `", "duplicate field `"] {
        if let Some((_, rest)) = message.split_once(marker) {
            if let Some((name, _)) = rest.split_once('`') {
                return Some(name);
            }
        }
    }
    None
}

/// A field-level body failure: the JSON key was present but its value did not
/// match the field type.
pub fn body_field_error(field: &str, error: serde_json::Error) -> Error {
    ProblemDetails::validation(vec![FieldError::new(
        field,
        format!("The `{field}` field is invalid: {error}"),
    )])
    .to_error()
}

/// Remove `key` (or one of `aliases`) from a JSON object.
///
/// `serde`'s `rename`/`alias` are resolved by the derive; this looks the
/// resolved keys up and takes ownership of the value, so the remaining map only
/// holds keys no field claimed (which `deny_unknown_fields` can reject).
pub fn take_body_field(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    aliases: &[&str],
) -> Option<serde_json::Value> {
    if let Some(value) = object.remove(key) {
        return Some(value);
    }
    for alias in aliases {
        if let Some(value) = object.remove(*alias) {
            return Some(value);
        }
    }
    None
}

/// Split one raw value on commas, dropping empty segments.
///
/// An empty value is an **empty list**, not a list holding one empty string:
/// `?ids=` means "present, no ids" (`Option<Vec<T>>` binds `Some(vec![])`).
fn split_one(value: &str) -> Vec<&str> {
    if value.is_empty() {
        Vec::new()
    } else if value.contains(',') {
        value
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect()
    } else {
        vec![value]
    }
}

/// Expand one optional raw value into a list, splitting on commas.
///
/// Public because the derive uses it for `Vec<T>` header fields, which carry a
/// single comma-separated line.
pub fn split_csv(value: Option<&str>) -> Vec<&str> {
    match value {
        Some(value) => split_one(value),
        None => Vec::new(),
    }
}

/// Split every raw value on commas, flattening the results.
fn split_values(values: &[String]) -> Vec<&str> {
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        out.extend(split_one(value));
    }
    out
}

/// Accumulates field-level binding failures into one 400 response.
///
/// Binding is best-effort: a bad field records an error and yields `None`, the
/// remaining fields still bind, and the request is rejected once at the end
/// with every failure listed — matching ASP.NET Core's `ModelState`.
#[derive(Default)]
pub struct Binder {
    errors: Vec<FieldError>,
}

impl Binder {
    /// A binder with no recorded errors.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether any field failed.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// The recorded failures, in binding order.
    pub fn errors(&self) -> &[FieldError] {
        &self.errors
    }

    /// Consume the binder, yielding the 400 `ProblemDetails` error.
    ///
    /// The response body carries `errors` as `{ "field": ["message"] }`.
    pub fn into_error(self) -> Error {
        ProblemDetails::validation(self.errors).to_error()
    }

    /// Record the canonical "required" failure for `field`.
    pub fn missing(&mut self, field: &str) {
        self.errors.push(FieldError::new(
            field,
            format!("The `{field}` field is required."),
        ));
    }

    /// Record an unparsable value for `field`.
    pub fn invalid(&mut self, field: &str, kind: &str, value: &str) {
        self.errors.push(FieldError::new(
            field,
            format!("The `{field}` field must be a valid {kind} (got `{value}`)."),
        ));
    }

    /// Record a JSON key that no field claimed (`deny_unknown_fields`).
    pub fn unknown_field(&mut self, name: &str) {
        self.errors.push(FieldError::new(
            "",
            format!("The body contains an unknown field `{name}`."),
        ));
    }

    /// Parse a required scalar.
    ///
    /// Returns `None` (and records an error) when the value is absent or
    /// unparsable; the caller checks [`Binder::has_errors`] before building the
    /// request.
    pub fn required<T: FromStr>(
        &mut self,
        raw: Option<&str>,
        field: &str,
        kind: &str,
    ) -> Option<T> {
        let Some(raw) = raw else {
            self.missing(field);
            return None;
        };
        match raw.parse::<T>() {
            Ok(value) => Some(value),
            Err(_) => {
                self.invalid(field, kind, raw);
                None
            }
        }
    }

    /// Parse an optional scalar: absent is `None`, present-but-invalid is an
    /// error (never silently `None`).
    pub fn optional<T: FromStr>(
        &mut self,
        raw: Option<&str>,
        field: &str,
        kind: &str,
    ) -> Option<T> {
        let raw = raw?;
        match raw.parse::<T>() {
            Ok(value) => Some(value),
            Err(_) => {
                self.invalid(field, kind, raw);
                None
            }
        }
    }

    /// Parse every occurrence of a repeated value into a list.
    ///
    /// An empty list is valid (ASP.NET Core binds missing collections as
    /// empty); unparsable elements are errors.
    pub fn list<T: FromStr>(&mut self, raw: &[&str], field: &str, kind: &str) -> Vec<T> {
        let mut out = Vec::with_capacity(raw.len());
        for value in raw {
            match value.parse::<T>() {
                Ok(parsed) => out.push(parsed),
                Err(_) => self.invalid(field, kind, value),
            }
        }
        out
    }

    /// Record a "required" failure for an already-extracted optional value.
    ///
    /// Used for body fields: `serde` has already validated the body, so a
    /// missing value here means the field was skipped by the shadow struct.
    pub fn take<T>(&mut self, value: Option<T>, field: &str) -> Option<T> {
        if value.is_none() {
            self.missing(field);
        }
        value
    }

    /// The first uploaded file, required.
    pub fn required_file(&mut self, file: Option<&FormFile>, field: &str) -> Option<FormFile> {
        match file {
            Some(file) => Some(file.clone()),
            None => {
                self.missing(field);
                None
            }
        }
    }

    /// The first uploaded file, optional.
    pub fn optional_file(&mut self, file: Option<&FormFile>) -> Option<FormFile> {
        file.cloned()
    }

    /// Every uploaded file under `name`.
    pub fn files(&mut self, files: Vec<&FormFile>) -> Vec<FormFile> {
        files.into_iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn required_reports_missing_and_invalid() {
        let mut binder = Binder::new();
        assert_eq!(binder.required::<u32>(None, "page", "integer"), None);
        assert_eq!(binder.required::<u32>(Some("abc"), "page", "integer"), None);
        assert_eq!(
            binder.required::<u32>(Some("2"), "page", "integer"),
            Some(2)
        );
        assert_eq!(binder.errors().len(), 2);
        assert!(binder.errors()[0].message.contains("required"));
    }

    #[test]
    fn optional_keeps_absent_as_none_and_rejects_invalid() {
        let mut binder = Binder::new();
        assert_eq!(binder.optional::<u32>(None, "page", "integer"), None);
        assert!(!binder.has_errors());
        assert_eq!(binder.optional::<u32>(Some("x"), "page", "integer"), None);
        assert!(binder.has_errors());
    }

    #[test]
    fn list_binds_every_occurrence() {
        let mut binder = Binder::new();
        assert_eq!(
            binder.list::<u32>(&["1", "2", "3"], "ids", "integer"),
            vec![1, 2, 3]
        );
        assert!(!binder.has_errors());
        assert_eq!(binder.list::<u32>(&["1", "x"], "ids", "integer"), vec![1]);
        assert!(binder.has_errors());
    }

    #[test]
    fn query_values_keep_every_occurrence_and_expand_comma_lists() {
        let path = HashMap::new();
        let query = HashMap::new();
        let mut multi: HashMap<String, Vec<String>> = HashMap::new();
        multi.insert("ids".into(), vec!["1".into(), "2,3".into()]);

        let input = BindInput {
            path: &path,
            query: &query,
            query_multi: Some(&multi),
            claims: None,
            form: None,
            is_body: false,
            is_multipart: false,
        };
        assert_eq!(input.query_list("ids"), vec!["1", "2", "3"]);
        assert_eq!(input.query_one("ids"), Some("2,3"));
    }

    #[test]
    fn error_body_groups_fields() {
        let mut binder = Binder::new();
        binder.missing("a");
        binder.missing("a");
        binder.missing("b");
        let error = binder.into_error();
        let Error::Problem(problem) = error else {
            panic!("expected Problem, got {error:?}");
        };
        assert_eq!(problem.status, 400);
        let errors = problem.extensions.as_ref().unwrap().get("errors").unwrap();
        assert_eq!(
            errors["a"],
            serde_json::json!(["The `a` field is required.", "The `a` field is required."])
        );
        assert_eq!(
            errors["b"],
            serde_json::json!(["The `b` field is required."])
        );
    }

    /// Claims whose dedicated accessors deliberately disagree with the raw map,
    /// so a test can tell which one a lookup reads.
    struct FakeClaims {
        subject: String,
        roles: Vec<String>,
        permissions: Vec<String>,
        tenant: Option<String>,
        raw: HashMap<String, String>,
    }

    impl FakeClaims {
        fn new(raw: &[(&str, &str)]) -> Self {
            Self {
                subject: "subject-from-helper".to_string(),
                roles: vec!["slot-role".to_string()],
                permissions: vec!["slot-permission".to_string()],
                tenant: Some("tenant-from-helper".to_string()),
                raw: raw
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect(),
            }
        }
    }

    impl IClaims for FakeClaims {
        fn subject(&self) -> &str {
            &self.subject
        }

        fn roles(&self) -> &[String] {
            &self.roles
        }

        fn permissions(&self) -> &[String] {
            &self.permissions
        }

        fn claims(&self) -> &HashMap<String, String> {
            &self.raw
        }

        fn get_tenantid(&self) -> Option<&str> {
            self.tenant.as_deref()
        }

        fn clone_box(&self) -> Box<dyn IClaims> {
            Box::new(Self {
                subject: self.subject.clone(),
                roles: self.roles.clone(),
                permissions: self.permissions.clone(),
                tenant: self.tenant.clone(),
                raw: self.raw.clone(),
            })
        }
    }

    fn claims_input<'a>(
        path: &'a HashMap<String, String>,
        claims: &'a FakeClaims,
    ) -> BindInput<'a> {
        BindInput {
            path,
            query: path,
            query_multi: None,
            claims: Some(claims),
            form: None,
            is_body: false,
            is_multipart: false,
        }
    }

    #[test]
    fn claim_prefers_the_raw_map_over_the_dedicated_accessors() {
        let path = HashMap::new();
        let claims = FakeClaims::new(&[
            ("sub", "raw-sub"),
            ("user_id", "raw-user-id"),
            ("tenant", "raw-tenant"),
        ]);
        let input = claims_input(&path, &claims);

        // A token that literally carries these keys keeps its own values; the
        // legacy aliases must not silently swap the source.
        assert_eq!(input.claim("sub"), Some("raw-sub"));
        assert_eq!(input.claim("user_id"), Some("raw-user-id"));
        assert_eq!(input.claim("tenant"), Some("raw-tenant"));
    }

    #[test]
    fn claim_falls_back_to_the_dedicated_accessors() {
        let path = HashMap::new();
        let claims = FakeClaims::new(&[]);
        let input = claims_input(&path, &claims);

        assert_eq!(input.claim("sub"), Some("subject-from-helper"));
        assert_eq!(input.claim("subject"), Some("subject-from-helper"));
        assert_eq!(input.claim("user_id"), Some("subject-from-helper"));
        assert_eq!(input.claim("tenant_id"), Some("tenant-from-helper"));
        assert_eq!(input.claim("tenant"), Some("tenant-from-helper"));
        assert_eq!(input.claim("email"), None);
    }

    #[test]
    fn claim_list_prefers_the_raw_map_then_the_dedicated_slots() {
        let path = HashMap::new();
        let claims = FakeClaims::new(&[("roles", "raw-admin, raw-auditor")]);
        let input = claims_input(&path, &claims);

        assert_eq!(input.claim_list("roles"), vec!["raw-admin", "raw-auditor"]);
        // No raw key: the dedicated slot supplies the values.
        assert_eq!(input.claim_list("permissions"), vec!["slot-permission"]);
        assert_eq!(input.claim_list("missing"), Vec::<&str>::new());
    }

    #[test]
    fn an_empty_value_is_an_empty_list_not_one_empty_element() {
        assert_eq!(split_one(""), Vec::<&str>::new());
        assert_eq!(split_one("a"), vec!["a"]);
        assert_eq!(split_one("a,, b"), vec!["a", "b"]);
        assert_eq!(split_csv(Some("")), Vec::<&str>::new());
        assert_eq!(split_csv(None), Vec::<&str>::new());

        // `?ids=` (present, empty) is an empty list rather than a parse error
        // or a one-element list holding `""`.
        let path = HashMap::new();
        let query = HashMap::new();
        let mut multi: HashMap<String, Vec<String>> = HashMap::new();
        multi.insert("ids".into(), vec![String::new()]);
        let input = BindInput {
            path: &path,
            query: &query,
            query_multi: Some(&multi),
            claims: None,
            form: None,
            is_body: false,
            is_multipart: false,
        };
        assert_eq!(input.query_list("ids"), Vec::<&str>::new());
    }
}
