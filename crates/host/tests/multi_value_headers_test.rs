//! Multi-valued response headers (two `Set-Cookie`s) survive the host hop.

use std::net::TcpListener;
use std::ops::ControlFlow;
use std::sync::Arc;
use webx_host::MiddlewareSpec;

use webx_core::error::Result;
use webx_core::http::IHttpContext;
use webx_core::middleware::IMiddleware;
use webx_host::server::Host;

struct TwoCookies;

#[async_trait::async_trait]
impl IMiddleware for TwoCookies {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        ctx.response_mut()
            .append_header("set-cookie", "access=abc; Path=/; HttpOnly");
        ctx.response_mut()
            .append_header("set-cookie", "refresh=def; Path=/; HttpOnly");
        Ok(ControlFlow::Continue(()))
    }
}

fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

#[tokio::test]
async fn two_set_cookie_headers_are_preserved() {
    let port = find_free_port();
    let addr = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .no_spa()
        .use_middleware(MiddlewareSpec::factory(|| {
            Arc::new(TwoCookies) as Arc<dyn IMiddleware>
        }))
        .build();
    let base = format!("http://{addr}");
    tokio::spawn(async move {
        let _ = host.run_at(&addr).await;
    });

    let client = reqwest::Client::new();
    let mut resp = None;
    for _ in 0..100 {
        if let Ok(candidate) = client.get(format!("{base}/health/live")).send().await {
            resp = Some(candidate);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let resp = resp.expect("host did not start");
    let cookies: Vec<_> = resp.headers().get_all("set-cookie").iter().collect();
    assert_eq!(
        cookies.len(),
        2,
        "both Set-Cookie headers must reach the client, got {cookies:?}"
    );
    assert_eq!(cookies[0].to_str().unwrap(), "access=abc; Path=/; HttpOnly");
    assert_eq!(
        cookies[1].to_str().unwrap(),
        "refresh=def; Path=/; HttpOnly"
    );
}
