mod test_utils;

use rust_webapp_core::middleware::IMiddleware;
use rust_webapp_host::cors::{CorsConfig, CorsMiddleware};

fn find_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

#[tokio::test]
async fn cors_wildcard_sets_star_origin() {
    let config = CorsConfig::default();
    let middleware = CorsMiddleware::new(config);
    let mut ctx = test_utils::TestHttpContext::new("GET", "/api/data")
        .with_header("origin", "https://example.com");

    middleware.invoke(&mut ctx).await.unwrap();

    let (_status, headers, _body) = ctx.into_response_parts();
    assert_eq!(
        find_header(&headers, "access-control-allow-origin").unwrap(),
        "*"
    );
}

#[tokio::test]
async fn cors_exact_origin_match() {
    let config = CorsConfig {
        origins: vec!["https://myapp.com".to_string()],
        ..Default::default()
    };

    let middleware = CorsMiddleware::new(config);
    let mut ctx = test_utils::TestHttpContext::new("GET", "/api/data")
        .with_header("origin", "https://myapp.com");

    middleware.invoke(&mut ctx).await.unwrap();

    let (_status, headers, _body) = ctx.into_response_parts();
    assert_eq!(
        find_header(&headers, "access-control-allow-origin").unwrap(),
        "https://myapp.com"
    );
}

#[tokio::test]
async fn cors_disallowed_origin_no_header() {
    let config = CorsConfig {
        origins: vec!["https://myapp.com".to_string()],
        ..Default::default()
    };

    let middleware = CorsMiddleware::new(config);
    let mut ctx = test_utils::TestHttpContext::new("GET", "/api/data")
        .with_header("origin", "https://evil.com");

    middleware.invoke(&mut ctx).await.unwrap();

    let (_status, headers, _body) = ctx.into_response_parts();
    assert!(
        find_header(&headers, "access-control-allow-origin").is_none(),
        "Disallowed origin should NOT get CORS header"
    );
}

#[tokio::test]
async fn cors_preflight_options_request() {
    let config = CorsConfig::default();
    let middleware = CorsMiddleware::new(config);
    let mut ctx = test_utils::TestHttpContext::new("OPTIONS", "/api/data")
        .with_header("origin", "https://example.com");

    middleware.invoke(&mut ctx).await.unwrap();

    let (status, headers, _body) = ctx.into_response_parts();
    assert_eq!(status, 204);
    assert!(find_header(&headers, "access-control-allow-methods").is_some());
    assert!(find_header(&headers, "access-control-allow-headers").is_some());
    assert!(find_header(&headers, "access-control-max-age").is_some());
}

#[tokio::test]
async fn cors_no_origin_header_no_cors_response() {
    let config = CorsConfig {
        origins: vec!["https://specific.com".to_string()],
        ..Default::default()
    };

    let middleware = CorsMiddleware::new(config);
    let mut ctx = test_utils::TestHttpContext::new("GET", "/api/data");

    middleware.invoke(&mut ctx).await.unwrap();

    let (_status, headers, _body) = ctx.into_response_parts();
    assert!(
        find_header(&headers, "access-control-allow-origin").is_none(),
        "Request without Origin should not get CORS for non-wildcard config"
    );
}

#[tokio::test]
async fn cors_credentials_allowed() {
    let config = CorsConfig {
        allow_credentials: true,
        ..Default::default()
    };

    let middleware = CorsMiddleware::new(config);
    let mut ctx = test_utils::TestHttpContext::new("GET", "/api/data")
        .with_header("origin", "https://example.com");

    middleware.invoke(&mut ctx).await.unwrap();

    let (_status, headers, _body) = ctx.into_response_parts();
    assert_eq!(
        find_header(&headers, "access-control-allow-credentials").unwrap(),
        "true"
    );
}

#[tokio::test]
async fn cors_credentials_not_set_when_disabled() {
    let config = CorsConfig::default();

    let middleware = CorsMiddleware::new(config);
    let mut ctx = test_utils::TestHttpContext::new("GET", "/api/data")
        .with_header("origin", "https://example.com");

    middleware.invoke(&mut ctx).await.unwrap();

    let (_status, headers, _body) = ctx.into_response_parts();
    assert!(find_header(&headers, "access-control-allow-credentials").is_none());
}
