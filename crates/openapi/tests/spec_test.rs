//! OpenAPI spec generation tests.

use rust_webx_core::route::scan::{ParamMeta, RequestParamEntry, RouteEntry};
use rust_webx_core::routing::HttpMethod;
use rust_webx_openapi::generate_openapi_spec;

inventory::submit! {
    RouteEntry::new(
        HttpMethod::Get,
        "/api/search",
        "SearchRequest",
        "String",
        "search",
        "",
        &[],
        "",
        "",
    )
}

inventory::submit! {
    RequestParamEntry {
        request_type: "SearchRequest",
        params: &[
            ParamMeta {
                name: "q",
                source: "query",
                type_hint: "string",
            },
            ParamMeta {
                name: "page",
                source: "query",
                type_hint: "integer",
            },
        ],
    }
}

#[test]
fn openapi_spec_has_required_top_level_fields() {
    let spec = generate_openapi_spec("Test API", "1.0.0");
    assert_eq!(spec["openapi"], "3.0.3");
    assert_eq!(spec["info"]["title"], "Test API");
    assert_eq!(spec["info"]["version"], "1.0.0");
    assert!(spec["paths"].is_object());
}

#[test]
fn openapi_spec_includes_query_params_from_request_meta() {
    let spec = generate_openapi_spec("Test API", "1.0.0");
    let params = &spec["paths"]["/api/search"]["get"]["parameters"];
    assert!(params.is_array());
    let names: Vec<&str> = params
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"q"));
    assert!(names.contains(&"page"));
    let q = params
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "q")
        .unwrap();
    assert_eq!(q["in"], "query");
}
