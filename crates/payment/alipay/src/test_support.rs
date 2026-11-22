//! Offline fixtures shared by the unit tests.
//!
//! RSA keys are generated once per test binary with a deterministic RNG (no
//! credential-looking literals checked into the repository, no network). The
//! generated X.509 certificates are *not* signed by a CA: they exist to
//! exercise parsing, public-key extraction and `app_cert_sn`/root-SN
//! computation, which never validates the certificate signature.

use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Duration;

use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use rsa::rand_core::{Infallible, TryCryptoRng, TryRng};
use rsa::{PrivateKey, PublicKey};
use x509_cert::der::asn1::BitString;
use x509_cert::der::{Decode, EncodePem};
use x509_cert::name::Name;
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::{AlgorithmIdentifierOwned, SubjectPublicKeyInfoOwned};
use x509_cert::time::Validity;
use x509_cert::{Certificate, TbsCertificate, Version};

use crate::options::{AlipayAccountOptions, AlipayKeyMode, AlipayOptions};
use crate::provider::{AlipayAccount, AlipayProvider};
use webx_pay::PaymentScope;

/// Application id every fixture account uses.
pub(crate) const APP_ID: &str = "2021000000000001";

/// Deterministic, non-cryptographic RNG used only to make test key generation
/// reproducible. It is never used outside `#[cfg(test)]`.
struct TestRng(u64);

impl TestRng {
    fn next_u64(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value >> 12;
        value ^= value << 25;
        value ^= value >> 27;
        self.0 = value;
        value.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
}

impl TryRng for TestRng {
    type Error = Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        Ok(self.next_u64() as u32)
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        Ok(self.next_u64())
    }

    fn try_fill_bytes(&mut self, destination: &mut [u8]) -> Result<(), Self::Error> {
        for chunk in destination.chunks_mut(8) {
            let bytes = self.next_u64().to_le_bytes();
            let length = chunk.len();
            chunk.copy_from_slice(&bytes[..length]);
        }
        Ok(())
    }
}

impl TryCryptoRng for TestRng {}

fn generate_pair(seed: u64) -> (PrivateKey, PublicKey) {
    let mut rng = TestRng(seed);
    // 1024-bit keeps debug-mode key generation fast; RSA2 (SHA-256) only needs
    // ~496 bits of modulus for PKCS#1 v1.5 padding.
    let private = PrivateKey::new(&mut rng, 1024).expect("test RSA key generation");
    let public = private.to_public_key();
    (private, public)
}

fn merchant_pair() -> &'static (PrivateKey, PublicKey) {
    static PAIR: OnceLock<(PrivateKey, PublicKey)> = OnceLock::new();
    PAIR.get_or_init(|| generate_pair(0x5eed_1234_5678_9abc))
}

fn alipay_pair() -> &'static (PrivateKey, PublicKey) {
    static PAIR: OnceLock<(PrivateKey, PublicKey)> = OnceLock::new();
    PAIR.get_or_init(|| generate_pair(0x0bad_c0de_dead_beef))
}

pub(crate) fn merchant_private_key() -> &'static PrivateKey {
    &merchant_pair().0
}

pub(crate) fn merchant_public_key() -> &'static PublicKey {
    &merchant_pair().1
}

pub(crate) fn alipay_private_key() -> &'static PrivateKey {
    &alipay_pair().0
}

pub(crate) fn alipay_public_key() -> &'static PublicKey {
    &alipay_pair().1
}

pub(crate) fn merchant_private_key_pem() -> &'static str {
    static PEM: OnceLock<String> = OnceLock::new();
    PEM.get_or_init(|| {
        merchant_private_key()
            .to_pkcs8_pem(LineEnding::LF)
            .expect("encode test private key")
            .to_string()
    })
}

pub(crate) fn alipay_public_key_pem() -> &'static str {
    static PEM: OnceLock<String> = OnceLock::new();
    PEM.get_or_init(|| {
        alipay_public_key()
            .to_public_key_pem(LineEnding::LF)
            .expect("encode test public key")
    })
}

/// Strip PEM armor and whitespace, returning the bare base64 body.
pub(crate) fn pem_body(pem: &str) -> String {
    pem.lines()
        .filter(|line| !line.starts_with("-----"))
        .map(str::trim)
        .collect()
}

/// The PEM certificates shared by the certificate-mode tests.
pub(crate) struct Certificates {
    pub(crate) app_cert_pem: String,
    pub(crate) alipay_public_cert_pem: String,
    pub(crate) root_chain_pem: String,
}

pub(crate) fn certificates() -> &'static Certificates {
    static CERTS: OnceLock<Certificates> = OnceLock::new();
    CERTS.get_or_init(build_certificates)
}

fn build_certificates() -> Certificates {
    const CA_NAME: &str = "CN=alipay-test-ca,O=alipay-test,C=CN";
    let app_cert = build_certificate(merchant_public_key(), CA_NAME, "CN=test-app", 1);
    let alipay_cert = build_certificate(alipay_public_key(), CA_NAME, "CN=alipay-public", 2);
    let root_one = build_certificate(
        merchant_public_key(),
        "CN=alipay-root-1,O=alipay-test,C=CN",
        "CN=alipay-root-1,O=alipay-test,C=CN",
        1,
    );
    let root_two = build_certificate(
        alipay_public_key(),
        "CN=alipay-root-2,O=alipay-test,C=CN",
        "CN=alipay-root-2,O=alipay-test,C=CN",
        2,
    );
    let root_chain_pem = format!(
        "{}\n{}",
        certificate_pem(&root_one),
        certificate_pem(&root_two)
    );
    Certificates {
        app_cert_pem: certificate_pem(&app_cert),
        alipay_public_cert_pem: certificate_pem(&alipay_cert),
        root_chain_pem,
    }
}

fn build_certificate(
    public_key: &PublicKey,
    issuer: &str,
    subject: &str,
    serial: u32,
) -> Certificate {
    let signature_algorithm = AlgorithmIdentifierOwned {
        oid: x509_cert::der::oid::db::rfc5912::SHA_256_WITH_RSA_ENCRYPTION,
        parameters: None,
    };
    let spki_der = public_key
        .to_public_key_der()
        .expect("encode test SubjectPublicKeyInfo");
    let subject_public_key_info = SubjectPublicKeyInfoOwned::from_der(spki_der.as_bytes())
        .expect("decode test SubjectPublicKeyInfo");
    Certificate {
        tbs_certificate: TbsCertificate {
            version: Version::V3,
            serial_number: SerialNumber::from(serial),
            signature: signature_algorithm.clone(),
            issuer: Name::from_str(issuer).expect("test issuer DN"),
            validity: Validity::from_now(Duration::from_secs(24 * 60 * 60)).expect("test validity"),
            subject: Name::from_str(subject).expect("test subject DN"),
            subject_public_key_info,
            issuer_unique_id: None,
            subject_unique_id: None,
            extensions: None,
        },
        signature_algorithm,
        signature: BitString::from_bytes(b"alipay-test-signature").expect("test signature"),
    }
}

fn certificate_pem(certificate: &Certificate) -> String {
    certificate
        .to_pem(LineEnding::LF)
        .expect("encode test certificate")
}

/// A valid public-key-mode account bound to the generated test keys.
pub(crate) fn account_options() -> AlipayAccountOptions {
    AlipayAccountOptions::new(
        APP_ID,
        merchant_private_key_pem(),
        AlipayKeyMode::public_key(alipay_public_key_pem()),
    )
}

/// A valid certificate-mode account bound to the generated test certificates.
pub(crate) fn certificate_account_options() -> AlipayAccountOptions {
    let certificates = certificates();
    AlipayAccountOptions::new(
        APP_ID,
        merchant_private_key_pem(),
        AlipayKeyMode::certificate(
            certificates.app_cert_pem.clone(),
            certificates.alipay_public_cert_pem.clone(),
            certificates.root_chain_pem.clone(),
        ),
    )
}

/// A single-account public-key-mode options value.
pub(crate) fn options() -> AlipayOptions {
    AlipayOptions::new(APP_ID, merchant_private_key_pem(), alipay_public_key_pem())
}

pub(crate) fn account() -> &'static AlipayAccount {
    static ACCOUNT: OnceLock<AlipayAccount> = OnceLock::new();
    ACCOUNT.get_or_init(|| {
        account_options()
            .into_account(false)
            .expect("test account builds")
    })
}

pub(crate) fn provider() -> &'static AlipayProvider {
    static PROVIDER: OnceLock<AlipayProvider> = OnceLock::new();
    PROVIDER.get_or_init(|| AlipayProvider::new(options()))
}

/// Default scope (no tenant/account): resolves the configured default account.
pub(crate) fn scope() -> PaymentScope {
    PaymentScope::new(crate::PROVIDER)
}

/// JSON object of a signed gateway response for `method`.
///
/// The signature covers `raw_node_json` exactly as embedded (which is why the
/// parser must extract the raw substring instead of re-serializing the JSON).
pub(crate) fn signed_gateway_body(method: &str, raw_node_json: &str) -> String {
    let node = format!("{}_response", method.replace('.', "_"));
    let signature =
        crate::sign::rsa2_sign(raw_node_json, alipay_private_key()).expect("sign test response");
    format!("{{\"{node}\":{raw_node_json},\"sign\":\"{signature}\"}}")
}

/// JSON object of a signed `error_response` gateway response.
pub(crate) fn signed_error_body(raw_error_json: &str) -> String {
    let signature =
        crate::sign::rsa2_sign(raw_error_json, alipay_private_key()).expect("sign test error");
    format!("{{\"error_response\":{raw_error_json},\"sign\":\"{signature}\"}}")
}

/// Form parameters with a valid RSA2 `sign` over the V1 notification content.
///
/// The passed parameters should include `sign_type=RSA2`; they must not already
/// contain `sign`.
pub(crate) fn signed_notification(
    params: impl IntoIterator<Item = (String, String)>,
) -> Vec<(String, String)> {
    let mut params: Vec<(String, String)> = params.into_iter().collect();
    let content = crate::sign::notify_sign_content(&params);
    let signature =
        crate::sign::rsa2_sign(&content, alipay_private_key()).expect("sign test notification");
    params.push(("sign".to_string(), signature));
    params
}

/// Notification parameters for a successful trade.
pub(crate) fn successful_notification() -> Vec<(String, String)> {
    [
        ("app_id", APP_ID),
        ("notify_time", "2023-11-15 06:13:20"),
        ("notify_type", "trade_status_sync"),
        ("notify_id", "notify-1"),
        ("charset", "utf-8"),
        ("version", "1.0"),
        ("sign_type", "RSA2"),
        ("trade_no", "2023111522001440000000000001"),
        ("out_trade_no", "ORDER-1"),
        ("trade_status", "TRADE_SUCCESS"),
        ("total_amount", "99.00"),
        ("receipt_amount", "99.00"),
        ("buyer_pay_amount", "99.00"),
        ("gmt_payment", "2023-11-15 06:13:19"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect()
}
