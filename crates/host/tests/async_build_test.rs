//! `HostBuilder::try_build_async` initialises rust-dix async singletons.

use std::sync::Arc;

use webx_core::mode::AppMode;
use webx_host::server::Host;

#[derive(Debug, PartialEq)]
struct AsyncConfig(String);

fn async_provider_builder() -> webx_host::server::HostBuilder {
    Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register(|svc| {
            svc.async_singleton(|_provider| {
                Box::pin(async {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    Arc::new(AsyncConfig("ready".to_string()))
                })
            })
        })
}

#[tokio::test]
async fn try_build_async_initializes_async_singletons() {
    let host = async_provider_builder()
        .try_build_async()
        .await
        .expect("async host build must succeed");

    let config: Arc<AsyncConfig> = host
        .provider()
        .get_async::<AsyncConfig>()
        .await
        .expect("the async singleton must be initialised by build_async");
    assert_eq!(config.as_ref(), &AsyncConfig("ready".to_string()));
}

#[test]
fn try_build_rejects_async_registrations() {
    let error = match async_provider_builder().try_build() {
        Ok(_) => panic!("sync build must reject async registrations"),
        Err(error) => error,
    };

    let message = error.to_string();
    assert!(
        message.to_lowercase().contains("async"),
        "the error must point at async registrations and build_async, got: {message}"
    );
}
