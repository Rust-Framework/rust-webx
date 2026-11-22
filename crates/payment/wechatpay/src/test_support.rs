//! Offline test fixtures: keys and account options generated at test time.
//!
//! No private key, APIv3 key or PEM body is stored in the repository: the RSA
//! key pair is generated deterministically from a SHA-256 counter RNG, the
//! APIv3 key is derived from a label, and the platform certificate used by the
//! certificate tests is built in memory from the generated public key.

use std::convert::Infallible;
use std::sync::{Arc, OnceLock};

use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey};
use rsa::rand_core::{TryCryptoRng, TryRng};
use rsa::sha2::{Digest, Sha256};
use rsa::{PrivateKey, PublicKey};

/// Deterministic SHA-256 counter RNG (test-only; not used in production code).
struct TestRng {
    counter: u64,
}

impl TestRng {
    fn new(seed: u64) -> Self {
        Self { counter: seed }
    }
}

impl TryRng for TestRng {
    type Error = Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        self.try_next_u64().map(|value| (value >> 32) as u32)
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        let mut bytes = [0u8; 8];
        self.try_fill_bytes(&mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn try_fill_bytes(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
        const LABEL: &[u8] = b"rust-webx-pay-wechatpay test rng";
        let mut offset = 0;
        while offset < output.len() {
            let mut block_input = [0u8; 40];
            block_input[..LABEL.len()].copy_from_slice(LABEL);
            block_input[32..].copy_from_slice(&self.counter.to_le_bytes());
            let block = Sha256::digest(block_input);
            let take = (output.len() - offset).min(block.len());
            output[offset..offset + take].copy_from_slice(&block[..take]);
            offset += take;
            self.counter += 1;
        }
        Ok(())
    }
}

impl TryCryptoRng for TestRng {}

/// One generated RSA key pair plus its PEM encodings.
pub(crate) struct TestKeyPair {
    pub(crate) private: Arc<PrivateKey>,
    pub(crate) public: Arc<PublicKey>,
    pub(crate) private_pem: String,
    pub(crate) public_pem: String,
}

/// Process-wide generated key pair (1024 bits keeps the offline test suite fast;
/// production keys are 2048 bits and the adapter does not enforce a size).
pub(crate) fn key_pair() -> &'static TestKeyPair {
    static KEY_PAIR: OnceLock<TestKeyPair> = OnceLock::new();
    KEY_PAIR.get_or_init(|| {
        let private = PrivateKey::new(&mut TestRng::new(0x5EED_1234), 1024)
            .expect("deterministic RSA key generation");
        let public = private.to_public_key();
        let private_pem = private
            .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .expect("PKCS#8 PEM encoding")
            .to_string();
        let public_pem = public
            .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
            .expect("SPKI PEM encoding");
        TestKeyPair {
            private: Arc::new(private),
            public: Arc::new(public),
            private_pem,
            public_pem,
        }
    })
}

/// Deterministic 32-byte APIv3 key (derived from a label, never a literal).
pub(crate) fn api_v3_key() -> [u8; 32] {
    api_v3_key_text().as_bytes().try_into().expect("32 bytes")
}

/// The APIv3 key as text (config fields carry the key as an ASCII string).
pub(crate) fn api_v3_key_text() -> String {
    let digest = Sha256::digest(b"rust-webx-pay-wechatpay test APIv3 key");
    let mut text = String::with_capacity(32);
    for byte in &digest[..32] {
        // Keep it printable ASCII while remaining exactly 32 bytes.
        text.push(char::from(b'a' + (byte % 26)));
    }
    text
}

/// Build a PEM X.509 certificate that carries `public_key` as its SPKI.
///
/// Only the `subjectPublicKeyInfo` field matters for the adapter (WeChat Pay
/// certificates are used as signature-verification trust anchors); the rest of
/// the certificate is syntactically valid but unsigned test data.
pub(crate) fn certificate_pem(public_key: &PublicKey) -> String {
    use rsa::pkcs8::EncodePublicKey;
    use x509_cert::certificate::{Certificate, TbsCertificate, Version};
    use x509_cert::der::asn1::BitString;
    use x509_cert::der::{pem::LineEnding, Decode, EncodePem};
    use x509_cert::name::RdnSequence;
    use x509_cert::serial_number::SerialNumber;
    use x509_cert::spki::{AlgorithmIdentifierOwned, ObjectIdentifier, SubjectPublicKeyInfoOwned};
    use x509_cert::time::Validity;

    let spki_der = public_key
        .to_public_key_der()
        .expect("SPKI DER encoding of the test public key");
    let subject_public_key_info =
        SubjectPublicKeyInfoOwned::from_der(spki_der.as_bytes()).expect("SPKI decoding");
    let signature_algorithm = AlgorithmIdentifierOwned {
        oid: ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.11"),
        parameters: None,
    };
    let certificate = Certificate {
        tbs_certificate: TbsCertificate {
            version: Version::V3,
            serial_number: SerialNumber::from(0x5EEDu32),
            signature: signature_algorithm.clone(),
            issuer: RdnSequence::default(),
            validity: Validity::from_now(std::time::Duration::from_secs(3_600))
                .expect("validity window"),
            subject: RdnSequence::default(),
            subject_public_key_info,
            issuer_unique_id: None,
            subject_unique_id: None,
            extensions: None,
        },
        signature_algorithm,
        signature: BitString::from_bytes(&[0u8; 256]).expect("dummy signature bits"),
    };
    certificate
        .to_pem(LineEnding::LF)
        .expect("certificate PEM encoding")
}

/// Account options wired to the generated key pair.
pub(crate) fn account_options() -> crate::WechatPayAccountOptions {
    crate::WechatPayAccountOptions::new(
        "1900000001",
        "wxd678efh567hg6787",
        "5157F09EFDC096DE15EBE81A47057A7232F1B8E1",
        key_pair().private_pem.clone(),
        api_v3_key_text(),
    )
    .with_notify_url("https://example.com/webhooks/payments/wechatpay")
}

/// Options with a single generated account.
pub(crate) fn options() -> crate::WechatPayOptions {
    crate::WechatPayOptions::new(account_options())
}
