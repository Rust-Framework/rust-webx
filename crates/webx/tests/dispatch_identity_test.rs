//! Regression tests for identity-based dispatch and attribute-order-safe
//! authorization.
//!
//! Two request types deliberately share the bare name `DetailRequest` in
//! different modules. Before the fix, the host keyed its dispatch map by the
//! request type *name*, so both routes ran whichever handler the linker
//! happened to register last. Now the map is keyed by `TypeId`.

use std::net::TcpListener;

use webx::*;

fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

async fn spawn_host() -> String {
    let port = find_free_port();
    let addr = format!("127.0.0.1:{port}");
    let host = Host::builder().mode(AppMode::Development).no_spa().build();
    let base = format!("http://{addr}");
    tokio::spawn(async move {
        let _ = host.run_at(&addr).await;
    });
    let client = reqwest::Client::new();
    for _ in 0..100 {
        if client
            .get(format!("{base}/health/live"))
            .send()
            .await
            .is_ok()
        {
            return base;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("host did not start");
}

pub mod alpha {
    use super::*;

    #[derive(Default, WebxRequestMeta)]
    pub struct DetailRequest;

    #[get("/identity/alpha")]
    impl IRequest<String> for DetailRequest {}

    #[derive(Default)]
    pub struct DetailHandler;

    #[handler]
    #[async_trait::async_trait]
    impl IRequestHandler<DetailRequest, String> for DetailHandler {
        async fn handle(&mut self, _req: DetailRequest) -> ActionResult<String> {
            Ok("ALPHA".to_string())
        }
    }
}

pub mod beta {
    use super::*;

    #[derive(Default, WebxRequestMeta)]
    pub struct DetailRequest;

    #[get("/identity/beta")]
    impl IRequest<String> for DetailRequest {}

    #[derive(Default)]
    pub struct DetailHandler;

    #[handler]
    #[async_trait::async_trait]
    impl IRequestHandler<DetailRequest, String> for DetailHandler {
        async fn handle(&mut self, _req: DetailRequest) -> ActionResult<String> {
            Ok("BETA".to_string())
        }
    }
}

#[derive(Default, WebxRequestMeta)]
pub struct AdminAboveRequest;

/// The security declaration is written ABOVE the route macro on purpose.
#[authorize(role = "admin")]
#[get("/identity/admin-above")]
impl IRequest<String> for AdminAboveRequest {}

#[derive(Default)]
pub struct AdminAboveHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<AdminAboveRequest, String> for AdminAboveHandler {
    async fn handle(&mut self, _req: AdminAboveRequest) -> ActionResult<String> {
        Ok("ADMIN".to_string())
    }
}

#[derive(Default, WebxRequestMeta)]
pub struct AdminBelowRequest;

#[get("/identity/admin-below")]
#[authorize(role = "admin")]
impl IRequest<String> for AdminBelowRequest {}

#[derive(Default)]
pub struct AdminBelowHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<AdminBelowRequest, String> for AdminBelowHandler {
    async fn handle(&mut self, _req: AdminBelowRequest) -> ActionResult<String> {
        Ok("ADMIN".to_string())
    }
}

#[derive(Default, WebxRequestMeta)]
pub struct PanicRequest;

#[get("/identity/panic")]
impl IRequest<String> for PanicRequest {}

#[derive(Default)]
pub struct PanicHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<PanicRequest, String> for PanicHandler {
    async fn handle(&mut self, _req: PanicRequest) -> ActionResult<String> {
        panic!("intentional handler panic")
    }
}

#[tokio::test]
async fn same_named_request_types_dispatch_to_their_own_handlers() {
    let base = spawn_host().await;
    let alpha: String = reqwest::get(format!("{base}/identity/alpha"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let beta: String = reqwest::get(format!("{base}/identity/beta"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(alpha, "ALPHA");
    assert_eq!(beta, "BETA");
}

#[tokio::test]
async fn a_panicking_handler_becomes_a_500_and_the_host_survives() {
    let base = spawn_host().await;
    let resp = reqwest::get(format!("{base}/identity/panic"))
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        500,
        "a handler panic must be isolated into a 500 response"
    );

    // The host is still serving: the panic did not kill the connection loop.
    let health = reqwest::get(format!("{base}/health/live")).await.unwrap();
    assert_eq!(health.status().as_u16(), 200);
}

#[tokio::test]
async fn authorize_above_the_route_macro_is_not_dropped() {
    let base = spawn_host().await;
    let resp = reqwest::get(format!("{base}/identity/admin-above"))
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        401,
        "`#[authorize]` above `#[get]` must still protect the endpoint"
    );
}

#[tokio::test]
async fn authorize_below_the_route_macro_still_works() {
    let base = spawn_host().await;
    let resp = reqwest::get(format!("{base}/identity/admin-below"))
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}
