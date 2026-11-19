//! `HostBuilder::add_http_client` wiring: the registry and the port resolve
//! through DI, and a handler can look a named client up without hyper code.

#![allow(non_snake_case)]

use std::sync::Arc;

use webx::*;

#[derive(Debug, Default, WebxRequestMeta)]
struct ProbeRequest;

#[get("/probe/client")]
impl IRequest<String> for ProbeRequest {}

#[derive(Inject)]
struct ProbeHandler {
    #[inject]
    factory: Arc<dyn IHttpClientFactory>,
}

#[handler(inject)]
#[async_trait::async_trait]
impl IRequestHandler<ProbeRequest, String> for ProbeHandler {
    async fn handle(&mut self, _req: ProbeRequest) -> ActionResult<String> {
        let client = self
            .factory
            .create("mock")
            .map_err(|error| Error::Internal(error.to_string()))?;
        Ok(client.name().to_string())
    }
}

#[tokio::test]
async fn add_http_client_registers_the_registry_and_factory() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("addr").port()
    };
    let addr = format!("127.0.0.1:{port}");
    let client = HttpClient::new("mock", HttpClientOptions::new(format!("http://{addr}")))
        .expect("client builds");

    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .add_http_client("mock", client)
        .build();
    let server = host.into_server();
    let handle = server.handle();
    let run_addr = addr.clone();
    tokio::spawn(async move {
        let _ = server.run_at(&run_addr).await;
    });

    let base = format!("http://{addr}");
    for _ in 0..100 {
        if reqwest::get(format!("{base}/health/live")).await.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let response = reqwest::get(format!("{base}/probe/client"))
        .await
        .expect("probe request");
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.text().await.expect("body"), "\"mock\"");

    handle.shutdown();
}
