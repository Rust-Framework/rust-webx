//! Minimal HTTP/3 server for external interoperability tools (h3i, h3spec,
//! quiche-client).
//!
//! ```text
//! cargo run -p rust-webx-h3 --example interop_server
//! ```
//!
//! Serves the real rust-webx engine pipeline over QUIC on UDP `H3_BIND`
//! (default `0.0.0.0:4433`):
//!
//! * `GET /health` -> `200 application/health+json` `{"status":"pass",...}`
//!   (the framework's native health endpoint, with one probe registered);
//! * `POST /echo`  -> JSON body echo (`{"data":"hello"}` -> `"hello"`).
//!
//! TLS material is read from the PEM paths in `H3_CERT`/`H3_KEY`; when neither
//! variable is set the example generates a throwaway self-signed `localhost`
//! certificate with the crate's dev-dependency `rcgen` and prints its path.
//! Interop clients must skip certificate validation for that certificate:
//!
//! ```text
//! h3spec -n 127.0.0.1 4433
//! quiche-client --no-verify https://127.0.0.1:4433/health
//! h3i --no-verify https://127.0.0.1:4433/health
//! ```
//!
//! The host also binds its normal HTTP/1.1+2 listener (on an ephemeral loopback
//! port here) because `Host::run` serves the configured `App.Urls`; only the
//! UDP/QUIC endpoint is meant for the interop tools.

use std::path::PathBuf;
use std::sync::Arc;

use webx::*;
use webx_h3::{H3Config, H3Transport};

#[derive(Debug, Default, WebxRequestMeta)]
struct EchoRequest {
    #[frombody]
    data: String,
}

#[post("/echo")]
impl IRequest<String> for EchoRequest {}

#[derive(Default)]
struct EchoHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<EchoRequest, String> for EchoHandler {
    async fn handle(&mut self, request: EchoRequest) -> ActionResult<String> {
        Ok(request.data)
    }
}

fn tls_error(message: impl std::fmt::Display) -> Error {
    Error::Http(message.to_string())
}

/// PEM certificate/key pair used by the QUIC listener.
struct TlsMaterial {
    cert: PathBuf,
    key: PathBuf,
    /// Keeps the generated certificate directory alive for the whole run.
    _generated: Option<tempfile::TempDir>,
}

impl TlsMaterial {
    fn resolve() -> Result<Self> {
        match (std::env::var_os("H3_CERT"), std::env::var_os("H3_KEY")) {
            (Some(cert), Some(key)) => Ok(Self {
                cert: cert.into(),
                key: key.into(),
                _generated: None,
            }),
            (None, None) => Self::self_signed(),
            _ => Err(tls_error("set both H3_CERT and H3_KEY, or neither")),
        }
    }

    fn self_signed() -> Result<Self> {
        let params = rcgen::CertificateParams::new(vec!["localhost".to_string()])
            .map_err(|error| tls_error(format!("certificate params: {error}")))?;
        let key = rcgen::KeyPair::generate()
            .map_err(|error| tls_error(format!("generate a private key: {error}")))?;
        let cert = params
            .self_signed(&key)
            .map_err(|error| tls_error(format!("self-sign the certificate: {error}")))?;

        let directory = tempfile::tempdir()
            .map_err(|error| tls_error(format!("create the certificate directory: {error}")))?;
        let cert_path = directory.path().join("cert.pem");
        let key_path = directory.path().join("key.pem");
        std::fs::write(&cert_path, cert.pem())
            .map_err(|error| tls_error(format!("write {}: {error}", cert_path.display())))?;
        std::fs::write(&key_path, key.serialize_pem())
            .map_err(|error| tls_error(format!("write {}: {error}", key_path.display())))?;

        Ok(Self {
            cert: cert_path,
            key: key_path,
            _generated: Some(directory),
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let bind = std::env::var("H3_BIND").unwrap_or_else(|_| "0.0.0.0:4433".to_string());
    let tls = TlsMaterial::resolve()?;
    let transport = H3Transport::new(H3Config::new(
        bind.clone(),
        tls.cert.clone(),
        tls.key.clone(),
    ))?;

    println!(
        "rust-webx HTTP/3 interop server listening on udp://{}",
        transport.local_addr()
    );
    println!("  certificate: {}", tls.cert.display());
    println!("  GET  /health -> 200 application/health+json");
    println!("  POST /echo   -> JSON body echo ({{\"data\":\"hello\"}})");
    println!("Press Ctrl+C to stop.");

    Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .add_health_check("h3_transport", HealthStatus::pass)
        .configure(|app| {
            app.use_options(|options| {
                // This example is only reached over QUIC; an ephemeral loopback
                // listener satisfies the host's mandatory URL configuration.
                options.app.urls = vec!["http://127.0.0.1:0".to_string()];
            });
        })
        .use_http_transport(Arc::new(transport))
        .try_build()?
        .run()
        .await
}
