//! WebSocket upgrade origin policy: a cross-site handshake must be refused when
//! the host configured an origin allow-list, and browserless clients keep
//! working when it did not.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use webx::*;

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

async fn wait_ready(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("host did not become ready");
}

struct EchoHandler;

#[async_trait]
impl IWebSocketHandler for EchoHandler {
    async fn on_connect(&self, connection: Arc<dyn IConnection>) -> Result<()> {
        while let Some(message) = connection.recv().await {
            match message? {
                RealtimeMessage::Text(text) => connection.send_text(text).await?,
                RealtimeMessage::Binary(bytes) => connection.send_binary(bytes).await?,
                RealtimeMessage::Close { .. } => break,
            }
        }
        Ok(())
    }
}

async fn spawn_host(port: u16, origins: Vec<String>) -> ServerHandle {
    let address = format!("127.0.0.1:{port}");
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_websocket("/echo", Arc::new(EchoHandler))
        .use_websocket_origins(origins)
        .build();
    let handle = host.server_handle();
    tokio::spawn(async move {
        let _ = host.run_at(&address).await;
    });
    wait_ready(port).await;
    handle
}

/// Perform a raw HTTP upgrade handshake and return the response status code.
async fn handshake_status(port: u16, origin: Option<&str>) -> u16 {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let mut request = String::from(
        "GET /echo HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n",
    );
    if let Some(origin) = origin {
        request.push_str(&format!("Origin: {origin}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await.expect("write");

    let mut response = Vec::with_capacity(512);
    let mut buffer = [0u8; 256];
    loop {
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer))
            .await
            .expect("read timeout")
            .expect("read");
        if read == 0 {
            break;
        }
        response.extend_from_slice(&buffer[..read]);
        if response.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&response);
    head.split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or_else(|| panic!("malformed response: {head:?}"))
}

#[tokio::test]
async fn a_configured_origin_allow_list_rejects_other_origins() {
    let port = free_port();
    let handle = spawn_host(port, vec!["https://app.example".to_string()]).await;

    assert_eq!(
        handshake_status(port, Some("https://evil.example")).await,
        403,
        "a cross-site origin must be refused before the upgrade"
    );
    assert_eq!(
        handshake_status(port, None).await,
        403,
        "a handshake without Origin cannot prove it is same-site"
    );
    assert_eq!(
        handshake_status(port, Some("https://app.example")).await,
        101,
        "a listed origin upgrades"
    );

    handle.shutdown();
}

#[tokio::test]
async fn without_an_allow_list_non_browser_clients_still_connect() {
    let port = free_port();
    let handle = spawn_host(port, Vec::new()).await;

    assert_eq!(
        handshake_status(port, None).await,
        101,
        "no allow-list keeps the historical behavior for non-browser clients"
    );
    assert_eq!(
        handshake_status(port, Some("https://any.example")).await,
        101
    );

    handle.shutdown();
}
