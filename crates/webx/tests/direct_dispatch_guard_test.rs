//! 2.4-9 closure: route guards are never bypassed by the direct fast path.
//!
//! The direct dispatch path is selected only for routes without declarative
//! authorization, dynamic authorization or a per-route body cap. This test pins
//! the observable contract: an unauthenticated request to a guarded route is
//! `401` (not a silent handler call), a token without the role is `403`, and a
//! token with the role is `200` — with no typed pipeline behavior registered,
//! which is exactly when the fast path is active elsewhere.

use std::net::TcpListener;

use serde::{Deserialize, Serialize};
use webx::*;

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

async fn wait_ready(port: u16) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("host did not become ready");
}

#[derive(Debug, Serialize, Deserialize)]
struct GuardedView {
    ok: bool,
}

#[derive(Default, WebxRequestMeta)]
struct GuardedRequest;

#[get("/guarded")]
#[authorize(role = "admin")]
impl IRequest<GuardedView> for GuardedRequest {}

#[derive(Default)]
struct GuardedHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<GuardedRequest, GuardedView> for GuardedHandler {
    async fn handle(&mut self, _req: GuardedRequest) -> ActionResult<GuardedView> {
        Ok(GuardedView { ok: true })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn route_authorization_is_enforced_on_the_direct_dispatch_host() {
    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.options(|options| {
                options.jwt.secret = "direct-guard-test-secret-at-least-32-bytes".to_string();
            });
        })
        .use_authentication()
        .try_build()
        .expect("host must build");

    let issuer = host
        .provider()
        .get::<dyn ITokenIssuer>()
        .expect("add_authentication registers the default issuer");
    let admin = issuer
        .issue(TokenClaims::new("admin-user").with_roles(["admin"]))
        .await
        .expect("issue admin token");
    let member = issuer
        .issue(TokenClaims::new("member-user").with_roles(["member"]))
        .await
        .expect("issue member token");

    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;

    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let anonymous = client
        .get(format!("{base}/guarded"))
        .send()
        .await
        .expect("anonymous request");
    assert_eq!(anonymous.status(), reqwest::StatusCode::UNAUTHORIZED);

    let forbidden = client
        .get(format!("{base}/guarded"))
        .bearer_auth(&member.token)
        .send()
        .await
        .expect("member request");
    assert_eq!(forbidden.status(), reqwest::StatusCode::FORBIDDEN);

    let allowed = client
        .get(format!("{base}/guarded"))
        .bearer_auth(&admin.token)
        .send()
        .await
        .expect("admin request");
    assert_eq!(allowed.status(), reqwest::StatusCode::OK);
    let view: GuardedView = allowed.json().await.expect("guarded body");
    assert!(view.ok);

    handle.shutdown();
}
