//! OpenAPI spec generation tests.

use webx_core::localization::{LocalizationOptions, StringLocalizer, TextRef};
use webx_core::route::scan::{AuthorizeRule, ParamMeta, RequestParamEntry, RouteEntry};
use webx_core::routing::HttpMethod;
use webx_openapi::generate_openapi_spec;

const RESOURCES_ZH: &str = r#"{
    "endpoint.blog.name": "博客模块-列表",
    "endpoint.blog.description": "返回博客列表",
    "endpoint.blog.category": "博客模块"
}"#;

const RESOURCES_EN: &str = r#"{
    "endpoint.blog.name": "Blog - List",
    "endpoint.blog.description": "Returns blog posts",
    "endpoint.blog.category": "Blog"
}"#;

inventory::submit! {
    RouteEntry::new(
        HttpMethod::Get,
        "/api/search",
        "SearchRequest",
        "String",
        TextRef::text("搜索"),
        TextRef::text("全文搜索"),
        TextRef::text(""),
        &[],
        &[],
    )
}

inventory::submit! {
    RouteEntry::new(
        HttpMethod::Get,
        "/api/blog",
        "BlogRequest",
        "String",
        TextRef::key("endpoint.blog.name"),
        TextRef::key("endpoint.blog.description"),
        TextRef::key("endpoint.blog.category"),
        &[],
        &[AuthorizeRule { roles: &["admin"], permissions: &[] }],
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
                required: false,
            },
            ParamMeta {
                name: "page",
                source: "query",
                type_hint: "integer",
                required: false,
            },
        ],
    }
}

fn localizer(default_culture: &str) -> StringLocalizer {
    let options = LocalizationOptions::none()
        .resources("zh-CN", RESOURCES_ZH)
        .resources("en", RESOURCES_EN);
    StringLocalizer::new(default_culture, &options).unwrap()
}

#[test]
fn openapi_spec_has_required_top_level_fields() {
    let spec = generate_openapi_spec("Test API", "1.0.0", &localizer("zh-CN"));
    assert_eq!(spec["openapi"], "3.0.3");
    assert_eq!(spec["info"]["title"], "Test API");
    assert_eq!(spec["info"]["version"], "1.0.0");
    assert!(spec["paths"].is_object());
}

#[test]
fn openapi_spec_includes_query_params_from_request_meta() {
    let spec = generate_openapi_spec("Test API", "1.0.0", &localizer("zh-CN"));
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

#[test]
fn operation_text_uses_the_localizer_default_culture() {
    let zh = generate_openapi_spec("Test API", "1.0.0", &localizer("zh-CN"));
    assert_eq!(zh["paths"]["/api/blog"]["get"]["summary"], "博客模块-列表");
    assert_eq!(
        zh["paths"]["/api/blog"]["get"]["description"],
        "返回博客列表"
    );

    // A host configured with an English default produces an English document.
    let en = generate_openapi_spec("Test API", "1.0.0", &localizer("en-US"));
    assert_eq!(en["paths"]["/api/blog"]["get"]["summary"], "Blog - List");
    assert_eq!(
        en["paths"]["/api/blog"]["get"]["description"],
        "Returns blog posts"
    );

    // Literal metadata is unaffected by the culture.
    assert_eq!(zh["paths"]["/api/search"]["get"]["summary"], "搜索");
}

#[test]
fn category_becomes_the_openapi_tag() {
    let zh = generate_openapi_spec("Test API", "1.0.0", &localizer("zh-CN"));
    assert_eq!(
        zh["paths"]["/api/blog"]["get"]["tags"],
        serde_json::json!(["博客模块"])
    );

    let en = generate_openapi_spec("Test API", "1.0.0", &localizer("en-US"));
    assert_eq!(
        en["paths"]["/api/blog"]["get"]["tags"],
        serde_json::json!(["Blog"])
    );

    // No category → the path-derived tag is kept.
    assert_eq!(
        zh["paths"]["/api/search"]["get"]["tags"],
        serde_json::json!(["Search"])
    );
}
