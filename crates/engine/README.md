# rust-webx-engine

The execution engine behind `rust-webx` 2.0.

- one acceptor + N current-thread worker runtimes (connections are handed over
  with safe `std::net::TcpStream` conversions; no `unsafe`);
- compile-time `HandlerDispatch` direct dispatch;
- zero-copy request `Parts`, lazy query parsing and `Bytes` responses;
- onion middleware pipeline with the 1.5 semantics;
- TLS (ALPN h2/http/1.1), HTTP/2, graceful drain and SSE tracking.

Applications normally depend on `rust-webx`, not on this crate. The crate is
published before `rust-webx-host` because the host adapter builds on it.

```text
cargo add rust-webx
```

See the workspace README and `docs/design/engine-2.0.md` for the architecture.
