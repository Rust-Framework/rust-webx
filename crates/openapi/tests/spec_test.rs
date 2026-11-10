//! OpenAPI spec generation tests.

use std::any::TypeId;

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

/// Two same-named request types in different modules: only `TypeId`
/// distinguishes them (the bare type name is identical).
mod contracts {
    pub struct ListOrdersRequest;
}

mod billing {
    pub struct ListOrdersRequest;
}

/// Stand-in for a request type whose parameters live in a *qualified* path
/// (`contracts::ListOrdersRequest`): the endpoint macro records the qualified
/// name, while the derive records the bare name.
mod qualified {
    pub struct QualifierRequest;
}

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

// A write endpoint: its request body fields become documented properties.
inventory::submit! {
    RouteEntry::new(
        HttpMethod::Post,
        "/api/blog",
        "CreateBlogRequest",
        "String",
        TextRef::text("创建博客"),
        TextRef::text("创建一篇博客"),
        TextRef::key("endpoint.blog.category"),
        &[],
        &[],
    )
}

// A write endpoint whose body has no required fields.
inventory::submit! {
    RouteEntry::new(
        HttpMethod::Put,
        "/api/settings",
        "UpdateSettingsRequest",
        "String",
        TextRef::text("更新设置"),
        TextRef::text("更新用户设置"),
        TextRef::text(""),
        &[],
        &[],
    )
}

// A read endpoint that (wrongly) declares body-only fields: the runtime cannot
// bind them, so the spec must not advertise a request body either.
inventory::submit! {
    RouteEntry::new(
        HttpMethod::Get,
        "/api/legacy-body",
        "LegacyBodyRequest",
        "String",
        TextRef::text("旧端点"),
        TextRef::text(""),
        TextRef::text(""),
        &[],
        &[],
    )
}

// A qualified `self_ty`: the route records `qualified::QualifierRequest`, while
// the derive records the bare `QualifierRequest` — a name lookup would miss.
inventory::submit! {
    RouteEntry::with_type_id(
        HttpMethod::Get,
        "/api/qualified",
        TypeId::of::<qualified::QualifierRequest>(),
        "qualified::QualifierRequest",
        "String",
        TextRef::text("限定路径"),
        TextRef::text(""),
        TextRef::text(""),
        &[],
        &[],
    )
}

// Two routes whose request types share the bare name `ListOrdersRequest`; only
// the `TypeId` tells them apart.
inventory::submit! {
    RouteEntry::with_type_id(
        HttpMethod::Get,
        "/api/contracts/orders",
        TypeId::of::<contracts::ListOrdersRequest>(),
        "contracts::ListOrdersRequest",
        "String",
        TextRef::text("合同订单"),
        TextRef::text(""),
        TextRef::text(""),
        &[],
        &[],
    )
}

inventory::submit! {
    RouteEntry::with_type_id(
        HttpMethod::Get,
        "/api/billing/orders",
        TypeId::of::<billing::ListOrdersRequest>(),
        "billing::ListOrdersRequest",
        "String",
        TextRef::text("账单订单"),
        TextRef::text(""),
        TextRef::text(""),
        &[],
        &[],
    )
}

inventory::submit! {
    RequestParamEntry {
        request_type: "QualifierRequest",
        req_type_id: TypeId::of::<qualified::QualifierRequest>(),
        params: &[ParamMeta {
            name: "term",
            source: "query",
            type_hint: "string",
            required: true,
            json_key: None,
        }],
    }
}

inventory::submit! {
    RequestParamEntry {
        request_type: "ListOrdersRequest",
        req_type_id: TypeId::of::<contracts::ListOrdersRequest>(),
        params: &[ParamMeta {
            name: "contract",
            source: "query",
            type_hint: "string",
            required: false,
            json_key: None,
        }],
    }
}

inventory::submit! {
    RequestParamEntry {
        request_type: "ListOrdersRequest",
        req_type_id: TypeId::of::<billing::ListOrdersRequest>(),
        params: &[ParamMeta {
            name: "invoice",
            source: "query",
            type_hint: "string",
            required: false,
            json_key: None,
        }],
    }
}

inventory::submit! {
    RequestParamEntry {
        request_type: "SearchRequest",
        // Hand-written entry without a type identity: falls back to the name.
        req_type_id: TypeId::of::<()>(),
        params: &[
            ParamMeta {
                name: "q",
                source: "query",
                type_hint: "string",
                required: false,
                json_key: None,
            },
            ParamMeta {
                name: "page",
                source: "query",
                type_hint: "integer",
                required: false,
                json_key: None,
            },
        ],
    }
}

inventory::submit! {
    RequestParamEntry {
        request_type: "CreateBlogRequest",
        req_type_id: TypeId::of::<()>(),
        params: &[
            ParamMeta {
                name: "title",
                source: "body",
                type_hint: "string",
                required: true,
                json_key: Some("title"),
            },
            ParamMeta {
                name: "published",
                source: "body",
                type_hint: "boolean",
                required: false,
                json_key: Some("published"),
            },
            ParamMeta {
                name: "tags",
                source: "body",
                type_hint: "array",
                required: false,
                json_key: Some("tags"),
            },
            // The struct field is `display_name`; the JSON key is renamed.
            ParamMeta {
                name: "display_name",
                source: "body",
                type_hint: "string",
                required: true,
                json_key: Some("displayName"),
            },
            // An unmarked field: the route method (POST) resolves it to the body.
            ParamMeta {
                name: "slug",
                source: "",
                type_hint: "string",
                required: false,
                json_key: Some("slug"),
            },
            // A `#[serde(flatten)]` field: no key of its own.
            ParamMeta {
                name: "meta",
                source: "body",
                type_hint: "object",
                required: false,
                json_key: None,
            },
        ],
    }
}

inventory::submit! {
    RequestParamEntry {
        request_type: "UpdateSettingsRequest",
        req_type_id: TypeId::of::<()>(),
        params: &[
            ParamMeta {
                name: "theme",
                source: "body",
                type_hint: "string",
                required: false,
                json_key: Some("theme"),
            },
            ParamMeta {
                name: "notify",
                source: "body",
                type_hint: "boolean",
                required: false,
                json_key: Some("notify"),
            },
        ],
    }
}

inventory::submit! {
    RequestParamEntry {
        request_type: "LegacyBodyRequest",
        req_type_id: TypeId::of::<()>(),
        params: &[ParamMeta {
            name: "payload",
            source: "body",
            type_hint: "object",
            required: true,
            json_key: None,
        }],
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

// ---------------------------------------------------------------------------
// JSON request body
// ---------------------------------------------------------------------------

#[test]
fn request_body_documents_field_properties_and_requiredness() {
    let spec = generate_openapi_spec("Test API", "1.0.0", &localizer("zh-CN"));
    let request_body = &spec["paths"]["/api/blog"]["post"]["requestBody"];

    assert_eq!(request_body["required"], true);
    let schema = &request_body["content"]["application/json"]["schema"];
    assert_eq!(schema["type"], "object", "got {request_body}");

    // Every field becomes a property with its JSON type.
    assert_eq!(schema["properties"]["title"]["type"], "string");
    assert_eq!(schema["properties"]["published"]["type"], "boolean");
    assert_eq!(schema["properties"]["tags"]["type"], "array");
    // The JSON key wins over the Rust field name (`display_name` → `displayName`).
    assert_eq!(schema["properties"]["displayName"]["type"], "string");
    assert!(schema["properties"]["display_name"].is_null());
    // An unmarked field on a write method is a body property too.
    assert_eq!(schema["properties"]["slug"]["type"], "string");
    // A flattened field has no property of its own; unknown keys stay allowed.
    assert!(schema["properties"]["meta"].is_null());
    assert_eq!(schema["additionalProperties"], true);

    // Requiredness comes from the fields, not from a constant.
    assert_eq!(
        schema["required"],
        serde_json::json!(["title", "displayName"])
    );

    // The JSON endpoint must not advertise a multipart body.
    assert!(request_body["content"]["multipart/form-data"].is_null());
}

#[test]
fn request_body_required_follows_the_fields() {
    let spec = generate_openapi_spec("Test API", "1.0.0", &localizer("zh-CN"));
    let request_body = &spec["paths"]["/api/settings"]["put"]["requestBody"];

    // No required body field → the body itself is optional.
    assert_eq!(request_body["required"], false);
    let schema = &request_body["content"]["application/json"]["schema"];
    assert_eq!(schema["properties"]["theme"]["type"], "string");
    assert_eq!(schema["properties"]["notify"]["type"], "boolean");
    assert_eq!(schema["required"], serde_json::json!([]));
}

#[test]
fn read_methods_do_not_advertise_a_request_body() {
    let spec = generate_openapi_spec("Test API", "1.0.0", &localizer("zh-CN"));
    let operation = &spec["paths"]["/api/legacy-body"]["get"];

    // A GET cannot bind a JSON body, so neither a request body nor the
    // body-only fields are documented.
    assert!(operation["requestBody"].is_null());
    assert!(operation["parameters"].is_null());
}

// ---------------------------------------------------------------------------
// Parameter metadata is keyed by `TypeId`
// ---------------------------------------------------------------------------

fn parameter_names(operation: &serde_json::Value) -> Vec<String> {
    operation["parameters"]
        .as_array()
        .map(|parameters| {
            parameters
                .iter()
                .map(|param| param["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn qualified_request_types_keep_their_parameter_metadata() {
    let spec = generate_openapi_spec("Test API", "1.0.0", &localizer("zh-CN"));

    // The derive records the bare name (`QualifierRequest`) while the route
    // records `qualified::QualifierRequest`: only the `TypeId` resolves it.
    let operation = &spec["paths"]["/api/qualified"]["get"];
    assert_eq!(parameter_names(operation), vec!["term"]);
    assert_eq!(operation["parameters"][0]["required"], true);
}

#[test]
fn same_named_request_types_do_not_share_parameters() {
    let spec = generate_openapi_spec("Test API", "1.0.0", &localizer("zh-CN"));

    // Two different types are both named `ListOrdersRequest`; each route must
    // keep its own parameters instead of one silently overwriting the other.
    assert_eq!(
        parameter_names(&spec["paths"]["/api/contracts/orders"]["get"]),
        vec!["contract"]
    );
    assert_eq!(
        parameter_names(&spec["paths"]["/api/billing/orders"]["get"]),
        vec!["invoice"]
    );
}
