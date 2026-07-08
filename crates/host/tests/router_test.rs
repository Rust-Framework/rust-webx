mod test_utils;

use rust_webx_core::error::Result as LrwfResult;
use rust_webx_core::http::IHttpContext;
use rust_webx_core::routing::{HttpMethod, IEndpoint, IRouter};
use rust_webx_host::router::Router;
use std::sync::Arc;

struct TestEndpoint {
    #[allow(dead_code)]
    label: &'static str,
}

#[async_trait::async_trait]
impl IEndpoint for TestEndpoint {
    async fn handle(&self, _ctx: &mut dyn IHttpContext) -> LrwfResult<()> {
        Ok(())
    }
}

#[tokio::test]
async fn router_exact_match() {
    let mut router = Router::new();
    router.register(
        HttpMethod::Get,
        "/hello",
        Arc::new(TestEndpoint { label: "hello" }),
    );

    let mut ctx = test_utils::TestHttpContext::new("GET", "/hello");
    let result = router.match_route(&mut ctx).await.unwrap();
    assert!(result.is_some(), "Should match /hello exactly");
}

#[tokio::test]
async fn router_path_parameter_extraction() {
    let mut router = Router::new();
    router.register(
        HttpMethod::Get,
        "/users/{id}",
        Arc::new(TestEndpoint {
            label: "user_by_id",
        }),
    );

    let mut ctx = test_utils::TestHttpContext::new("GET", "/users/42");
    let result = router.match_route(&mut ctx).await.unwrap();
    assert!(result.is_some());
    let (_endpoint, params, pattern) = result.unwrap();
    assert_eq!(params.get("id").unwrap(), "42");
    assert_eq!(pattern, "/users/{id}");
}

#[tokio::test]
async fn router_multi_segment_params() {
    let mut router = Router::new();
    router.register(
        HttpMethod::Get,
        "/a/{x}/b/{y}",
        Arc::new(TestEndpoint { label: "multi" }),
    );

    let mut ctx = test_utils::TestHttpContext::new("GET", "/a/1/b/2");
    let result = router.match_route(&mut ctx).await.unwrap();
    assert!(result.is_some());
    let (_endpoint, params, _pattern) = result.unwrap();
    assert_eq!(params.get("x").unwrap(), "1");
    assert_eq!(params.get("y").unwrap(), "2");
}

#[tokio::test]
async fn router_http_method_distinction() {
    let mut router = Router::new();
    router.register(
        HttpMethod::Get,
        "/resource",
        Arc::new(TestEndpoint {
            label: "get_resource",
        }),
    );

    let mut ctx = test_utils::TestHttpContext::new("POST", "/resource");
    let result = router.match_route(&mut ctx).await.unwrap();
    assert!(result.is_none(), "POST should NOT match GET-only route");
}

#[tokio::test]
async fn router_route_not_found() {
    let router = Router::new();
    let mut ctx = test_utils::TestHttpContext::new("GET", "/nonexistent");
    let result = router.match_route(&mut ctx).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn router_multiple_methods_same_path() {
    let mut router = Router::new();
    router.register(
        HttpMethod::Get,
        "/items",
        Arc::new(TestEndpoint { label: "get_items" }),
    );
    router.register(
        HttpMethod::Post,
        "/items",
        Arc::new(TestEndpoint {
            label: "post_items",
        }),
    );

    let mut ctx = test_utils::TestHttpContext::new("GET", "/items");
    assert!(router.match_route(&mut ctx).await.unwrap().is_some());

    let mut ctx = test_utils::TestHttpContext::new("POST", "/items");
    assert!(router.match_route(&mut ctx).await.unwrap().is_some());
}

#[tokio::test]
async fn router_nested_routes() {
    let mut router = Router::new();
    router.register(
        HttpMethod::Get,
        "/api/users",
        Arc::new(TestEndpoint { label: "list" }),
    );
    router.register(
        HttpMethod::Get,
        "/api/users/{id}",
        Arc::new(TestEndpoint { label: "detail" }),
    );

    let mut ctx = test_utils::TestHttpContext::new("GET", "/api/users/99");
    let result = router.match_route(&mut ctx).await.unwrap();
    assert!(result.is_some());
    let (_endpoint, params, pattern) = result.unwrap();
    assert_eq!(params.get("id").unwrap(), "99");
    assert_eq!(pattern, "/api/users/{id}");
}

#[tokio::test]
async fn router_root_path_match() {
    let mut router = Router::new();
    router.register(
        HttpMethod::Get,
        "/",
        Arc::new(TestEndpoint { label: "root" }),
    );

    let mut ctx = test_utils::TestHttpContext::new("GET", "/");
    let result = router.match_route(&mut ctx).await.unwrap();
    assert!(result.is_some());
}

#[tokio::test]
async fn router_wildcard_match() {
    let mut router = Router::new();
    // matchit 0.9 uses {*any} syntax for wildcards
    router.register(
        HttpMethod::Get,
        "/api/{*any}",
        Arc::new(TestEndpoint { label: "wildcard" }),
    );

    let mut ctx = test_utils::TestHttpContext::new("GET", "/api/admin/dashboard");
    let result = router.match_route(&mut ctx).await.unwrap();
    assert!(result.is_some());
    let (_endpoint, _params, pattern) = result.unwrap();
    assert_eq!(pattern, "/api/{*any}");
}

#[tokio::test]
async fn router_static_over_dynamic() {
    let mut router = Router::new();
    router.register(
        HttpMethod::Get,
        "/api/users/me",
        Arc::new(TestEndpoint { label: "me" }),
    );
    router.register(
        HttpMethod::Get,
        "/api/users/{id}",
        Arc::new(TestEndpoint {
            label: "user_by_id",
        }),
    );

    // "me" should match statically, not {id}="me"
    let mut ctx = test_utils::TestHttpContext::new("GET", "/api/users/me");
    let result = router.match_route(&mut ctx).await.unwrap();
    assert!(result.is_some());
    let (_endpoint, params, pattern) = result.unwrap();
    assert_eq!(pattern, "/api/users/me");
    assert!(!params.contains_key("id"));
}
