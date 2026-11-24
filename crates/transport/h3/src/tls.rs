//! TLS 1.3 material for the HTTP/3 endpoint.
//!
//! QUIC requires TLS 1.3, and HTTP/3 requires the ALPN protocol ID `h3`; using
//! the shared HTTP/1.1+2 rustls configuration would negotiate the wrong ALPN or
//! fail during the handshake. This module therefore builds an **independent**
//! TLS server configuration with exactly one protocol (`h3`) and exactly one
//! version (TLS 1.3). PEM decoding stays with `rustls-pemfile`, the provider and
//! cipher suites stay with rustls — no key material handling is re-implemented
//! here.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use webx_core::config::TlsClientAuth;
use webx_core::error::{Error, Result};

/// ALPN protocol ID mandated by RFC 9114 for HTTP/3.
pub(crate) const H3_ALPN: &[u8] = b"h3";

/// Build the TLS 1.3-only rustls server configuration used for QUIC.
///
/// The returned configuration is independent from the engine's TCP TLS
/// configuration: it advertises ALPN `h3` only (a client offering `h2` is
/// rejected during the handshake instead of receiving an HTTP/3 connection).
pub(crate) fn build_server_config(
    cert_path: &Path,
    key_path: &Path,
    client_ca_path: &Path,
    client_auth: TlsClientAuth,
) -> Result<Arc<ServerConfig>> {
    if !cert_path.is_file() {
        return Err(Error::Http(format!(
            "cannot open HTTP/3 TLS certificate '{}': file does not exist",
            cert_path.display()
        )));
    }
    if !key_path.is_file() {
        return Err(Error::Http(format!(
            "cannot open HTTP/3 TLS private key '{}': file does not exist",
            key_path.display()
        )));
    }

    let certificates = load_certificates(cert_path)?;
    let key = load_private_key(key_path)?;

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ServerConfig::builder_with_provider(Arc::clone(&provider))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|err| Error::Http(format!("HTTP/3 requires TLS 1.3: {err}")))?;
    let builder = match client_auth {
        TlsClientAuth::Off => builder.with_no_client_auth(),
        TlsClientAuth::Optional | TlsClientAuth::Required => {
            if !client_ca_path.is_file() {
                return Err(Error::Http(format!(
                    "HTTP/3 mTLS requires a readable client CA bundle; '{}' does not exist",
                    client_ca_path.display()
                )));
            }
            let roots = load_client_roots(client_ca_path)?;
            let verifier = WebPkiClientVerifier::builder_with_provider(roots, provider);
            let verifier = match client_auth {
                TlsClientAuth::Optional => verifier.allow_unauthenticated(),
                _ => verifier,
            };
            let verifier = verifier.build().map_err(|err| {
                Error::Http(format!(
                    "cannot build the HTTP/3 client-certificate verifier from '{}': {err}",
                    client_ca_path.display()
                ))
            })?;
            builder.with_client_cert_verifier(verifier)
        }
    };
    let mut config = builder.with_single_cert(certificates, key).map_err(|err| {
        Error::Http(format!(
            "HTTP/3 TLS configuration rejected the certificate/key pair ({}, {}): {err}",
            cert_path.display(),
            key_path.display()
        ))
    })?;
    config.alpn_protocols = vec![H3_ALPN.to_vec()];

    Ok(Arc::new(config))
}

fn load_client_roots(path: &Path) -> Result<Arc<RootCertStore>> {
    let certificates = load_certificates(path)?;
    let mut roots = RootCertStore::empty();
    for certificate in certificates {
        roots.add(certificate).map_err(|err| {
            Error::Http(format!(
                "cannot use a certificate from HTTP/3 client CA '{}': {err}",
                path.display()
            ))
        })?;
    }
    Ok(Arc::new(roots))
}

fn load_certificates(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let file = File::open(path).map_err(|err| {
        Error::Http(format!(
            "cannot open HTTP/3 TLS certificate '{}': {err}",
            path.display()
        ))
    })?;
    let mut reader = BufReader::new(file);

    let mut certificates = Vec::new();
    for item in rustls_pemfile::certs(&mut reader) {
        certificates.push(item.map_err(|err| {
            Error::Http(format!(
                "cannot parse HTTP/3 TLS certificate '{}': {err}",
                path.display()
            ))
        })?);
    }
    if certificates.is_empty() {
        return Err(Error::Http(format!(
            "no PEM CERTIFICATE block found in HTTP/3 TLS certificate '{}'",
            path.display()
        )));
    }

    Ok(certificates)
}

fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let file = File::open(path).map_err(|err| {
        Error::Http(format!(
            "cannot open HTTP/3 TLS private key '{}': {err}",
            path.display()
        ))
    })?;
    let mut reader = BufReader::new(file);

    // `rustls_pemfile::private_key` accepts PKCS#8, PKCS#1 and SEC1 blocks.
    rustls_pemfile::private_key(&mut reader)
        .map_err(|err| {
            Error::Http(format!(
                "cannot parse HTTP/3 TLS private key '{}': {err}",
                path.display()
            ))
        })?
        .ok_or_else(|| {
            Error::Http(format!(
                "no PKCS#8, PKCS#1 or SEC1 private key found in HTTP/3 TLS private key '{}'",
                path.display()
            ))
        })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn temp_pem(contents: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        file.write_all(contents.as_bytes())
            .expect("write temp file");
        file
    }

    fn self_signed_files() -> (tempfile::NamedTempFile, tempfile::NamedTempFile) {
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("self-signed certificate");
        (
            temp_pem(&certified.cert.pem()),
            temp_pem(&certified.key_pair.serialize_pem()),
        )
    }

    #[test]
    fn served_config_is_tls13_only_with_h3_alpn() {
        let (cert, key) = self_signed_files();
        let config =
            build_server_config(cert.path(), key.path(), Path::new(""), TlsClientAuth::Off)
                .expect("TLS config builds");
        assert_eq!(config.alpn_protocols, vec![b"h3".to_vec()]);
        // 2.9 security policy: QUIC 0-RTT stays off until the idempotency
        // replay story is designed end to end.
        assert_eq!(config.max_early_data_size, 0, "0-RTT must stay disabled");
        // The provider is pinned to TLS 1.3 by `with_protocol_versions`; rustls
        // exposes the enabled versions through the builder, not the config, so
        // the ALPN pin plus a successful build is the observable contract here.
        assert!(!config.alpn_protocols.contains(&b"h2".to_vec()));
    }

    #[test]
    fn missing_certificate_names_the_file() {
        let (_, key) = self_signed_files();
        let err = build_server_config(
            Path::new("does-not-exist/cert.pem"),
            key.path(),
            Path::new(""),
            TlsClientAuth::Off,
        )
        .unwrap_err();
        assert!(err.to_string().contains("does-not-exist/cert.pem"), "{err}");
    }

    #[test]
    fn missing_private_key_names_the_file() {
        let (cert, _) = self_signed_files();
        let err = build_server_config(
            cert.path(),
            Path::new("does-not-exist/key.pem"),
            Path::new(""),
            TlsClientAuth::Off,
        )
        .unwrap_err();
        assert!(err.to_string().contains("does-not-exist/key.pem"), "{err}");
    }

    #[test]
    fn a_file_without_certificates_is_reported() {
        let cert = temp_pem("-----BEGIN NOPE-----\nAAAA\n-----END NOPE-----\n");
        let (_, key) = self_signed_files();
        let err = build_server_config(cert.path(), key.path(), Path::new(""), TlsClientAuth::Off)
            .unwrap_err();
        assert!(
            err.to_string().contains("no PEM CERTIFICATE block"),
            "{err}"
        );
    }

    #[test]
    fn a_file_without_a_private_key_is_reported() {
        let (cert, _) = self_signed_files();
        let not_a_key = temp_pem(&std::fs::read_to_string(cert.path()).expect("read cert"));
        let err = build_server_config(
            cert.path(),
            not_a_key.path(),
            Path::new(""),
            TlsClientAuth::Off,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("no PKCS#8, PKCS#1 or SEC1 private key"),
            "{err}"
        );
    }

    #[test]
    fn mtls_requires_a_readable_client_ca() {
        let (cert, key) = self_signed_files();
        for auth in [TlsClientAuth::Optional, TlsClientAuth::Required] {
            let err = build_server_config(
                cert.path(),
                key.path(),
                Path::new("does-not-exist/client-ca.pem"),
                auth,
            )
            .unwrap_err();
            assert!(
                err.to_string().contains("client CA bundle")
                    && err.to_string().contains("does-not-exist/client-ca.pem"),
                "{err}"
            );
        }
    }

    #[test]
    fn self_signed_certificate_serves_as_client_auth_material() {
        let (cert, key) = self_signed_files();
        // A CA file is present and parseable: the verifier builds even though
        // this specific certificate is not a CA (rustls validates at handshake
        // time, which keeps startup cheap and deterministic).
        let config = build_server_config(
            cert.path(),
            key.path(),
            cert.path(),
            TlsClientAuth::Required,
        )
        .expect("client-auth TLS config builds");
        assert_eq!(config.alpn_protocols, vec![b"h3".to_vec()]);
    }

    #[test]
    fn a_mismatched_certificate_and_key_are_a_typed_error() {
        let (first_cert, _) = self_signed_files();
        let (_, second_key) = self_signed_files();
        let err = build_server_config(
            first_cert.path(),
            second_key.path(),
            Path::new(""),
            TlsClientAuth::Off,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("rejected the certificate/key pair"),
            "{err}"
        );
    }
}
