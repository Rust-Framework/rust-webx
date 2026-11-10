//! Endpoint catalog: resource keys, global culture, authorization rules, and
//! host-scoped access.

#![allow(non_snake_case)] // `#[derive(Inject)]` generates `__rdi_construct_<Type>` helpers.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use webx::*;

const RESOURCES_ZH: &str = r#"{
    "endpoint.blog.create.name": "博客模块-创建博客",
    "endpoint.blog.create.description": "创建一个博客草稿。",
    "endpoint.blog.category": "博客模块"
}"#;

const RESOURCES_EN: &str = r#"{
    "endpoint.blog.create.name": "Blog - Create",
    "endpoint.blog.create.description": "Create a blog draft.",
    "endpoint.blog.category": "Blog"
}"#;

#[derive(Default, Serialize, Deserialize, WebxRequestMeta)]
struct CreateBlogRequest;

#[post("/api/endpoints/blog")]
#[display(key = "endpoint.blog.create.name", text = "创建博客")]
#[description(key = "endpoint.blog.create.description", text = "创建一个博客草稿。")]
#[category(key = "endpoint.blog.category")]
#[authorize(role = "admin")]
#[authorize(permission = "blog:create")]
impl IRequest<String> for CreateBlogRequest {}

#[derive(Default)]
struct CreateBlogHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<CreateBlogRequest, String> for CreateBlogHandler {
    async fn handle(&mut self, _req: CreateBlogRequest) -> Result<String> {
        Ok("created".into())
    }
}

#[derive(Default, Serialize, Deserialize, WebxRequestMeta)]
struct GetBlogRequest {
    slug: String,
}

/// 文档回退-博客详情
///
/// 按 slug 读取博客。
#[get("/api/endpoints/blog/{slug}")]
impl IRequest<String> for GetBlogRequest {}

#[derive(Default)]
struct GetBlogHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<GetBlogRequest, String> for GetBlogHandler {
    async fn handle(&mut self, req: GetBlogRequest) -> Result<String> {
        Ok(req.slug)
    }
}

#[derive(Default, Serialize, Deserialize, WebxRequestMeta)]
struct PingRequest;

#[endpoint(Get, "/api/endpoints/ping")]
impl IRequest<String> for PingRequest {}

#[derive(Default)]
struct PingHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<PingRequest, String> for PingHandler {
    async fn handle(&mut self, _req: PingRequest) -> Result<String> {
        Ok("pong".into())
    }
}

#[derive(Default, Serialize, Deserialize, WebxRequestMeta)]
struct OrderRequest;

// Text attributes above the route macro: the `#[display]`-family macros carry
// their arguments to a hidden attribute, so order does not matter.
#[display("顺序无关-名称")]
#[description("顺序无关-描述")]
#[category("顺序测试")]
#[get("/api/endpoints/order")]
impl IRequest<String> for OrderRequest {}

#[derive(Default)]
struct OrderHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<OrderRequest, String> for OrderHandler {
    async fn handle(&mut self, _req: OrderRequest) -> Result<String> {
        Ok("ordered".into())
    }
}

/// Mirrors the documented "return all endpoints" handler: both services are
/// injected from DI.
#[derive(Inject)]
struct ListEndpointsHandler {
    #[inject]
    catalog: Arc<dyn IEndpointCatalog>,
    #[inject]
    localizer: Arc<dyn IStringLocalizer>,
}

fn localizer() -> StringLocalizer {
    let options = LocalizationOptions::none()
        .resources("zh-CN", RESOURCES_ZH)
        .resources("en", RESOURCES_EN);
    StringLocalizer::new("zh-CN", &options).unwrap()
}

fn find<'a>(endpoints: &'a [EndpointInfo], path: &str) -> &'a EndpointInfo {
    endpoints
        .iter()
        .find(|endpoint| endpoint.path == path)
        .unwrap_or_else(|| panic!("endpoint {path} not in catalog"))
}

#[test]
fn descriptors_carry_resource_keys_not_translations() {
    let catalog = EndpointCatalog::from_inventory();
    let descriptor = catalog.get("POST", "/api/endpoints/blog").unwrap();

    assert_eq!(descriptor.name.key, "endpoint.blog.create.name");
    assert_eq!(descriptor.name.text, "创建博客");
    assert_eq!(
        descriptor.description.key,
        "endpoint.blog.create.description"
    );
    assert_eq!(descriptor.category.key, "endpoint.blog.category");
    assert_eq!(descriptor.authorization.len(), 2);
}

#[test]
fn localized_view_uses_the_default_culture() {
    let catalog = EndpointCatalog::from_inventory();
    let endpoints = catalog.localized(&localizer());

    let create = find(&endpoints, "/api/endpoints/blog");
    assert_eq!(create.name, "博客模块-创建博客");
    assert_eq!(create.description, "创建一个博客草稿。");
    assert_eq!(create.category, "博客模块");

    // The English resource exists, but the default culture is zh-CN.
    assert_ne!(create.name, "Blog - Create");
}

#[test]
fn explicit_culture_overrides_the_default() {
    let catalog = EndpointCatalog::from_inventory();
    let endpoints = catalog.localized_for(&localizer(), "en-US");

    let create = find(&endpoints, "/api/endpoints/blog");
    assert_eq!(create.name, "Blog - Create");
    assert_eq!(create.description, "Create a blog draft.");
    assert_eq!(create.category, "Blog");
}

#[test]
fn doc_comment_is_the_fallback_when_no_display_attribute_exists() {
    let catalog = EndpointCatalog::from_inventory();
    let descriptor = catalog.get("GET", "/api/endpoints/blog/{slug}").unwrap();
    assert!(descriptor.name.key.is_empty());
    assert_eq!(descriptor.name.text, "文档回退-博客详情");

    let endpoints = catalog.localized(&localizer());
    let detail = find(&endpoints, "/api/endpoints/blog/{slug}");
    assert_eq!(detail.name, "文档回退-博客详情");
    assert_eq!(detail.category, "");
    assert_eq!(
        detail.description,
        "文档回退-博客详情\n\n按 slug 读取博客。"
    );
}

#[test]
fn text_attributes_may_precede_the_route_macro() {
    let catalog = EndpointCatalog::from_inventory();
    let descriptor = catalog.get("GET", "/api/endpoints/order").unwrap();
    assert_eq!(descriptor.name.text, "顺序无关-名称");
    assert_eq!(descriptor.description.text, "顺序无关-描述");
    assert_eq!(descriptor.category.text, "顺序测试");

    let endpoints = catalog.localized(&localizer());
    let order = find(&endpoints, "/api/endpoints/order");
    assert_eq!(order.name, "顺序无关-名称");
    assert_eq!(order.category, "顺序测试");
}

#[test]
fn name_falls_back_to_the_request_type() {
    let catalog = EndpointCatalog::from_inventory();
    let descriptor = catalog.get("GET", "/api/endpoints/ping").unwrap();
    assert!(descriptor.name.key.is_empty());
    assert_eq!(descriptor.name.text, "ping");
    assert!(descriptor.description.text.is_empty());
    assert!(descriptor.category.key.is_empty());
    assert!(descriptor.category.text.is_empty());
}

#[test]
fn catalog_serializes_to_method_path_name_description_and_rules() {
    let catalog = EndpointCatalog::from_inventory();
    let endpoints = catalog.localized(&localizer());
    let create = find(&endpoints, "/api/endpoints/blog");

    assert_eq!(
        serde_json::to_value(create).unwrap(),
        serde_json::json!({
            "method": "POST",
            "path": "/api/endpoints/blog",
            "name": "博客模块-创建博客",
            "description": "创建一个博客草稿。",
            "category": "博客模块",
            "authorization": [
                { "roles": ["admin"], "permissions": [] },
                { "roles": [], "permissions": ["blog:create"] },
            ],
        })
    );
}

#[test]
fn descriptors_serialize_with_resource_references() {
    let catalog = EndpointCatalog::from_inventory();
    let descriptor = catalog.get("POST", "/api/endpoints/blog").unwrap();
    let value = serde_json::to_value(descriptor).unwrap();

    assert_eq!(value["method"], "POST");
    assert_eq!(value["path"], "/api/endpoints/blog");
    assert_eq!(value["name"]["key"], "endpoint.blog.create.name");
    assert_eq!(value["category"]["key"], "endpoint.blog.category");
}

#[test]
fn catalog_is_sorted_by_path_then_method() {
    let catalog = EndpointCatalog::from_inventory();
    for pair in catalog.all().windows(2) {
        assert!(
            (pair[0].path, pair[0].method) <= (pair[1].path, pair[1].method),
            "{} {} came before {} {}",
            pair[0].method,
            pair[0].path,
            pair[1].method,
            pair[1].path
        );
    }
}

#[test]
fn host_exposes_the_catalog_and_uses_the_global_culture_config() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|builder| {
            builder.useOptions(|options| {
                options.culture.default = "zh-CN".to_string();
            });
        })
        .add_localization_with(
            LocalizationOptions::none()
                .resources("zh-CN", RESOURCES_ZH)
                .resources("en", RESOURCES_EN),
        )
        .build();

    let catalog = host.endpoints();
    assert_eq!(catalog.all().len(), 4);

    let create = catalog.get("POST", "/api/endpoints/blog").unwrap();
    assert_eq!(
        host.localizer().resolve(create.name).value,
        "博客模块-创建博客"
    );

    // Both services are resolvable from DI, without the Host handle.
    let injected = host
        .provider()
        .get::<dyn IEndpointCatalog>()
        .expect("IEndpointCatalog is registered in DI");
    assert_eq!(injected.all().len(), 4);

    let injected_localizer = host
        .provider()
        .get::<dyn IStringLocalizer>()
        .expect("IStringLocalizer is registered in DI");
    assert_eq!(injected_localizer.default_culture(), "zh-CN");

    // The documented handler-style injection works for both services.
    let handler = __rdi_try_construct_ListEndpointsHandler(host.provider().as_ref())
        .expect("handler dependencies resolve");
    assert_eq!(handler.catalog.all().len(), 4);
    assert_eq!(handler.localizer.default_culture(), "zh-CN");
}
