//! OpenAPI 3.0.3 specification generator.
//!
//! Scans the inventory for all registered routes and builds
//! a rich OpenAPI JSON document with parameters, request bodies,
//! response schemas, and summaries.

use rust_webx_core::route::scan::RouteEntry;
use serde_json::{json, Value as JsonValue};
use std::collections::BTreeSet;

/// Generate an OpenAPI 3.0.3 specification from registered routes.
///
/// Returns a `serde_json::Value` that can be serialized to JSON.
pub fn generate_openapi_spec(title: &str, version: &str) -> JsonValue {
    let mut paths = serde_json::Map::new();
    let mut tags_set = BTreeSet::new();

    for entry in inventory::iter::<RouteEntry> {
        let entry = entry.clone();
        let method = entry.method.as_str().to_lowercase();
        let tag = extract_tag(entry.path);
        tags_set.insert(tag.clone());

        let mut parameters = Vec::new();
        let mut has_body = false;

        for param in entry.params {
            if param.source == "body" {
                has_body = true;
            } else {
                parameters.push(json!({
                    "name": param.name,
                    "in": param.source,
                    "required": true,
                    "schema": { "type": param.type_hint },
                }));
            }
        }

        let operation_id = to_operation_id(entry.handler_type);

        let mut operation = json!({
            "operationId": operation_id,
            "summary": entry.summary,
            "responses": build_responses(entry.rsp_type),
            "tags": [tag],
        });

        if !entry.description.is_empty() {
            operation["description"] = json!(entry.description);
        }

        if !parameters.is_empty() {
            operation["parameters"] = json!(parameters);
        }

        if has_body {
            operation["requestBody"] = json!({
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                        }
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

    json!({
        "openapi": "3.0.3",
        "info": {
            "title": title,
            "version": version,
        },
        "tags": tags,
        "paths": paths,
        "components": {
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

/// Build a responses object based on the response type name.
fn build_responses(rsp_type: &str) -> JsonValue {
    let schema = if rsp_type == "()" || rsp_type == "unknown" {
        json!({ "type": "object" })
    } else if rsp_type == "String" {
        json!({ "type": "string" })
    } else if rsp_type.starts_with("Vec<") {
        let inner = &rsp_type[4..rsp_type.len() - 1];
        json!({
            "type": "array",
            "items": { "$ref": format!("#/components/schemas/{}", inner) }
        })
    } else {
        json!({ "$ref": format!("#/components/schemas/{}", rsp_type) })
    };

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
