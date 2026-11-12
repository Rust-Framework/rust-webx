//! OpenAPI 3.0.3 specification generator.
//!
//! Scans the inventory for all registered routes and builds
//! a rich OpenAPI JSON document with parameters, request bodies,
//! response schemas, names, and descriptions.

use serde_json::{json, Value as JsonValue};
use std::collections::{BTreeSet, HashMap};
use webx_core::localization::IStringLocalizer;
use webx_core::route::scan::{ParamMeta, RequestParamEntry, RouteEntry};
use webx_core::routing::HttpMethod;

/// Generate an OpenAPI 3.0.3 specification from registered routes.
///
/// Endpoint names and descriptions are resolved through `localizer` using the
/// host's configured default culture; OpenAPI has no multi-language operation
/// text, so one document is produced per hosted language.
///
/// Returns a `serde_json::Value` that can be serialized to JSON.
pub fn generate_openapi_spec(
    title: &str,
    version: &str,
    localizer: &dyn IStringLocalizer,
) -> JsonValue {
    let request_index = RequestParams::collect();
    let mut paths = serde_json::Map::new();
    let mut tags_set = BTreeSet::new();
    let mut schema_names = BTreeSet::new();

    for entry in inventory::iter::<RouteEntry> {
        let entry = entry.clone();
        let method = entry.method.as_str().to_lowercase();
        // `#[category]` is the explicit grouping; fall back to the path-derived
        // tag so uncategorized endpoints keep their documentation grouping.
        let category = localizer.resolve(entry.category).value;
        let tag = if category.is_empty() {
            extract_tag(entry.path)
        } else {
            category
        };
        tags_set.insert(tag.clone());

        let mut parameters = Vec::new();
        let mut body_params: Vec<ParamMeta> = Vec::new();
        let mut form_params: Vec<ParamMeta> = Vec::new();

        // An unmarked field (`source == ""`) is a body field on a method that
        // carries a body, and a query field everywhere else — the same rule the
        // runtime binder applies.
        let body_method = matches!(
            entry.method,
            HttpMethod::Post | HttpMethod::Put | HttpMethod::Patch
        );

        let request_params = request_index.for_route(entry.req_type_id, entry.handler_type);
        for param in merge_params(entry.params, request_params) {
            let source = match param.source {
                "" if body_method => "body",
                "" => "query",
                other => other,
            };

            match source {
                // An upload endpoint describes its fields as multipart parts;
                // the generic JSON body is not applicable.
                "form" => form_params.push(param),
                "body" => body_params.push(param),
                // Claims come from the principal, not from the request.
                "claim" => {}
                _ => {
                    let required = param.required || source == "path";
                    parameters.push(json!({
                        "name": param.name,
                        "in": source,
                        "required": required,
                        "schema": { "type": param.type_hint },
                    }));
                }
            }
        }

        let operation_id = to_operation_id(entry.handler_type);
        let summary = localizer.resolve(entry.name).value;

        let mut operation = json!({
            "operationId": operation_id,
            "summary": summary,
            "responses": build_responses(entry.rsp_type, &mut schema_names),
            "tags": [tag],
        });

        let description = localizer.resolve(entry.description).value;
        if !description.is_empty() {
            operation["description"] = json!(description);
        }

        if !parameters.is_empty() {
            operation["parameters"] = json!(parameters);
        }

        if !form_params.is_empty() {
            operation["requestBody"] = json!({
                "required": true,
                "content": {
                    "multipart/form-data": {
                        "schema": form_schema(&form_params),
                    }
                }
            });
        } else if body_method && !body_params.is_empty() {
            // Only write methods carry a JSON body. A read method (GET/DELETE/…)
            // cannot bind these fields at runtime, so no request body is
            // advertised for it.
            operation["requestBody"] = json!({
                "required": body_params.iter().any(|param| param.required),
                "content": {
                    "application/json": {
                        "schema": body_schema(&body_params),
                    }
                }
            });
        }

        let path_entry = paths
            .entry(entry.path.to_string())
            .or_insert_with(|| json!({}));
        if let Some(obj) = path_entry.as_object_mut() {
            obj.insert(method, operation);
        }
    }

    let tags: Vec<JsonValue> = tags_set.iter().map(|t| json!({"name": t})).collect();

    // The body shape of a DTO is not reflected at runtime (Rust has no
    // reflection), so a `$ref` would dangle. Emit one placeholder component per
    // referenced name: the document stays valid, and the endpoint's own
    // `rsp_type` still names the type a reader should look up in the code.
    let schemas: serde_json::Map<String, JsonValue> = schema_names
        .into_iter()
        .map(|name| {
            (
                name.clone(),
                json!({
                    "type": "object",
                    "description": format!(
                        "Placeholder for `{name}` — see the Rust type for its fields."
                    ),
                }),
            )
        })
        .collect();

    json!({
        "openapi": "3.0.3",
        "info": {
            "title": title,
            "version": version,
        },
        "tags": tags,
        "paths": paths,
        "components": {
            "schemas": schemas,
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "JWT",
                }
            }
        }
    })
}

/// Request type → parameter metadata, indexed by `TypeId` first and by bare
/// name only as a fallback for hand-written entries.
struct RequestParams {
    by_type: HashMap<std::any::TypeId, &'static [ParamMeta]>,
    by_name: HashMap<&'static str, &'static [ParamMeta]>,
}

impl RequestParams {
    fn collect() -> Self {
        let mut by_type = HashMap::new();
        let mut by_name = HashMap::new();
        for entry in inventory::iter::<RequestParamEntry> {
            by_name.insert(entry.request_type, entry.params);
            // `TypeId::of::<()>()` is the "no type id" sentinel of hand-written
            // entries (`RouteEntry::new`); it must not shadow a real type.
            if entry.req_type_id != std::any::TypeId::of::<()>() {
                by_type.insert(entry.req_type_id, entry.params);
            }
        }
        Self { by_type, by_name }
    }

    /// The parameter metadata for one route's request type.
    ///
    /// The `TypeId` is authoritative: a qualified `self_ty`
    /// (`contracts::ListOrdersRequest`) and two same-named request types in
    /// different modules or crates can never resolve to the wrong entry. The
    /// request type name is a compatibility fallback for hand-written entries,
    /// whose route entry carries the `()` sentinel instead of a real type id.
    fn for_route(
        &self,
        req_type_id: std::any::TypeId,
        request_type: &'static str,
    ) -> Option<&'static [ParamMeta]> {
        self.by_type
            .get(&req_type_id)
            .copied()
            .or_else(|| self.by_name.get(request_type).copied())
    }
}

/// Merge route-level params (path from the route template) with the request
/// struct's own metadata.
///
/// The struct is the richer source: it knows real types and requiredness, while
/// the template only knows that a `{name}` segment exists. On a collision the
/// struct's type hint wins, so `{id}` documents as `integer` when the field is
/// an integer.
fn merge_params(
    route_params: &'static [ParamMeta],
    request_params: Option<&'static [ParamMeta]>,
) -> Vec<ParamMeta> {
    let mut merged: Vec<ParamMeta> = route_params.to_vec();

    if let Some(extra) = request_params {
        for param in extra {
            match merged
                .iter_mut()
                .find(|seen| seen.name == param.name && seen.source == param.source)
            {
                Some(existing) => {
                    if existing.type_hint == "string" && param.type_hint != "string" {
                        existing.type_hint = param.type_hint;
                    }
                    existing.required = existing.required || param.required;
                }
                None => merged.push(param.clone()),
            }
        }
    }

    merged
}

/// Build the `multipart/form-data` object schema for an upload endpoint.
///
/// `type_hint` of `binary` / `binary[]` (set by the derive for `FormFile`
/// fields) maps to OpenAPI's `string`/`format: binary` representation.
fn form_schema(params: &[ParamMeta]) -> JsonValue {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    for param in params {
        let schema = match param.type_hint {
            "binary" => json!({ "type": "string", "format": "binary" }),
            "binary[]" => json!({
                "type": "array",
                "items": { "type": "string", "format": "binary" }
            }),
            other => json!({ "type": other }),
        };
        properties.insert(param.name.to_string(), schema);
        if param.required {
            required.push(json!(param.name));
        }
    }

    json!({
        "type": "object",
        "properties": JsonValue::Object(properties),
        "required": required,
    })
}

/// Build the JSON request body schema from the field-level metadata.
///
/// Only entries carrying a `json_key` become named properties. A body field
/// without one (`#[serde(flatten)]`) spreads its inner properties into the
/// object, which this metadata cannot enumerate: those keys are reported via
/// `additionalProperties: true` while the field's requiredness still counts
/// towards `requestBody.required`.
fn body_schema(params: &[ParamMeta]) -> JsonValue {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    let mut opaque = false;

    for param in params {
        match param.json_key {
            Some(key) => {
                properties.insert(key.to_string(), body_property_schema(param.type_hint));
                if param.required {
                    required.push(json!(key));
                }
            }
            None => opaque = true,
        }
    }

    let mut schema = json!({ "type": "object" });
    if !properties.is_empty() {
        schema["properties"] = JsonValue::Object(properties);
        schema["required"] = json!(required);
    }
    if opaque {
        schema["additionalProperties"] = json!(true);
    }
    schema
}

/// The schema of one JSON body property, from its type hint.
fn body_property_schema(type_hint: &str) -> JsonValue {
    match type_hint {
        "binary" => json!({ "type": "string", "format": "binary" }),
        "array" => json!({ "type": "array" }),
        other => json!({ "type": other }),
    }
}

/// Build a responses object based on the response type name.
///
/// `schema_names` collects every referenced component so the caller can emit a
/// placeholder schema for it (this generator cannot reflect DTO fields).
fn build_responses(rsp_type: &str, schema_names: &mut BTreeSet<String>) -> JsonValue {
    let schema = response_schema(rsp_type, schema_names);

    json!({
        "200": {
            "description": "Successful response",
            "content": {
                "application/json": { "schema": schema }
            }
        },
        "400": { "description": "Bad request — validation or deserialization error" },
        "404": { "description": "Resource not found" },
        "500": { "description": "Internal server error" },
    })
}

/// The success-body schema for one declared response type.
fn response_schema(rsp_type: &str, schema_names: &mut BTreeSet<String>) -> JsonValue {
    if rsp_type == "()" || rsp_type == "unknown" {
        return json!({ "type": "object" });
    }
    if last_type_segment(rsp_type) == "String" {
        return json!({ "type": "string" });
    }
    // Response-control endpoints choose status, headers and body at runtime, so
    // the document can only say "opaque". Detection mirrors
    // `is_raw_response_type` in `rust-webx-macros`
    // (`crates/macros/src/endpoint.rs`): the last path segment is `Response`
    // (bare or qualified) or the type mentions `IActionResult`. Otherwise a
    // `$ref` would point at the wrapper type instead of the response the
    // handler actually writes.
    if is_response_control_type(rsp_type) {
        return json!({
            "type": "object",
            "description": "Response written by the handler (IActionResult: status, headers and body are runtime values).",
        });
    }
    if let Some(inner) = rsp_type
        .strip_prefix("Vec<")
        .and_then(|rest| rest.strip_suffix('>'))
    {
        return json!({
            "type": "array",
            "items": schema_ref(inner, schema_names),
        });
    }
    schema_ref(rsp_type, schema_names)
}

/// The last segment of a (possibly qualified) Rust type name.
fn last_type_segment(rsp_type: &str) -> &str {
    rsp_type.rsplit("::").next().unwrap_or(rsp_type).trim()
}

/// Whether the declared response type is the framework's response-control type.
///
/// Mirrors `is_raw_response_type` in `rust-webx-macros` so the generated spec
/// and the runtime agree; keep the two in sync.
fn is_response_control_type(rsp_type: &str) -> bool {
    last_type_segment(rsp_type) == "Response" || rsp_type.contains("IActionResult")
}

/// A `$ref` to a placeholder component, named after the Rust type.
fn schema_ref(rsp_type: &str, schema_names: &mut BTreeSet<String>) -> JsonValue {
    let name = component_name(rsp_type);
    schema_names.insert(name.clone());
    json!({ "$ref": format!("#/components/schemas/{}", name) })
}

/// Turn a Rust type name into an OpenAPI component key.
///
/// Component keys allow `[A-Za-z0-9._-]` only, so generic parameters and paths
/// (`Vec<UserDto>`, `contracts::UserDto`) are flattened to underscores.
fn component_name(rsp_type: &str) -> String {
    let mut name = String::with_capacity(rsp_type.len());
    let mut last_underscore = false;
    for ch in rsp_type.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' {
            name.push(ch);
            last_underscore = false;
        } else if !last_underscore {
            name.push('_');
            last_underscore = true;
        }
    }
    let trimmed = name.trim_matches('_');
    if trimmed.is_empty() {
        "Unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Extract a tag name from a path for controller-style grouping.
///
/// Uses the last non-parameter segment as the tag, so routes like
/// `/api/users/{id}` and `/api/users` both map to tag `"Users"`.
///
/// e.g., "/api/users/{id}" →"Users", "/health" →"Health"
fn extract_tag(path: &str) -> String {
    let tag = path
        .trim_start_matches('/')
        .split('/')
        .rfind(|s| !s.starts_with('{') && !s.is_empty())
        .unwrap_or("default");
    titlecase(tag)
}

/// Convert a handler type name to a camelCase operation ID.
/// "GetUserRequest" →"getUserRequest", "ListUsersRequest" →"listUsersRequest"
fn to_operation_id(handler_type: &str) -> String {
    let name = handler_type.strip_suffix("Request").unwrap_or(handler_type);
    let mut chars = name.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_ascii_lowercase().to_string() + chars.as_str(),
    }
}

fn titlecase(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}
