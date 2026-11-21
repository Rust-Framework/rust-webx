//! 2.5 M4 — OIDC over real HTTPS with a private CA (local test IdP).
//!
//! This exercises `HttpOidcHttp` + rustls trust roots + discovery, JWKS and
//! token exchange end to end. It is still a test IdP, not an external provider,
//! but it covers the transport code the unit tests stub out.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use rcgen::{BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyPair, SanType};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use webx_oidc::{ClientAuth, HttpOidcHttp, OidcClient};

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

fn jwks(public_pem: &str) -> Value {
    let der = base64::engine::general_purpose::STANDARD
        .decode(
            public_pem
                .lines()
                .filter(|line| !line.starts_with("-----"))
                .collect::<String>(),
        )
        .expect("SPKI");
    let xy = &der[der.len() - 64..];
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    json!({
        "keys": [{
            "kty": "EC",
            "kid": "kid-e2e",
            "use": "sig",
            "crv": "P-256",
            "x": b64.encode(&xy[..32]),
            "y": b64.encode(&xy[32..])
        }]
    })
}

fn id_token(private_pem: &str, issuer: &str) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("kid-e2e".to_string());
    encode(
        &header,
        &json!({
            "sub": "user-e2e",
            "iss": issuer,
            "aud": "client-e2e",
            "exp": now_secs() + 3600,
            "iat": now_secs(),
            "nonce": "nonce-e2e",
            "email": "e2e@example.com",
            "name": "E2E User"
        }),
        &EncodingKey::from_ec_pem(private_pem.as_bytes()).expect("encoding key"),
    )
    .expect("id_token")
}

async fn read_request(
    stream: &mut tokio_rustls::server::TlsStream<TcpStream>,
) -> Option<(String, String)> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        stream.read_exact(&mut byte).await.ok()?;
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
        if head.len() > 64 * 1024 {
            return None;
        }
    }
    let head = String::from_utf8_lossy(&head);
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.split('?').next()?.to_string();
    let content_length = lines
        .find_map(|line| {
            let lower = line.to_ascii_lowercase();
            lower
                .strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        stream.read_exact(&mut body).await.ok()?;
    }
    Some((method, path))
}

async fn write_json(
    stream: &mut tokio_rustls::server::TlsStream<TcpStream>,
    status: u16,
    body: &Value,
) {
    let body = serde_json::to_vec(body).expect("json");
    let head = format!(
        "HTTP/1.1 {status} OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(&body).await;
    let _ = stream.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oidc_discovery_and_code_exchange_over_https() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    // CA + server certificate (localhost), both generated for this test.
    let mut ca_params = CertificateParams::new(Vec::new()).expect("CA params");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_key = KeyPair::generate().expect("CA key");
    let ca_cert = ca_params.self_signed(&ca_key).expect("CA cert");

    let mut server_params = CertificateParams::new(Vec::new()).expect("server params");
    server_params.subject_alt_names =
        vec![SanType::IpAddress("127.0.0.1".parse().expect("loopback"))];
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server_key = KeyPair::generate().expect("server key");
    let server_cert = server_params
        .signed_by(&server_key, &ca_cert, &ca_key)
        .expect("server cert");

    // IdP signing key (ES256) + JWKS.
    let idp_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("IdP key");
    let idp_private = idp_key.serialize_pem();
    let idp_public = idp_key.public_key_pem();

    let dir = tempfile::tempdir().expect("temp dir");
    let ca_path = dir.path().join("ca.pem");
    std::fs::write(&ca_path, ca_cert.pem()).expect("ca file");

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let issuer = format!("https://127.0.0.1:{port}");

    let mut server_tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(server_cert.der().to_vec())],
            PrivateKeyDer::try_from(server_key.serialize_der()).expect("server key der"),
        )
        .expect("server tls");
    server_tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_tls));

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let server_issuer = issuer.clone();
    let server_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => break,
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { continue };
                    let acceptor = acceptor.clone();
                    let issuer = server_issuer.clone();
                    let idp_private = idp_private.clone();
                    let jwks = jwks(&idp_public);
                    tokio::spawn(async move {
                        let Ok(mut tls) = acceptor.accept(stream).await else { return };
                        let Some((method, path)) = read_request(&mut tls).await else { return };
                        match (method.as_str(), path.as_str()) {
                            ("GET", "/.well-known/openid-configuration") => {
                                write_json(&mut tls, 200, &json!({
                                    "issuer": issuer,
                                    "authorization_endpoint": format!("{issuer}/authorize"),
                                    "token_endpoint": format!("{issuer}/token"),
                                    "jwks_uri": format!("{issuer}/jwks"),
                                    "code_challenge_methods_supported": ["S256"],
                                    "id_token_signing_alg_values_supported": ["ES256"],
                                    "response_types_supported": ["code"]
                                })).await;
                            }
                            ("GET", "/jwks") => write_json(&mut tls, 200, &jwks).await,
                            ("POST", "/token") => {
                                write_json(&mut tls, 200, &json!({
                                    "access_token": "access-e2e",
                                    "id_token": id_token(&idp_private, &issuer),
                                    "refresh_token": "refresh-e2e",
                                    "expires_in": 3600,
                                    "token_type": "Bearer"
                                })).await;
                            }
                            _ => write_json(&mut tls, 404, &json!({"error": "not_found"})).await,
                        }
                    });
                }
            }
        }
    });

    let http = Arc::new(
        HttpOidcHttp::with_root_certificates(&issuer, Duration::from_secs(5), &ca_path)
            .expect("oidc http"),
    );
    let client = OidcClient::discover(http, &issuer, "client-e2e", ClientAuth::None)
        .await
        .expect("discovery over HTTPS");
    let session = client
        .exchange_code(
            "code-e2e",
            "https://app.example.com/cb",
            Some("verifier"),
            Some("nonce-e2e"),
        )
        .await
        .expect("code exchange over HTTPS");
    assert_eq!(session.subject, "user-e2e");
    assert_eq!(session.email.as_deref(), Some("e2e@example.com"));
    assert_eq!(session.name.as_deref(), Some("E2E User"));

    let _ = shutdown_tx.send(true);
    let _ = server_task.await;
}
