//! F1 閳?asymmetric JWT signing, JWKS publication and `kid` rotation.
//!
//! Covers the signing-side closed loop (sign with the private key, verify with
//! the published public keys), the JWKS document shape (public parameters
//! only), retired-key rotation, and the fail-closed startup rules of
//! `Jwt.Signing`. Everything here is offline: no `Jwt.Jwks` fetch is ever
//! performed.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{
    decode, decode_header, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation,
};
use serde_json::{json, Value};
use webx_core::config::{
    JwtAlgorithm, JwtJwksSourceSection, JwtRetiredSigningKeySection, JwtSection, JwtSigningSection,
};
use webx_core::mode::AppMode;
use webx_core::security::{ITokenIssuer, TokenClaims};
use webx_host::auth_jwks::{
    IJwtKeyResolver, Jwk, JwkSet, LocalFirstKeyResolver, StaticKeyResolver,
};
use webx_host::auth_jwt::{JwtKeySet, JwtSigningKey, JwtTokenIssuer, JwtValidation};
use webx_host::server::Host;

const HS256_SECRET: &str = "f1-auth-signing-test-hs256-secret-32-bytes";

// ---------------------------------------------------------------------------
// Test-only key material (never used by production code)
// ---------------------------------------------------------------------------

/// TEST-ONLY RSA-2048 PKCS#8 private key. Generated for this test file; it is
/// public by construction and must never be used outside tests.
const TEST_ONLY_RSA_PRIVATE_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCwLJU4OyqQJfUo
fMueIyy7Xn5m5j2ygNfnakZ8AkA5IyALbRsy8b1JsHBMKzVn9d3vGZfbtoRHGxZy
17KdzA2q9bZcscYdJ6UzPUgK2nMy9b9lsqBNvnfdqjMwdesLHmfCJsCdSloeGx10
l2ioaQSbh+cVa2qbdozqoeJ3v/SE3RspIUqUqqO3hxpUO/vR5U9G+23q6Eked7Ed
lR4C1R6mB46OauyuRT6RzF+VLppKSI5R+nD3uUu2vNddUgdXXbfb/M3ba6QOmMCz
eLh3NDB7uiRdDgKdUPOYoyvI/2mm9ITpSLuWqAsgrkBYNrA1yz4e3r0obGgugDBT
iDSOBXA/AgMBAAECggEAAiV0VIMwlyFGHKBOTr/mq3f+h6oc/ytNPzc4GXgYaH5d
xW8CI62/s50f9a/iyfdObiklhJ8Fn4i3qbmob0dtRzN+SNcaLu4s3FjOESvIbhQU
Frhg0ZY5vjDifw2lSdnRaFyNW+ZZ2dswI/cQQhBrL88JDYouMQb2SWTtgGTWt1qB
ClwXLYgh4oUXoX44m6eg6UZr1D2koSnSmNhLY5n+KisNVmBtkAJvi0v9oaOHyAxo
fQ1CLq3HWa/wjjEAqFPXmvQCeGvuBAvgoPaDMVjz0Qb3ILCgKEsSavG/w8N9z458
9Xu88CGYP/AS2v0lDALseI+JEBqI/s/KHv8d5nI0qQKBgQDszQsodu+Xkr8+gGbZ
UKr56KE1XN1YOSmFR3xl30qEhJvCzbdgbbnWV71ve8DH3AyDrcNZvw+cY71Fl+2O
RoLL0yQHMeuELWzSxtUZbiTuJ4H4sIsfB2rZLNVRNMuv6/WUV53JDMOMF1JRUy5b
EhssKwSXcadPF1bkUBvoAU8JOwKBgQC+dTDk0FBIE1Yrhbzq8C54zl/pvhPSoqcQ
AJeFpmtNFPwCQn6GqUDvY6kL4kZLrA4oIRzoPwrH0Ud+8wSPuYeItHYdgZF7REfK
eizBS8uw6DD5uGhq02LgA/VLAFr6kum0WHa7nY4UMA03dkK4tq7cqg83vThDk8Wl
/udcxGBkzQKBgQCagmfHbWOD2SKkG5AAOBx3mAyoDwH2MMYgyrYmmn263fPsNeJz
LuVVMEbQ2US+o5wex2BxzLLY7lNvd2Rvsx5g9FHLyAzOX/1jwo0FXNMH+gtaQ4dj
JanYLuK5a+OD1EiXFFxF9OgbHpR9r/LNRZEpTqBeWLA8cRQnQQYUadYyRQKBgD5G
aW9X4fg/QlzGxQf48ukTae8u9KJaBtBHsUPFL66rnt9KSL7VVuMvrhWxrhSONK7Q
bPhPrMG7X2pWIfawzm1XvItXU29/2w2Mt1eY5gh5U5JDxvivF9Rx1uHoQtdN9FbA
2knscYmOULQC2jw1l6WVgZZTK7OE/QqS96mwmGg9AoGAJSSrmdQzvw8hHniO30M/
eMeh34jEvZ3bHjxB5faPJomtsDDWu1wqwxR/GdNsfgwBpggTRzSmphUjBjFmNxpc
FEHM/ktoViGWozre3eZF3ECnfTJc0x/AWD4LMEdIm/3yTdUbcknlMXFLMpPyby6t
CG+s4lI/cqPfd/393ZXa1V4=
-----END PRIVATE KEY-----";

/// TEST-ONLY SPKI public key matching [`TEST_ONLY_RSA_PRIVATE_PEM`].
const TEST_ONLY_RSA_PUBLIC_PEM: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAsCyVODsqkCX1KHzLniMs
u15+ZuY9soDX52pGfAJAOSMgC20bMvG9SbBwTCs1Z/Xd7xmX27aERxsWcteyncwN
qvW2XLHGHSelMz1ICtpzMvW/ZbKgTb533aozMHXrCx5nwibAnUpaHhsddJdoqGkE
m4fnFWtqm3aM6qHid7/0hN0bKSFKlKqjt4caVDv70eVPRvtt6uhJHnexHZUeAtUe
pgeOjmrsrkU+kcxflS6aSkiOUfpw97lLtrzXXVIHV1232/zN22ukDpjAs3i4dzQw
e7okXQ4CnVDzmKMryP9ppvSE6Ui7lqgLIK5AWDawNcs+Ht69KGxoLoAwU4g0jgVw
PwIDAQAB
-----END PUBLIC KEY-----";

/// TEST-ONLY 1024-bit RSA PKCS#8 private key, used to prove the RSA strength
/// guard rejects short moduli. Never used outside tests.
const TEST_ONLY_SHORT_RSA_PRIVATE_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIICdwIBADANBgkqhkiG9w0BAQEFAASCAmEwggJdAgEAAoGBAMrYxQVMqTBjLm3S
U/8b6Q1ge3okyLLoMA4I2GDTg5kAgsCgxIVtyWJgxG/Lhh14+gwbAfYJAPbfSyid
eEdn94VpsvVx4Nraf06slqSY9OhnF6FYjTkXk5HjaemDzt+v27r3F2woXYhvmshU
nWdYf8ZkldahfsLvk1e9Rdc4kBJhAgMBAAECgYAFSI4hLDT/8KVl8OOFMonocknl
DY44srmFtA9GvlugIZZGRCm6O/jR2iljrA5rOBpzNEThzqw6wegoP6nBWmDlCdOm
Ma2i+zwy8zxF+0nOw6jzz53GAJCWB7qoqE8OWf3R0oALPwSTKSSaG+PxbsUWgH22
0+4Z2LKHt+njZUejAQJBAOYD+2GZudRfP8evx7qUUoLDBGfvtJHO0ozxR15mRzui
JrqSZmjOL4LQRliloRL50Rf6aQtaWYe/Z0sS3Vj9wPECQQDhwxHcHQnMNaBM8q2L
LNZ1i26Df7sPp0OQjnib7auko8lHMAbwU8g+3FEFG7gRQWF9w99RUPyOnSM4OC28
7GhxAkEAszSfAie29JNRKc72GZO9xUsTf7buRDuTJ2REanPdQ8TNz8aIK1eZBJ4o
cKB6okIoejoK5WF7J5wkagVElzT8wQJAOlLt80KhPFuCTT7IrNLpmLZr15cRGYsp
vlfAOvBczoztu/FhaYcLjYnXeI1fsT2F1TrcH6LfjGx2I9kHn+4nUQJBANXOjaXl
+ZLPijklFrTpcGRbVq4IaBsldD4sKklu8C5iwYYFs6La+avMYUtcNlmR85VAvevc
22ORE1K/z6CaEpA=
-----END PRIVATE KEY-----";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after unix epoch")
        .as_secs()
}

/// A fresh P-256 key pair (PEM private, PEM public).
fn ec_key_pair() -> (String, String) {
    let pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("EC key");
    (pair.serialize_pem(), pair.public_key_pem())
}

fn es256_token(private_pem: &str, kid: &str, subject: &str) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(kid.to_string());
    encode(
        &header,
        &json!({
            "sub": subject,
            "iat": now_secs(),
            "exp": now_secs() + 3600,
        }),
        &EncodingKey::from_ec_pem(private_pem.as_bytes()).expect("encoding key"),
    )
    .expect("sign test token")
}

/// A private temp directory holding key files for one test.
struct KeyDir {
    dir: tempfile::TempDir,
}

impl KeyDir {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("temp dir"),
        }
    }

    fn write(&self, name: &str, contents: &str) -> String {
        let path = self.dir.path().join(name);
        std::fs::write(&path, contents).expect("write key file");
        path.to_string_lossy().into_owned()
    }

    /// Write a private key and, on Unix, make it `0600` so the startup guard
    /// accepts it (the guard is a no-op on Windows).
    fn write_private(&self, name: &str, contents: &str) -> String {
        let path: PathBuf = self.dir.path().join(name);
        std::fs::write(&path, contents).expect("write private key");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .expect("chmod 600");
        }
        path.to_string_lossy().into_owned()
    }
}

fn signing_section(
    algorithm: JwtAlgorithm,
    key_id: &str,
    private_key_file: &str,
    public_key_file: Option<&str>,
) -> JwtSigningSection {
    let mut section = JwtSigningSection::default();
    section.algorithm = algorithm;
    section.key_id = key_id.to_string();
    section.private_key_file = private_key_file.to_string();
    section.public_key_file = public_key_file.map(str::to_string);
    section
}

fn retired_section(
    algorithm: JwtAlgorithm,
    key_id: &str,
    public_key_file: &str,
) -> JwtRetiredSigningKeySection {
    let mut section = JwtRetiredSigningKeySection::default();
    section.algorithm = algorithm;
    section.key_id = key_id.to_string();
    section.public_key_file = public_key_file.to_string();
    section
}

fn es256_validation() -> Validation {
    Validation::new(Algorithm::ES256)
}

// ---------------------------------------------------------------------------
// End-to-end: sign with the private key, verify through the resolver
// ---------------------------------------------------------------------------

#[tokio::test]
async fn es256_signing_key_resolver_roundtrip() {
    let (private_pem, _) = ec_key_pair();
    let files = KeyDir::new();
    let private_path = files.write_private("ec-2026-11-a.pem", &private_pem);

    let keys = Arc::new(
        JwtKeySet::from_section(
            &signing_section(JwtAlgorithm::Es256, "2026-11-a", &private_path, None),
            &[],
        )
        .expect("key set"),
    );
    assert_eq!(keys.active().key_id(), "2026-11-a");
    assert_eq!(keys.active().algorithm(), JwtAlgorithm::Es256);

    let issuer = JwtTokenIssuer::from_key_set(Arc::clone(&keys), Duration::from_secs(3600));
    let issued = issuer
        .issue(TokenClaims::new("user-1"))
        .await
        .expect("issue token");

    let header = decode_header(&issued.token).expect("token header");
    assert_eq!(
        header.alg,
        Algorithm::ES256,
        "asymmetric header carries alg"
    );
    assert_eq!(
        header.kid.as_deref(),
        Some("2026-11-a"),
        "asymmetric header carries kid"
    );

    let resolver = keys.verification_resolver();
    let decoding_key = resolver
        .resolve("2026-11-a", Algorithm::ES256)
        .await
        .expect("resolve by kid");
    let decoded = decode::<Value>(&issued.token, decoding_key.as_ref(), &es256_validation())
        .expect("verify through the resolver");
    assert_eq!(decoded.claims["sub"], "user-1");
    assert_eq!(
        decoded.claims["jti"].as_str().map(str::is_empty),
        Some(false),
        "the issuer still mints jti"
    );
}

#[tokio::test]
async fn ps256_key_roundtrips_through_the_resolver() {
    // PS256 shares the RSA key material with RS256; this pins the algorithm
    // mapping so a PS256 signing key is not silently unusable.
    let files = KeyDir::new();
    let private_path = files.write_private("rsa-2026-11-a.pem", TEST_ONLY_RSA_PRIVATE_PEM);
    let keys = Arc::new(
        JwtKeySet::from_section(
            &signing_section(JwtAlgorithm::Ps256, "2026-11-a", &private_path, None),
            &[],
        )
        .expect("PS256 key set"),
    );
    let issuer = JwtTokenIssuer::from_key_set(Arc::clone(&keys), Duration::from_secs(3600));
    let issued = issuer
        .issue(TokenClaims::new("user-1"))
        .await
        .expect("issue PS256 token");

    let header = decode_header(&issued.token).expect("token header");
    assert_eq!(header.alg, Algorithm::PS256);

    let resolver = keys.verification_resolver();
    let key = resolver
        .resolve("2026-11-a", Algorithm::PS256)
        .await
        .expect("PS256 key resolves");
    let mut validation = Validation::new(Algorithm::PS256);
    validation.algorithms = vec![Algorithm::PS256];
    let decoded =
        decode::<Value>(&issued.token, key.as_ref(), &validation).expect("PS256 verifies");
    assert_eq!(decoded.claims["sub"], "user-1");
}

/// Regression: `StaticKeyResolver::with_pem` used to reject `PS256` because the
/// algorithm was not mapped to the RSA PEM parser.
#[test]
fn static_resolver_with_pem_accepts_ps256() {
    StaticKeyResolver::new()
        .with_pem(
            "2026-11-a",
            Algorithm::PS256,
            TEST_ONLY_RSA_PUBLIC_PEM.as_bytes(),
        )
        .expect("PS256 public key accepted");
    StaticKeyResolver::new()
        .with_pem(
            "2026-11-a",
            Algorithm::RS256,
            TEST_ONLY_RSA_PUBLIC_PEM.as_bytes(),
        )
        .expect("RS256 public key accepted");
    assert!(
        StaticKeyResolver::new()
            .with_pem(
                "2026-11-a",
                Algorithm::HS256,
                TEST_ONLY_RSA_PUBLIC_PEM.as_bytes()
            )
            .is_err(),
        "HS256 has no PEM public key"
    );
}

// ---------------------------------------------------------------------------
// JWKS publication
// ---------------------------------------------------------------------------

#[test]
fn signing_key_api_and_debug_redaction() {
    let (private, _) = ec_key_pair();
    let key = JwtSigningKey::from_pem(JwtAlgorithm::Es256, "2026-11-a", private.as_bytes())
        .expect("EC signing key");
    assert_eq!(key.key_id(), "2026-11-a");
    assert_eq!(key.algorithm(), JwtAlgorithm::Es256);
    let public_jwk = key.public_jwk();
    assert_eq!(public_jwk.kty, "EC");
    assert_eq!(public_jwk.use_.as_deref(), Some("sig"));

    // `Debug` must never print PEM bodies or private parameters.
    let debug = format!("{key:?}");
    assert!(debug.contains("<redacted>"), "got {debug}");
    assert!(!debug.contains("PRIVATE KEY"), "got {debug}");
    let pem_body = private
        .lines()
        .find(|line| !line.starts_with("-----"))
        .unwrap_or_default();
    assert!(
        !debug.contains(&pem_body[..pem_body.len().min(24)]),
        "the PEM body leaked into Debug: {debug}"
    );

    // The key set's Debug is redacted the same way.
    let files = KeyDir::new();
    let path = files.write_private("ec.pem", &private);
    let keys = JwtKeySet::from_section(
        &signing_section(JwtAlgorithm::Es256, "2026-11-a", &path, None),
        &[],
    )
    .expect("key set");
    let debug = format!("{keys:?}");
    assert!(debug.contains("<redacted>"), "got {debug}");
    assert!(!debug.contains("PRIVATE KEY"), "got {debug}");
}

#[test]
fn jwks_document_has_only_public_material() {
    let (ec_private, _) = ec_key_pair();
    let files = KeyDir::new();
    let ec_path = files.write_private("ec.pem", &ec_private);
    let rsa_path = files.write_private("rsa.pem", TEST_ONLY_RSA_PRIVATE_PEM);

    let ec_jwks = JwtKeySet::from_section(
        &signing_section(JwtAlgorithm::Es256, "ec-1", &ec_path, None),
        &[],
    )
    .expect("EC key set")
    .jwks();
    let document: Value =
        serde_json::from_str(&ec_jwks.to_json().expect("serialize EC JWKS")).expect("JSON");
    let key = &document["keys"][0];
    assert_eq!(key["kty"], "EC");
    assert_eq!(key["kid"], "ec-1");
    assert_eq!(key["alg"], "ES256");
    assert_eq!(key["use"], "sig");
    assert_eq!(key["crv"], "P-256");
    for coordinate in ["x", "y"] {
        let value = key[coordinate].as_str().expect("EC coordinate");
        assert_eq!(value.len(), 43, "P-256 {coordinate} is 32 base64url bytes");
    }
    for private in ["d", "p", "q", "dp", "dq", "qi", "k"] {
        assert!(
            key.get(private).is_none(),
            "an EC JWK must not publish {private}"
        );
    }

    let rsa_jwks = JwtKeySet::from_section(
        &signing_section(JwtAlgorithm::Rs256, "rsa-1", &rsa_path, None),
        &[],
    )
    .expect("RSA key set")
    .jwks();
    let rsa_json = rsa_jwks.to_json().expect("serialize RSA JWKS");
    let document: Value = serde_json::from_str(&rsa_json).expect("JSON");
    let key = &document["keys"][0];
    assert_eq!(key["kty"], "RSA");
    assert_eq!(key["alg"], "RS256");
    assert_eq!(key["use"], "sig");
    assert!(key["n"].as_str().is_some_and(|value| !value.is_empty()));
    assert_eq!(key["e"], "AQAB", "the RSA exponent is the minimal encoding");
    for private in ["d", "p", "q", "dp", "dq", "qi"] {
        assert!(
            key.get(private).is_none(),
            "an RSA JWK must not publish {private}"
        );
    }

    // The document parses back into the verifier's own type with `use: sig`.
    let parsed: JwkSet = serde_json::from_str(&rsa_json).expect("JWKS round-trip");
    assert_eq!(parsed.keys.len(), 1);
    assert_eq!(parsed.keys[0].use_.as_deref(), Some("sig"));
    assert_eq!(parsed.keys[0].kid.as_deref(), Some("rsa-1"));
}

#[test]
fn jwk_from_public_key_pem_rejects_hs256() {
    let error = Jwk::from_public_key_pem(
        JwtAlgorithm::Hs256,
        "kid",
        TEST_ONLY_RSA_PUBLIC_PEM.as_bytes(),
    )
    .expect_err("HS256 has no public JWK");
    assert!(error.to_string().contains("Jwt.Signing"), "got {error}");
}

// ---------------------------------------------------------------------------
// Rotation: a retired public key keeps verifying old tokens
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retired_key_still_verifies() {
    let (old_private, old_public) = ec_key_pair();
    let (new_private, _) = ec_key_pair();
    let old_token = es256_token(&old_private, "2026-10-a", "user-old");

    let files = KeyDir::new();
    let new_private_path = files.write_private("ec-new.pem", &new_private);
    let old_public_path = files.write("ec-old.pub.pem", &old_public);

    let keys = Arc::new(
        JwtKeySet::from_section(
            &signing_section(JwtAlgorithm::Es256, "2026-11-a", &new_private_path, None),
            &[retired_section(
                JwtAlgorithm::Es256,
                "2026-10-a",
                &old_public_path,
            )],
        )
        .expect("rotated key set"),
    );

    // The old token still verifies through the retired public key.
    let resolver = keys.verification_resolver();
    let old_key = resolver
        .resolve("2026-10-a", Algorithm::ES256)
        .await
        .expect("retired kid resolves");
    let decoded = decode::<Value>(&old_token, old_key.as_ref(), &es256_validation())
        .expect("old token verifies");
    assert_eq!(decoded.claims["sub"], "user-old");

    // New tokens are signed with the new active key.
    let issuer = JwtTokenIssuer::from_key_set(Arc::clone(&keys), Duration::from_secs(3600));
    let issued = issuer
        .issue(TokenClaims::new("user-new"))
        .await
        .expect("issue");
    assert_eq!(
        decode_header(&issued.token).expect("header").kid.as_deref(),
        Some("2026-11-a")
    );

    // The published set contains both keys, because old tokens are still live.
    let jwks = keys.jwks();
    assert_eq!(jwks.keys.len(), 2, "JWKS publishes active + retired");
    let kids: Vec<&str> = jwks
        .keys
        .iter()
        .filter_map(|key| key.kid.as_deref())
        .collect();
    assert!(kids.contains(&"2026-11-a"), "active kid is published");
    assert!(kids.contains(&"2026-10-a"), "retired kid is published");
    assert!(kids
        .iter()
        .all(|kid| *kid == "2026-11-a" || *kid == "2026-10-a"));
}

// ---------------------------------------------------------------------------
// Startup negatives
// ---------------------------------------------------------------------------

#[test]
fn hs256_signing_algorithm_is_rejected() {
    let (private, _) = ec_key_pair();
    let files = KeyDir::new();
    let path = files.write_private("ec.pem", &private);
    let error = JwtKeySet::from_section(
        &signing_section(JwtAlgorithm::Hs256, "kid-1", &path, None),
        &[],
    )
    .expect_err("HS256 must not be a Jwt.Signing algorithm");
    assert!(error.code() == "dependency_injection", "got {error:?}");
    assert!(error.to_string().contains("Jwt.Signing"), "got {error}");
}

#[test]
fn blank_key_id_is_rejected() {
    let (private, _) = ec_key_pair();
    let files = KeyDir::new();
    let path = files.write_private("ec.pem", &private);
    for blank in ["", "   ", "\t\n"] {
        let error = JwtKeySet::from_section(
            &signing_section(JwtAlgorithm::Es256, blank, &path, None),
            &[],
        )
        .expect_err("a blank key id must fail startup");
        assert!(
            error.to_string().contains("Jwt.Signing.KeyId"),
            "got {error}"
        );
    }
}

#[test]
fn key_id_with_whitespace_is_rejected() {
    let (private, _) = ec_key_pair();
    let files = KeyDir::new();
    let path = files.write_private("ec.pem", &private);
    let error = JwtKeySet::from_section(
        &signing_section(JwtAlgorithm::Es256, "2026 11 a", &path, None),
        &[],
    )
    .expect_err("a key id with whitespace must fail startup");
    assert!(error.to_string().contains("whitespace"), "got {error}");
}

#[test]
fn missing_or_empty_private_key_file_is_rejected() {
    let (private, _) = ec_key_pair();
    let files = KeyDir::new();
    let empty_path = files.write_private("empty.pem", "\n   \n");
    let missing_path = files.dir.path().join("missing.pem");

    let error = JwtKeySet::from_section(
        &signing_section(
            JwtAlgorithm::Es256,
            "kid-1",
            &missing_path.to_string_lossy(),
            None,
        ),
        &[],
    )
    .expect_err("a missing private key file must fail startup");
    assert!(
        error.to_string().contains("Jwt.Signing.PrivateKeyFile"),
        "got {error}"
    );

    let error = JwtKeySet::from_section(
        &signing_section(JwtAlgorithm::Es256, "kid-1", &empty_path, None),
        &[],
    )
    .expect_err("an empty private key file must fail startup");
    assert!(
        error.to_string().contains("Jwt.Signing.PrivateKeyFile"),
        "got {error}"
    );
    // The existing fixture must still be usable, so the failure above is about
    // the empty file and not about the key parser.
    let good = files.write_private("ec.pem", &private);
    JwtKeySet::from_section(
        &signing_section(JwtAlgorithm::Es256, "kid-1", &good, None),
        &[],
    )
    .expect("a real key file still loads");
}

#[test]
fn short_rsa_key_is_rejected() {
    let files = KeyDir::new();
    let short_path = files.write_private("rsa-1024.pem", TEST_ONLY_SHORT_RSA_PRIVATE_PEM);
    let error = JwtKeySet::from_section(
        &signing_section(JwtAlgorithm::Rs256, "rsa-1", &short_path, None),
        &[],
    )
    .expect_err("a 1024-bit RSA signing key must fail startup");
    assert!(error.to_string().contains("2048"), "got {error}");

    // The 2048-bit fixture is accepted, so the guard is about key strength.
    let good_path = files.write_private("rsa-2048.pem", TEST_ONLY_RSA_PRIVATE_PEM);
    JwtKeySet::from_section(
        &signing_section(JwtAlgorithm::Rs256, "rsa-1", &good_path, None),
        &[],
    )
    .expect("a 2048-bit RSA signing key loads");
}

#[test]
fn mismatched_public_key_file_is_rejected() {
    let (private, _) = ec_key_pair();
    let (_, other_public) = ec_key_pair();
    let files = KeyDir::new();
    let private_path = files.write_private("ec.pem", &private);
    let other_public_path = files.write("other.pub.pem", &other_public);

    let error = JwtKeySet::from_section(
        &signing_section(
            JwtAlgorithm::Es256,
            "kid-1",
            &private_path,
            Some(&other_public_path),
        ),
        &[],
    )
    .expect_err("a public key that does not match the private key must fail startup");
    assert!(
        error.to_string().contains("Jwt.Signing.PublicKeyFile"),
        "got {error}"
    );
}

#[test]
fn matching_public_key_file_is_accepted() {
    let (private, public) = ec_key_pair();
    let files = KeyDir::new();
    let private_path = files.write_private("ec.pem", &private);
    let public_path = files.write("ec.pub.pem", &public);
    JwtKeySet::from_section(
        &signing_section(
            JwtAlgorithm::Es256,
            "kid-1",
            &private_path,
            Some(&public_path),
        ),
        &[],
    )
    .expect("a matching public key file is accepted");
}

#[test]
fn duplicate_retired_key_id_is_rejected() {
    let (private, _) = ec_key_pair();
    let (_, old_public) = ec_key_pair();
    let files = KeyDir::new();
    let private_path = files.write_private("ec.pem", &private);
    let old_public_path = files.write("old.pub.pem", &old_public);

    let error = JwtKeySet::from_section(
        &signing_section(JwtAlgorithm::Es256, "2026-11-a", &private_path, None),
        &[
            retired_section(JwtAlgorithm::Es256, "2026-10-a", &old_public_path),
            retired_section(JwtAlgorithm::Es256, "2026-10-a", &old_public_path),
        ],
    )
    .expect_err("two retired keys must not share a key id");
    assert!(error.to_string().contains("not unique"), "got {error}");
}

#[test]
fn retired_key_must_not_reuse_the_active_key_id() {
    let (private, public) = ec_key_pair();
    let files = KeyDir::new();
    let private_path = files.write_private("ec.pem", &private);
    let public_path = files.write("ec.pub.pem", &public);
    let error = JwtKeySet::from_section(
        &signing_section(JwtAlgorithm::Es256, "kid-1", &private_path, None),
        &[retired_section(JwtAlgorithm::Es256, "kid-1", &public_path)],
    )
    .expect_err("the active key id belongs to one key");
    assert!(error.to_string().contains("not unique"), "got {error}");
}

#[test]
fn retired_hs256_key_is_rejected() {
    let (private, public) = ec_key_pair();
    let files = KeyDir::new();
    let private_path = files.write_private("ec.pem", &private);
    let public_path = files.write("ec.pub.pem", &public);
    let error = JwtKeySet::from_section(
        &signing_section(JwtAlgorithm::Es256, "kid-1", &private_path, None),
        &[retired_section(JwtAlgorithm::Hs256, "kid-0", &public_path)],
    )
    .expect_err("retired entries are public keys, never HS256");
    assert!(
        error.to_string().contains("Jwt.RetiredSigningKeys"),
        "got {error}"
    );
}

#[cfg(unix)]
#[test]
fn group_readable_private_key_is_rejected() {
    use std::os::unix::fs::PermissionsExt;

    let (private, _) = ec_key_pair();
    let files = KeyDir::new();
    let path = files.write_private("ec.pem", &private);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod 644");
    let error = JwtKeySet::from_section(
        &signing_section(JwtAlgorithm::Es256, "kid-1", &path, None),
        &[],
    )
    .expect_err("a group/world-readable private key must fail startup");
    assert!(error.to_string().contains("0600"), "got {error}");
}

// ---------------------------------------------------------------------------
// Host wiring
// ---------------------------------------------------------------------------

fn host_with_signing(
    signing: JwtSigningSection,
    retired: Vec<JwtRetiredSigningKeySection>,
    algorithms: Vec<JwtAlgorithm>,
) -> Result<Host, webx_core::error::Error> {
    Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(move |app| {
            app.options(move |options| {
                options.jwt.secret.clear();
                options.jwt.algorithms = algorithms;
                options.jwt.signing = Some(signing);
                options.jwt.retired_signing_keys = retired;
            })
        })
        .use_authentication()
        .try_build()
}

#[tokio::test]
async fn host_with_signing_issues_es256_tokens_with_kid() {
    let (private, _) = ec_key_pair();
    let files = KeyDir::new();
    let private_path = files.write_private("ec.pem", &private);

    let host = host_with_signing(
        signing_section(JwtAlgorithm::Es256, "2026-11-a", &private_path, None),
        Vec::new(),
        vec![JwtAlgorithm::Es256],
    )
    .expect("a host with Jwt.Signing must build (self-check included)");

    let issuer = host
        .provider()
        .get::<dyn ITokenIssuer>()
        .expect("the asymmetric issuer is registered");
    let issued = issuer
        .issue(TokenClaims::new("user-1"))
        .await
        .expect("issue through the host");
    let header = decode_header(&issued.token).expect("header");
    assert_eq!(header.alg, Algorithm::ES256);
    assert_eq!(header.kid.as_deref(), Some("2026-11-a"));
}

#[test]
fn host_with_signing_and_explicit_jwks_builds_without_fetching() {
    // `Jwt.Jwks` wins as the verification source; the local key set only signs
    // and publishes. Building must not touch the network.
    let (private, _) = ec_key_pair();
    let files = KeyDir::new();
    let private_path = files.write_private("ec.pem", &private);
    let mut jwks = webx_core::config::JwtJwksSection::default();
    jwks.url = "https://id.example.com/.well-known/jwks.json".to_string();

    Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(move |app| {
            app.options(move |options| {
                options.jwt.secret.clear();
                options.jwt.algorithms = vec![JwtAlgorithm::Es256];
                options.jwt.signing = Some(signing_section(
                    JwtAlgorithm::Es256,
                    "2026-11-a",
                    &private_path,
                    None,
                ));
                options.jwt.jwks = Some(jwks);
            })
        })
        .use_authentication()
        .try_build()
        .expect("Jwt.Jwks + Jwt.Signing must build");
}

#[test]
fn host_rejects_signing_algorithm_missing_from_jwt_algorithms() {
    let (private, _) = ec_key_pair();
    let files = KeyDir::new();
    let private_path = files.write_private("ec.pem", &private);
    let error = host_with_signing(
        signing_section(JwtAlgorithm::Es256, "2026-11-a", &private_path, None),
        Vec::new(),
        vec![JwtAlgorithm::Hs256],
    )
    .err()
    .expect("the host must not sign with an algorithm it does not accept");
    assert!(error.code() == "dependency_injection", "got {error:?}");
    assert!(
        error.to_string().contains("Jwt.Signing.Algorithm"),
        "got {error}"
    );
}

#[test]
fn host_rejects_hs256_signing_section() {
    let (private, _) = ec_key_pair();
    let files = KeyDir::new();
    let private_path = files.write_private("ec.pem", &private);
    let error = host_with_signing(
        signing_section(JwtAlgorithm::Hs256, "2026-11-a", &private_path, None),
        Vec::new(),
        vec![JwtAlgorithm::Hs256],
    )
    .err()
    .expect("HS256 in Jwt.Signing must fail startup");
    assert!(error.to_string().contains("Jwt.Signing"), "got {error}");
}

// ---------------------------------------------------------------------------
// Regression: no Jwt.Signing keeps the HS256 path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn host_without_signing_still_issues_hs256() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.options(|options| {
                options.jwt.secret = HS256_SECRET.to_string();
            })
        })
        .use_authentication()
        .try_build()
        .expect("the default HS256 host must still build");

    let issuer = host
        .provider()
        .get::<dyn ITokenIssuer>()
        .expect("the default HS256 issuer is registered");
    let issued = issuer
        .issue(TokenClaims::new("user-1"))
        .await
        .expect("issue HS256 token");

    let header = decode_header(&issued.token).expect("header");
    assert_eq!(header.alg, Algorithm::HS256, "HS256 is unchanged");
    assert!(
        header.kid.is_none(),
        "the HS256 header stays byte-for-byte the pre-F1 header"
    );

    let decoded = decode::<Value>(
        &issued.token,
        &DecodingKey::from_secret(HS256_SECRET.as_bytes()),
        &Validation::new(Algorithm::HS256),
    )
    .expect("HS256 token verifies with Jwt.Secret");
    assert_eq!(decoded.claims["sub"], "user-1");
}

// ---------------------------------------------------------------------------
// Configuration binding
// ---------------------------------------------------------------------------

#[test]
fn config_section_binds_signing_and_retired_keys() {
    let section: JwtSection = serde_json::from_value(json!({
        "Algorithms": ["ES256"],
        "Signing": {
            "Algorithm": "ES256",
            "KeyId": "2026-11-a",
            "PrivateKeyFile": "/run/secrets/jwt-signing.pem",
            "PublicKeyFile": "/run/secrets/jwt-signing.pub.pem"
        },
        "RetiredSigningKeys": [{
            "KeyId": "2026-10-a",
            "Algorithm": "ES256",
            "PublicKeyFile": "/run/secrets/jwt-signing-2026-10.pem"
        }]
    }))
    .expect("the PascalCase Jwt section binds");

    let signing = section.signing.expect("Signing is bound");
    assert_eq!(signing.algorithm, JwtAlgorithm::Es256);
    assert_eq!(signing.key_id, "2026-11-a");
    assert_eq!(signing.private_key_file, "/run/secrets/jwt-signing.pem");
    assert_eq!(
        signing.public_key_file.as_deref(),
        Some("/run/secrets/jwt-signing.pub.pem")
    );
    assert_eq!(section.retired_signing_keys.len(), 1);
    let retired = &section.retired_signing_keys[0];
    assert_eq!(retired.key_id, "2026-10-a");
    assert_eq!(retired.algorithm, JwtAlgorithm::Es256);
    assert_eq!(
        retired.public_key_file,
        "/run/secrets/jwt-signing-2026-10.pem"
    );

    // The default (no `Signing`) keeps the HS256 behaviour.
    let default = JwtSection::default();
    assert!(default.signing.is_none());
    assert!(default.retired_signing_keys.is_empty());
}

/// F1+F2 composition: the local key set is authoritative for its own `kid`s,
/// so a partner IdP outage never rejects self-issued tokens; foreign kids
/// still resolve through the remote chain, and an unknown kid is a 401.
#[tokio::test]
async fn local_key_set_wins_before_remote_sources() {
    let (local_private, local_public) = ec_key_pair();
    let (remote_private, remote_public) = ec_key_pair();

    let local = Arc::new(
        StaticKeyResolver::new()
            .with_pem("local-kid", Algorithm::ES256, local_public.as_bytes())
            .expect("local key"),
    );
    let remote = Arc::new(
        StaticKeyResolver::new()
            .with_pem("remote-kid", Algorithm::ES256, remote_public.as_bytes())
            .expect("remote key"),
    );
    let resolver = LocalFirstKeyResolver::new(local, remote);

    let own = es256_token(&local_private, "local-kid", "user-1");
    let own_header = jsonwebtoken::decode_header(&own).expect("own header");
    assert!(
        resolver
            .resolve(own_header.kid.as_deref().unwrap_or(""), own_header.alg)
            .await
            .is_ok(),
        "a self-issued token must verify from the local key set alone"
    );

    let foreign = es256_token(&remote_private, "remote-kid", "user-2");
    let foreign_header = jsonwebtoken::decode_header(&foreign).expect("foreign header");
    assert!(
        resolver
            .resolve(
                foreign_header.kid.as_deref().unwrap_or(""),
                foreign_header.alg
            )
            .await
            .is_ok(),
        "an unknown local kid falls through to the remote sources"
    );

    let error = resolver
        .resolve("nobody", Algorithm::ES256)
        .await
        .err()
        .expect("unknown everywhere");
    assert_eq!(error.status_code(), 401, "got {error:?}");
}

// ---------------------------------------------------------------------------
// Hardening regressions (security review: M-2, L-1, L-4)
// ---------------------------------------------------------------------------

/// `iss`/`aud`/`nbf` are issuer-owned even when `Jwt.Issuer`/`Jwt.Audience` is
/// not configured: a caller must not be able to choose the token's trust
/// identity through `TokenClaims::extra`.
#[tokio::test]
async fn issuer_and_audience_cannot_be_smuggled_through_extra() {
    let files = KeyDir::new();
    let (private_pem, _) = ec_key_pair();
    let path = files.write_private("signing.pem", &private_pem);
    let keys = Arc::new(
        JwtKeySet::from_section(
            &signing_section(JwtAlgorithm::Es256, "kid-1", &path, None),
            &[],
        )
        .expect("key set"),
    );
    let issuer = JwtTokenIssuer::from_key_set(Arc::clone(&keys), Duration::from_secs(60));
    let issued = issuer
        .issue(
            TokenClaims::new("user-1")
                .with_claim("iss", "https://evil.example.com")
                .with_claim("aud", "evil-audience")
                .with_claim("nbf", "0")
                .with_claim("tenant_id", "acme"),
        )
        .await
        .expect("issue token");

    let mut validation = Validation::new(Algorithm::ES256);
    validation.required_spec_claims.clear();
    validation.validate_aud = false;
    let key = keys
        .verification_resolver()
        .resolve("kid-1", Algorithm::ES256)
        .await
        .expect("verification key");
    let data = jsonwebtoken::decode::<Value>(&issued.token, key.as_ref(), &validation)
        .expect("decode token");
    assert!(
        data.claims.get("iss").is_none(),
        "iss must be stripped from extra: {data:?}"
    );
    assert!(
        data.claims.get("aud").is_none(),
        "aud must be stripped from extra: {data:?}"
    );
    assert!(data.claims.get("nbf").is_none(), "nbf is issuer-owned");
    assert_eq!(data.claims["tenant_id"], "acme", "ordinary claims survive");
}

/// `Jwt.RetiredSigningKeys` without `Jwt.Signing` has no consumer: a configured
/// list is a startup error, not a silently ignored knob.
#[test]
fn retired_signing_keys_require_signing() {
    let mut section = JwtSection::default();
    section.algorithms = vec![JwtAlgorithm::Es256];
    section.retired_signing_keys = vec![retired_section(
        JwtAlgorithm::Es256,
        "2026-10-a",
        "/run/secrets/retired.pem",
    )];
    let error = JwtValidation::from_section(&section)
        .expect_err("retired keys without an active signing key must fail startup");
    assert_eq!(error.code(), "dependency_injection", "got {error:?}");
    assert!(error.to_string().contains("Jwt.Signing"), "got {error}");
}

/// A retired key goes through the same RSA strength floor as the active key:
/// a 1024-bit public key must never keep verifying tokens.
#[test]
fn retired_weak_rsa_public_key_is_rejected() {
    use rsa::pkcs8::{DecodePrivateKey, EncodePublicKey, LineEnding};

    let private = rsa::PrivateKey::from_pkcs8_pem(TEST_ONLY_SHORT_RSA_PRIVATE_PEM)
        .expect("test-only 1024-bit RSA private key");
    let public_pem = private
        .to_public_key()
        .to_public_key_pem(LineEnding::LF)
        .expect("public key PEM");
    let error = Jwk::from_public_key_pem(JwtAlgorithm::Rs256, "old-rsa", public_pem.as_bytes())
        .expect_err("a 1024-bit RSA public key must be rejected");
    assert!(error.to_string().contains("2048"), "got {error}");
}

/// A multi-issuer host must be told which listed issuer it is; the default
/// issuer then stamps that `iss` on self-signed tokens (M-7).
#[tokio::test]
async fn signing_issuer_stamps_multi_issuer_self_signed_tokens() {
    use base64::Engine as _;

    let (private, _) = ec_key_pair();
    let files = KeyDir::new();
    let private_path = files.write_private("ec.pem", &private);
    let self_issuer = "https://auth.example.com";
    let partner = "https://partner.example.com";

    let mut signing = signing_section(JwtAlgorithm::Es256, "2026-11-a", &private_path, None);
    signing.issuer = Some(self_issuer.to_string());
    let source = |issuer: &str| {
        let mut section = JwtJwksSourceSection::default();
        section.issuer = issuer.to_string();
        section.url = format!("{issuer}/.well-known/jwks.json");
        section
    };

    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(move |app| {
            app.options(move |options| {
                options.jwt.secret.clear();
                options.jwt.algorithms = vec![JwtAlgorithm::Es256];
                options.jwt.signing = Some(signing);
                options.jwt.issuers = vec![self_issuer.to_string(), partner.to_string()];
                options.jwt.jwks_sources = vec![source(self_issuer), source(partner)];
            })
        })
        .use_authentication()
        .try_build()
        .expect("multi-issuer host with an explicit Signing.Issuer must build");

    let issuer = host
        .provider()
        .get::<dyn ITokenIssuer>()
        .expect("the asymmetric issuer is registered");
    let issued = issuer
        .issue(TokenClaims::new("user-1"))
        .await
        .expect("issue through the host");
    let payload = issued.token.split('.').nth(1).expect("JWT payload");
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .expect("base64url payload");
    let claims: Value = serde_json::from_slice(&decoded).expect("JSON payload");
    assert_eq!(
        claims.get("iss").and_then(Value::as_str),
        Some(self_issuer),
        "the default issuer must stamp Jwt.Signing.Issuer"
    );

    // A self issuer outside `Jwt.Issuers` is a configuration error.
    let (private, _) = ec_key_pair();
    let files = KeyDir::new();
    let private_path = files.write_private("ec.pem", &private);
    let mut signing = signing_section(JwtAlgorithm::Es256, "2026-11-b", &private_path, None);
    signing.issuer = Some("https://not-trusted.example.com".to_string());
    let error = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(move |app| {
            app.options(move |options| {
                options.jwt.secret.clear();
                options.jwt.algorithms = vec![JwtAlgorithm::Es256];
                options.jwt.signing = Some(signing);
                options.jwt.issuers = vec![self_issuer.to_string(), partner.to_string()];
                options.jwt.jwks_sources = vec![source(self_issuer), source(partner)];
            })
        })
        .use_authentication()
        .try_build()
        .err()
        .expect("an untrusted Signing.Issuer must fail startup");
    assert!(
        error.to_string().contains("Jwt.Signing.Issuer"),
        "got {error}"
    );
}

/// A PEM block in `Jwt.Secret` is a classic footgun: pasting a public key there
/// would let anyone holding the (public) key mint HS256 tokens.
#[test]
fn pem_shaped_hs256_secret_is_rejected() {
    let error = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(|app| {
            app.options(|options| {
                options.jwt.secret =
                    "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8A\n-----END PUBLIC KEY-----"
                        .to_string();
            })
        })
        .use_authentication()
        .try_build()
        .err()
        .expect("a PEM block must not be accepted as an HMAC secret");
    assert!(
        error.to_string().contains("Jwt.Secret"),
        "the error must name Jwt.Secret: {error}"
    );
}
