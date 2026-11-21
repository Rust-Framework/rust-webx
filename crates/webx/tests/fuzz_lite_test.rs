//! Fuzz-lite (2.4-8): deterministic malformed-input sweep over the wire.
//!
//! A seeded PRNG generates malformed requests (methods, paths, headers, body
//! lengths) and asserts the server never hangs or panics: every input either
//! gets a well-formed HTTP status or the connection is closed by the server,
//! and a valid request afterwards still succeeds. This is the CI-runnable
//! subset of the fuzz gate (a full `cargo-fuzz` corpus lives outside PR CI).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use webx::*;

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("addr").port()
}

#[derive(Default, WebxRequestMeta)]
struct EchoRequest;

#[derive(serde::Serialize)]
struct EchoView {
    ok: bool,
}

#[get("/echo")]
impl IRequest<EchoView> for EchoRequest {}

#[derive(Default)]
struct EchoHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<EchoRequest, EchoView> for EchoHandler {
    async fn handle(&mut self, _req: EchoRequest) -> ActionResult<EchoView> {
        Ok(EchoView { ok: true })
    }
}

fn wait_ready(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("fuzz server did not become ready");
}

/// Simple xorshift PRNG: deterministic across platforms, no dependencies.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, upper: usize) -> usize {
        (self.next() % upper as u64) as usize
    }

    fn byte(&mut self) -> u8 {
        (self.next() & 0xff) as u8
    }
}

/// Send one raw request and return what the server did: a status, or a closed
/// connection. A hang is a failure.
fn probe(port: u16, payload: &[u8]) -> std::result::Result<Option<u16>, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    // Ignore write errors (the server may already have closed the connection).
    let _ = stream.write_all(payload);
    let _ = stream.flush();
    let mut head = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => return Ok(None),
            Ok(_) => {
                head.push(byte[0]);
                if head.ends_with(b"\r\n\r\n") {
                    break;
                }
                if head.len() > 8 * 1024 {
                    return Err("response head too large".to_string());
                }
            }
            Err(_) => return Ok(None),
        }
    }
    let head = String::from_utf8_lossy(&head);
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok());
    match status {
        Some(status @ 100..=599) => Ok(Some(status)),
        other => Err(format!(
            "malformed status line {other:?}: {}",
            &head[..head.len().min(120)]
        )),
    }
}

#[test]
fn malformed_requests_never_panic_or_hang_the_server() {
    let port = free_port();
    let address = format!("127.0.0.1:{port}");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("fuzz runtime");
    runtime.spawn(async move {
        let host = Host::builder().mode(AppMode::Development).no_spa().build();
        let _ = host.run_at(&address).await;
    });
    wait_ready(port);

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let methods = [
        "GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "HEAD", "TRACE",
    ];
    let mut answered = 0usize;
    let mut closed = 0usize;

    for _ in 0..240 {
        let mut payload = Vec::new();
        // 1/4 of inputs are pure garbage without a request line.
        if rng.below(4) == 0 {
            for _ in 0..rng.below(64) {
                payload.push(rng.byte());
            }
        } else {
            let method = methods[rng.below(methods.len())];
            payload.extend_from_slice(method.as_bytes());
            payload.push(b' ');
            // Path: mostly valid-ish bytes, sometimes control bytes or huge.
            let path_len = 1 + rng.below(48);
            for _ in 0..path_len {
                let byte = match rng.below(8) {
                    0..=2 => b"/abc"[rng.below(4)],
                    3 => rng.byte(),
                    _ => b"0123456789abcdefghij"[rng.below(20)],
                };
                payload.push(byte);
            }
            payload.extend_from_slice(b" HTTP/1.1\r\nHost: fuzz\r\n");
            // Random headers, including malformed ones.
            for _ in 0..rng.below(4) {
                let name = ["x-fuzz", "content-length", "transfer-encoding", "x", ""][rng.below(5)];
                payload.extend_from_slice(name.as_bytes());
                payload.extend_from_slice(b": ");
                for _ in 0..rng.below(16) {
                    payload.push(rng.byte());
                }
                payload.extend_from_slice(b"\r\n");
            }
            payload.extend_from_slice(b"\r\n");
            // Random body bytes.
            let body_len = rng.below(128);
            for _ in 0..body_len {
                payload.push(rng.byte());
            }
        }

        match probe(port, &payload) {
            Ok(Some(status)) => {
                answered += 1;
                assert!(
                    (100..=599).contains(&status),
                    "status out of range: {status}"
                );
            }
            Ok(None) => closed += 1,
            Err(error) => panic!("server produced a malformed response: {error}"),
        }
    }

    assert!(
        answered + closed == 240,
        "every fuzz input must be answered or closed"
    );
    assert!(
        answered > 0,
        "the server should answer some malformed inputs"
    );

    // The server is still healthy after the whole sweep.
    let mut valid = TcpStream::connect(("127.0.0.1", port)).expect("valid connection");
    valid
        .write_all(b"GET /echo HTTP/1.1\r\nHost: fuzz\r\n\r\n")
        .expect("valid write");
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        valid.read_exact(&mut byte).expect("valid read");
        head.push(byte[0]);
    }
    assert!(
        String::from_utf8_lossy(&head).starts_with("HTTP/1.1 200"),
        "the server must keep serving valid requests after fuzzing"
    );
}
