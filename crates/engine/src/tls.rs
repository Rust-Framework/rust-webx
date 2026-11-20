//! TLS termination material for the engine.
//!
//! Certificate and key parsing is not re-invented here: `rustls` owns the
//! protocol and `rustls-pemfile` owns PEM decoding. This module only turns the
//! operator's two file paths into an `Arc<ServerConfig>` with the ALPN list the
//! engine's hyper `auto` builder expects, and reports every failure as a typed
//! [`Error`] instead of panicking during startup.

use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use webx_core::error::{Error, Result};

/// Protocols advertised during the TLS handshake, HTTP/2 first.
///
/// The order matters: a client that supports both picks `h2`, while an older
/// client still negotiates `http/1.1`. hyper's `auto::Builder` then serves the
/// version the client selected.
pub const ALPN_PROTOCOLS: [&[u8]; 2] = [b"h2", b"http/1.1"];

/// Build the server TLS configuration from PEM certificate and key files.
///
/// * The certificate file must contain at least one `CERTIFICATE` block; every
///   block is loaded so intermediate chains work.
/// * The key may be PKCS#8 (`PRIVATE KEY`), PKCS#1 (`RSA PRIVATE KEY`) or SEC1
///   (`EC PRIVATE KEY`).
/// * A file that cannot be opened, parsed, or paired (`with_single_cert`
///   mismatch) is reported as [`Error::Http`] with the offending path — the key
///   material itself is never included in the message or logged.
pub fn build_server_config(cert_path: &str, key_path: &str) -> Result<Arc<ServerConfig>> {
    if cert_path.trim().is_empty() || key_path.trim().is_empty() {
        return Err(Error::Http(
            "TLS certificate or key path not configured".to_string(),
        ));
    }

    // Report a missing file as the file the operator must fix, before parsing
    // the other one. Order matters for diagnostics only.
    if !std::path::Path::new(cert_path).is_file() {
        return Err(Error::Http(format!(
            "cannot open TLS certificate '{cert_path}': file does not exist"
        )));
    }
    if !std::path::Path::new(key_path).is_file() {
        return Err(Error::Http(format!(
            "cannot open TLS private key '{key_path}': file does not exist"
        )));
    }

    let certs = load_certs(cert_path)?;
    let key = load_key(key_path)?;

    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|err| {
            Error::Http(format!(
                "TLS configuration rejected the certificate/key pair ({cert_path}, {key_path}): {err}"
            ))
        })?;
    config.alpn_protocols = ALPN_PROTOCOLS
        .iter()
        .map(|protocol| protocol.to_vec())
        .collect();

    Ok(Arc::new(config))
}

fn load_certs(path: &str) -> Result<Vec<CertificateDer<'static>>> {
    let file = File::open(path)
        .map_err(|err| Error::Http(format!("cannot open TLS certificate '{path}': {err}")))?;
    let mut reader = BufReader::new(file);

    let mut certs = Vec::new();
    for item in rustls_pemfile::certs(&mut reader) {
        let cert = item
            .map_err(|err| Error::Http(format!("cannot parse TLS certificate '{path}': {err}")))?;
        certs.push(cert);
    }

    if certs.is_empty() {
        // `rustls_pemfile` silently skips a block whose base64 payload is
        // malformed, so distinguish "wrong file type" from "invalid payload"
        // for the operator.
        let has_certificate_block = std::fs::read_to_string(path)
            .map(|text| text.contains("-----BEGIN CERTIFICATE-----"))
            .unwrap_or(false);
        if has_certificate_block {
            return Err(Error::Http(format!(
                "cannot parse TLS certificate '{path}': invalid PEM payload"
            )));
        }
        return Err(Error::Http(format!(
            "no PEM CERTIFICATE block found in '{path}'"
        )));
    }
    Ok(certs)
}

fn load_key(path: &str) -> Result<PrivateKeyDer<'static>> {
    let file = File::open(path)
        .map_err(|err| Error::Http(format!("cannot open TLS private key '{path}': {err}")))?;
    let mut reader = BufReader::new(file);

    // `private_key` accepts PKCS#8, PKCS#1 and SEC1 PEM blocks.
    rustls_pemfile::private_key(&mut reader)
        .map_err(|err| Error::Http(format!("cannot parse TLS private key '{path}': {err}")))?
        .ok_or_else(|| {
            Error::Http(format!(
                "no PKCS#8, PKCS#1 or SEC1 private key found in '{path}'"
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(contents: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        file.write_all(contents.as_bytes())
            .expect("write temp file");
        file
    }

    #[test]
    fn empty_paths_are_rejected_with_a_typed_error() {
        let err = build_server_config("", "key.pem").unwrap_err();
        assert!(err.to_string().contains("not configured"), "{err}");
    }

    #[test]
    fn missing_certificate_path_names_the_file() {
        let err = build_server_config("does-not-exist/cert.pem", "key.pem").unwrap_err();
        assert!(err.to_string().contains("does-not-exist/cert.pem"), "{err}");
    }

    /// A certificate PEM whose base64 body decodes (the parser does not
    /// validate DER, so four base64 characters are enough to reach the key
    /// stage).
    const DECODABLE_CERT_PEM: &str =
        "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n";

    #[test]
    fn missing_key_path_names_the_file() {
        let cert = temp_file(DECODABLE_CERT_PEM);
        let err = build_server_config(
            cert.path().to_str().expect("utf-8 temp path"),
            "does-not-exist/key.pem",
        )
        .unwrap_err();
        assert!(err.to_string().contains("does-not-exist/key.pem"), "{err}");
    }

    #[test]
    fn a_file_without_certificates_is_reported() {
        let cert = temp_file("-----BEGIN NOPE-----\nAAAA\n-----END NOPE-----\n");
        let key = temp_file("not a key");
        let err = build_server_config(
            cert.path().to_str().expect("utf-8 temp path"),
            key.path().to_str().expect("utf-8 temp path"),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("no PEM CERTIFICATE block"),
            "{err}"
        );
    }

    #[test]
    fn a_certificate_with_invalid_base64_is_a_typed_error() {
        // Three base64 characters are not a valid payload length.
        let cert = temp_file("-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----\n");
        let key = temp_file(DECODABLE_CERT_PEM);
        let err = build_server_config(
            cert.path().to_str().expect("utf-8 temp path"),
            key.path().to_str().expect("utf-8 temp path"),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("cannot parse TLS certificate"),
            "{err}"
        );
    }

    #[test]
    fn a_file_without_a_private_key_is_reported() {
        let cert = temp_file(DECODABLE_CERT_PEM);
        // A certificate block is not a key: `private_key` skips it.
        let key = temp_file(DECODABLE_CERT_PEM);
        let err = build_server_config(
            cert.path().to_str().expect("utf-8 temp path"),
            key.path().to_str().expect("utf-8 temp path"),
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("no PKCS#8, PKCS#1 or SEC1 private key"),
            "{err}"
        );
    }

    #[test]
    fn alpn_advertises_h2_before_http1() {
        assert_eq!(ALPN_PROTOCOLS, [&b"h2"[..], &b"http/1.1"[..]]);
    }
}
