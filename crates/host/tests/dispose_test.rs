//! Host shutdown disposes the rust-dix provider (singletons released once).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use webx_core::mode::AppMode;
use webx_host::server::Host;

#[derive(Debug)]
struct Disposable {
    drops: Arc<AtomicUsize>,
}

impl Drop for Disposable {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.local_addr().expect("addr").port()
}

#[tokio::test]
async fn shutdown_disposes_the_di_provider_once() {
    let drops = Arc::new(AtomicUsize::new(0));

    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .register({
            let drops = Arc::clone(&drops);
            move |svc| {
                svc.singleton(move |_| {
                    Arc::new(Disposable {
                        drops: Arc::clone(&drops),
                    })
                })
            }
        })
        .try_build()
        .expect("host must build");

    let provider = Arc::clone(host.provider());
    let handle = host.server_handle();
    let addr = format!("127.0.0.1:{}", free_port());

    let server = tokio::spawn(async move { host.run_at(&addr).await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    handle.shutdown();
    server
        .await
        .expect("server task must finish")
        .expect("server must shut down cleanly");

    assert_eq!(
        drops.load(Ordering::SeqCst),
        1,
        "the provider must release its singleton exactly once"
    );

    let error = provider
        .get::<Disposable>()
        .expect_err("a disposed provider must not resolve services");
    assert!(
        error.to_string().to_lowercase().contains("disposed"),
        "the error must report ProviderDisposed, got: {error}"
    );
}
