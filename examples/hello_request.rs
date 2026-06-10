//! hello_request — Minimal "Hello World" API using the LRWF framework.
//!
//! Demonstrates `#[get]` route shortcut + `#[handler]`  auto DI registration.
//! Run with: `cargo run --example hello_request`

use lrwf::*;

/// Request type — just a marker with no fields.
struct HelloRequest;

/// Declare the route and response type.
#[get("/hello")]
impl IRequest<String> for HelloRequest {}

/// Handler — auto-registered into DI via `#[handler]`.
#[derive(Default)]
struct HelloHandler;

#[handler]
#[async_trait]
impl IRequestHandler<HelloRequest, String> for HelloHandler {
    async fn handle(&self, _req: HelloRequest) -> Result<String> {
        Ok("Hello, World! Welcome to LRWF.".into())
    }
}

#[tokio::main]
async fn main() {
    Host::builder()
        .mode(AppMode::Development)
        .configure(|app| {
            app.useOptions(|o| {
                o.app.name = "Hello World API".into();
            });
        })
        .build()
        .run()
        .await
        .expect("Server failed to start");
}
