//! HostBuilder API tests — verifies middleware registration and pipeline wiring.

use std::net::TcpListener;
use std::ops::ControlFlow;
use std::sync::Arc;

use rust_webapp_core::http::IHttpContext;
use rust_webapp_core::middleware::IMiddleware;
use rust_webapp_host::server::Host;

fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

struct MarkerMiddleware;

impl Default for MarkerMiddleware {
    fn default() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl IMiddleware for MarkerMiddleware {
    async fn invoke(
        &self,
        ctx: &mut dyn IHttpContext,
    ) -> rust_webapp_core::error::Result<ControlFlow<()>> {
        ctx.response_mut().set_header("x-marker", "yes");
        Ok(ControlFlow::Continue(()))
    }
}

#[tokio::test]
async fn use_middleware_registers_into_pipeline() {
    let port = find_free_port();
    let addr = format!("127.0.0.1:{}", port);
    let host = Host::builder()
        .mode(rust_webapp_core::mode::AppMode::Development)
        .no_spa()
        .use_middleware::<MarkerMiddleware>()
        .build();
    tokio::spawn(async move { host.run_at(&addr).await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.headers().get("x-marker").unwrap(), "yes");
}

#[tokio::test]
async fn use_middleware_with_registers_into_pipeline() {
    use rust_webapp_core::http::IHttpContext;
    use rust_webapp_core::middleware::IMiddleware;
    use std::ops::ControlFlow;

    struct TagMiddleware;
    #[async_trait::async_trait]
    impl IMiddleware for TagMiddleware {
        async fn invoke(
            &self,
            ctx: &mut dyn IHttpContext,
        ) -> rust_webapp_core::error::Result<ControlFlow<()>> {
            ctx.response_mut().set_header("x-via-with", "1");
            Ok(ControlFlow::Continue(()))
        }
    }

    let port = find_free_port();
    let addr = format!("127.0.0.1:{}", port);
    let host = Host::builder()
        .mode(rust_webapp_core::mode::AppMode::Development)
        .no_spa()
        .use_middleware_with(|| Arc::new(TagMiddleware) as Arc<dyn IMiddleware>)
        .build();
    tokio::spawn(async move { host.run_at(&addr).await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.headers().get("x-via-with").unwrap(), "1");
}
