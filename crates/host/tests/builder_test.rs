//! HostBuilder API tests — verifies middleware registration and pipeline wiring.

use std::net::TcpListener;
use std::ops::ControlFlow;
use std::sync::Arc;

use webx_core::http::IHttpContext;
use webx_core::middleware::IMiddleware;
use webx_core::options_monitor::{IOptionsMonitor, OptionsMonitor};
use webx_host::server::Host;
use webx_host::MiddlewareSpec;

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
    ) -> webx_core::error::Result<ControlFlow<()>> {
        ctx.response_mut().set_header("x-marker", "yes");
        Ok(ControlFlow::Continue(()))
    }
}

#[tokio::test]
async fn use_middleware_registers_into_pipeline() {
    let port = find_free_port();
    let addr = format!("127.0.0.1:{}", port);
    let host = Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .no_spa()
        .use_middleware(MiddlewareSpec::new::<MarkerMiddleware>())
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
    use std::ops::ControlFlow;
    use webx_core::http::IHttpContext;
    use webx_core::middleware::IMiddleware;

    struct TagMiddleware;
    #[async_trait::async_trait]
    impl IMiddleware for TagMiddleware {
        async fn invoke(
            &self,
            ctx: &mut dyn IHttpContext,
        ) -> webx_core::error::Result<ControlFlow<()>> {
            ctx.response_mut().set_header("x-via-with", "1");
            Ok(ControlFlow::Continue(()))
        }
    }

    let port = find_free_port();
    let addr = format!("127.0.0.1:{}", port);
    let host = Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .no_spa()
        .use_middleware(MiddlewareSpec::factory(|| {
            Arc::new(TagMiddleware) as Arc<dyn IMiddleware>
        }))
        .build();
    tokio::spawn(async move { host.run_at(&addr).await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.headers().get("x-via-with").unwrap(), "1");
}

// ---------------------------------------------------------------------------
// add_options: one binding, three registrations (API convergence #9)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, serde::Deserialize)]
struct SiteConfig {
    site_name: String,
}

#[test]
fn add_options_registers_the_snapshot_the_monitor_and_the_port() {
    let host = Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .no_spa()
        .add_options::<SiteConfig>("Site")
        .try_build()
        .expect("host must build");
    let provider = host.provider();

    let snapshot = provider
        .get::<SiteConfig>()
        .expect("add_options registers `Arc<T>`");
    let monitor = provider
        .get::<OptionsMonitor<SiteConfig>>()
        .expect("add_options registers `Arc<OptionsMonitor<T>>`");
    let port = provider
        .get::<dyn IOptionsMonitor<SiteConfig>>()
        .expect("add_options registers `Arc<dyn IOptionsMonitor<T>>`");

    // The `Site` section is absent here, so the bound value is `Default`.
    let startup = SiteConfig::default();
    assert_eq!(snapshot.site_name, startup.site_name);
    assert!(
        Arc::ptr_eq(&snapshot, &monitor.current()),
        "the snapshot and the monitor must start from the same bound value"
    );
    assert!(
        Arc::ptr_eq(&monitor.current(), &port.current()),
        "the port must observe the monitor that was registered"
    );

    // No watcher is configured: only an explicit publication moves the monitor,
    // and the injected `Arc<T>` keeps the startup snapshot.
    let previous = monitor.set(SiteConfig {
        site_name: "reloaded".to_string(),
    });
    assert_eq!(previous.site_name, startup.site_name);
    assert_eq!(port.current().site_name, "reloaded");
    assert_eq!(
        snapshot.site_name, startup.site_name,
        "`Arc<T>` is a frozen startup snapshot, not a live view"
    );
}

/// `watch_options_file` goes through the same registration path, so the three
/// instances exist there too and the poller updates the registered monitor.
#[test]
fn watch_options_file_registers_the_same_three_instances() {
    let dir = tempfile::tempdir().expect("temp dir");
    let host = Host::builder()
        .mode(webx_core::mode::AppMode::Development)
        .no_spa()
        .watch_options_file::<SiteConfig>(
            "Site",
            dir.path().join("site.json"),
            std::time::Duration::from_millis(50),
        )
        .try_build()
        .expect("host must build");
    let provider = host.provider();

    assert!(
        provider.get::<SiteConfig>().is_ok(),
        "watch_options_file registers `Arc<T>` too"
    );
    let monitor = provider
        .get::<OptionsMonitor<SiteConfig>>()
        .expect("watch_options_file registers `Arc<OptionsMonitor<T>>`");
    let port = provider
        .get::<dyn IOptionsMonitor<SiteConfig>>()
        .expect("watch_options_file registers `Arc<dyn IOptionsMonitor<T>>`");
    assert!(
        Arc::ptr_eq(&monitor.current(), &port.current()),
        "the poller and the port must share one monitor"
    );
}
