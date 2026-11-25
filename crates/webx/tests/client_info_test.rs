//! `#[fromclient] ClientInfo` binding: peer-only by default, trusted forwarded
//! resolution only behind configured proxies.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use webx::*;

#[derive(Debug, WebxRequestMeta)]
struct WhoAmIRequest {
    #[fromclient]
    client: ClientInfo,
}

#[get("/api/whoami")]
impl IRequest<WhoAmI> for WhoAmIRequest {}

#[derive(Debug, Serialize, Deserialize)]
struct WhoAmI {
    peer: Option<String>,
    client_ip: Option<String>,
    forwarded_for: Vec<String>,
}

#[derive(Default)]
struct WhoAmIHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<WhoAmIRequest, WhoAmI> for WhoAmIHandler {
    async fn handle(&mut self, req: WhoAmIRequest) -> ActionResult<WhoAmI> {
        Ok(WhoAmI {
            peer: req.client.peer_addr.map(|addr| addr.to_string()),
            client_ip: req.client.client_ip.map(|ip| ip.to_string()),
            forwarded_for: req
                .client
                .forwarded_for
                .iter()
                .map(ToString::to_string)
                .collect(),
        })
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

async fn spawn_host(port: u16, trust: bool, trusted_proxies: &[&str]) -> ServerHandle {
    let address = format!("127.0.0.1:{port}");
    let trusted: Vec<String> = trusted_proxies
        .iter()
        .map(|cidr| cidr.to_string())
        .collect();
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(move |app| {
            app.options(move |options| {
                options.client_info.trust_forwarded_headers = trust;
                options.client_info.trusted_proxies = trusted;
            });
        })
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    let base = format!("http://127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if reqwest::get(format!("{base}/health/live")).await.is_ok() {
            return handle;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host did not become ready");
}

async fn whoami(port: u16, forwarded_for: &str) -> WhoAmI {
    reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/api/whoami"))
        .header("x-forwarded-for", forwarded_for)
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forwarded_headers_are_ignored_without_trust() {
    let port = free_port();
    let handle = spawn_host(port, false, &[]).await;

    let body = whoami(port, "203.0.113.7").await;
    assert_eq!(body.client_ip.as_deref(), Some("127.0.0.1"));
    assert!(body.forwarded_for.is_empty());
    assert!(body
        .peer
        .as_deref()
        .is_some_and(|peer| peer.starts_with("127.0.0.1:")));

    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_trusted_proxy_resolves_the_rightmost_untrusted_hop() {
    let port = free_port();
    let handle = spawn_host(port, true, &["127.0.0.0/8"]).await;

    let body = whoami(port, "203.0.113.7, 127.0.0.1").await;
    assert_eq!(body.client_ip.as_deref(), Some("203.0.113.7"));
    assert_eq!(
        body.forwarded_for,
        vec!["203.0.113.7".to_string(), "127.0.0.1".to_string()]
    );

    handle.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headers_are_ignored_when_the_peer_is_not_a_trusted_proxy() {
    let port = free_port();
    // The test peer is 127.0.0.1, which is not inside 10.0.0.0/8.
    let handle = spawn_host(port, true, &["10.0.0.0/8"]).await;

    let body = whoami(port, "203.0.113.7").await;
    assert_eq!(body.client_ip.as_deref(), Some("127.0.0.1"));
    assert!(body.forwarded_for.is_empty());

    handle.shutdown();
}
