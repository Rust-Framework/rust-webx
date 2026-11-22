//! At-rest secret protection: AES-256-GCM with a rotatable key ring.
//!
//! [`AesGcmSecretProtector`] implements
//! [`ISecretProtector`](webx_core::secrets::ISecretProtector) for values that
//! must live in a database row (TOTP seeds, provider refresh tokens,
//! one-time signing keys). Ciphertexts are self-describing:
//!
//! ```text
//! webx1:<key-id>:<base64url-no-pad(nonce(12) || AES-256-GCM ciphertext)>
//! ```
//!
//! The embedded key id lets a deployment rotate keys without a data migration:
//! register a ring whose **active** key is new while the old keys stay for
//! reads, and each row is re-encrypted the next time it is written.
//!
//! # Key material
//!
//! Keys are 32 bytes (AES-256). [`AesGcmSecretProtector::from_secret_store`]
//! resolves base64-encoded keys from an
//! [`ISecretStore`](webx_core::secrets::ISecretStore) (environment, mounted
//! files, KMS/Vault adapter), so the key material never lives in
//! `appsettings.json`. [`AesGcmSecretProtector::from_keys`] takes raw bytes for
//! tests and custom loaders.
//!
//! # Deployment semantics
//!
//! * **Single process**: build the ring once at startup; no state on disk.
//! * **Multiple instances**: every node needs the same ring (same secret store
//!   or KMS), otherwise one node cannot read another's rows.
//! * **Containers**: keys are mounted/environment secrets; restart is safe.
//! * **Performance**: one AEAD operation per field (microseconds); do not
//!   protect blobs.

use std::sync::Arc;

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use async_trait::async_trait;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine as _;

use webx_core::secrets::{
    ISecretProtector, ISecretStore, SecretError, SecretValue, PROTECTED_SECRET_PREFIX,
};

/// Diagnostic name used in errors (never the key or the plaintext).
const PROTECTOR_NAME: &str = "secret-protector";

/// AES-256-GCM size of the nonce prepended to each ciphertext.
const NONCE_BYTES: usize = 12;

/// AES-256 key size.
const KEY_BYTES: usize = 32;

struct RingKey {
    id: String,
    cipher: Aes256Gcm,
}

/// AES-256-GCM protector over a rotatable key ring.
pub struct AesGcmSecretProtector {
    keys: Vec<RingKey>,
    active: usize,
}

impl AesGcmSecretProtector {
    /// Build a ring from raw 32-byte keys.
    ///
    /// `active_key_id` must be one of `keys`; ids must be `[A-Za-z0-9._-]{1,64}`
    /// so the ciphertext format stays parseable.
    pub fn from_keys(
        keys: Vec<(String, Vec<u8>)>,
        active_key_id: &str,
    ) -> Result<Self, SecretError> {
        if keys.is_empty() {
            return Err(unavailable("at least one protection key is required"));
        }
        let mut ring = Vec::with_capacity(keys.len());
        for (id, material) in keys {
            validate_key_id(&id)?;
            if material.len() != KEY_BYTES {
                return Err(unavailable(format!(
                    "protection key `{id}` must be {KEY_BYTES} bytes (got {})",
                    material.len()
                )));
            }
            if ring.iter().any(|key: &RingKey| key.id == id) {
                return Err(unavailable(format!("duplicate protection key id `{id}`")));
            }
            let cipher = Aes256Gcm::new_from_slice(&material)
                .map_err(|_| unavailable(format!("protection key `{id}` is invalid")))?;
            ring.push(RingKey { id, cipher });
        }
        let active = ring
            .iter()
            .position(|key| key.id == active_key_id)
            .ok_or_else(|| {
                unavailable(format!(
                    "active protection key `{active_key_id}` is not in the key ring"
                ))
            })?;
        Ok(Self { keys: ring, active })
    }

    /// Resolve base64-encoded keys from an [`ISecretStore`].
    ///
    /// `key_ids` are both the store names and the ids embedded in ciphertexts
    /// (for example `SECRET_PROTECTION_K1`); values must decode to 32 bytes.
    pub async fn from_secret_store(
        store: &dyn ISecretStore,
        key_ids: &[&str],
        active_key_id: &str,
    ) -> Result<Self, SecretError> {
        if key_ids.is_empty() {
            return Err(unavailable("at least one protection key id is required"));
        }
        let mut keys = Vec::with_capacity(key_ids.len());
        for id in key_ids {
            let value = store.get(id).await.map_err(|error| match error {
                SecretError::NotFound(name) => {
                    unavailable(format!("protection key `{name}` is not available"))
                }
                other => other,
            })?;
            let material = decode_key(value.expose())
                .ok_or_else(|| unavailable(format!("protection key `{id}` is not valid base64")))?;
            keys.push(((*id).to_string(), material));
        }
        Self::from_keys(keys, active_key_id)
    }

    /// Number of keys in the ring (rotation diagnostics).
    pub fn key_count(&self) -> usize {
        self.keys.len()
    }
}

#[async_trait]
impl ISecretProtector for AesGcmSecretProtector {
    async fn protect(&self, plaintext: &str) -> Result<String, SecretError> {
        let key = &self.keys[self.active];
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let encrypted = key
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| unavailable("encryption failed"))?;
        let mut payload = Vec::with_capacity(NONCE_BYTES + encrypted.len());
        payload.extend_from_slice(nonce.as_slice());
        payload.extend_from_slice(&encrypted);
        Ok(format!(
            "{PROTECTED_SECRET_PREFIX}:{}:{}",
            key.id,
            URL_SAFE_NO_PAD.encode(payload)
        ))
    }

    async fn unprotect(&self, ciphertext: &str) -> Result<SecretValue, SecretError> {
        let rest = ciphertext
            .strip_prefix(PROTECTED_SECRET_PREFIX)
            .and_then(|rest| rest.strip_prefix(':'))
            .ok_or_else(|| unavailable("ciphertext is not a protected secret"))?;
        let (key_id, payload) = rest
            .split_once(':')
            .ok_or_else(|| unavailable("ciphertext is missing the key id"))?;
        validate_key_id(key_id)?;
        let key = self
            .keys
            .iter()
            .find(|key| key.id == key_id)
            .ok_or_else(|| unavailable(format!("unknown protection key id `{key_id}`")))?;
        let payload = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| unavailable("ciphertext payload is not valid base64"))?;
        if payload.len() <= NONCE_BYTES {
            return Err(unavailable("ciphertext payload is too short"));
        }
        let (nonce, body) = payload.split_at(NONCE_BYTES);
        let nonce = Nonce::from_slice(nonce);
        let plaintext = key
            .cipher
            .decrypt(nonce, body)
            .map_err(|_| unavailable("ciphertext authentication failed"))?;
        let plaintext = String::from_utf8(plaintext)
            .map_err(|_| unavailable("plaintext is not valid UTF-8"))?;
        Ok(SecretValue::new(plaintext))
    }

    fn active_key_id(&self) -> &str {
        &self.keys[self.active].id
    }
}

fn unavailable(detail: impl Into<String>) -> SecretError {
    SecretError::Unavailable {
        name: PROTECTOR_NAME.to_string(),
        detail: detail.into(),
    }
}

fn validate_key_id(id: &str) -> Result<(), SecretError> {
    let valid = !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(unavailable(format!(
            "protection key id `{id}` must match [A-Za-z0-9._-]{{1,64}}"
        )))
    }
}

fn decode_key(value: &str) -> Option<Vec<u8>> {
    let value = value.trim();
    for engine in [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD] {
        if let Ok(bytes) = engine.decode(value) {
            return Some(bytes);
        }
    }
    None
}

/// Build a protector from a base64 key map (test/bootstrap helper).
pub fn protector_from_base64_keys(
    keys: Vec<(String, String)>,
    active_key_id: &str,
) -> Result<AesGcmSecretProtector, SecretError> {
    let mut decoded = Vec::with_capacity(keys.len());
    for (id, value) in keys {
        let material = decode_key(&value)
            .ok_or_else(|| unavailable(format!("protection key `{id}` is not valid base64")))?;
        decoded.push((id, material));
    }
    AesGcmSecretProtector::from_keys(decoded, active_key_id)
}

/// Register-friendly alias.
pub type SharedSecretProtector = Arc<dyn ISecretProtector>;

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;

    fn key(byte: u8) -> Vec<u8> {
        vec![byte; KEY_BYTES]
    }

    fn protector(active: &str) -> AesGcmSecretProtector {
        AesGcmSecretProtector::from_keys(
            vec![("k1".to_string(), key(1)), ("k2".to_string(), key(2))],
            active,
        )
        .expect("valid ring")
    }

    #[tokio::test]
    async fn protect_then_unprotect_round_trips_and_is_randomized() {
        let protector = protector("k2");
        assert_eq!(protector.active_key_id(), "k2");
        assert_eq!(protector.key_count(), 2);

        let first = protector.protect("totp-seed").await.expect("protect");
        let second = protector.protect("totp-seed").await.expect("protect");
        assert_ne!(first, second, "a fresh nonce per encryption");
        assert!(first.starts_with("webx1:k2:"));

        assert_eq!(
            protector
                .unprotect(&first)
                .await
                .expect("unprotect")
                .expose(),
            "totp-seed"
        );
        assert_eq!(
            protector
                .unprotect(&second)
                .await
                .expect("unprotect")
                .expose(),
            "totp-seed"
        );
    }

    #[tokio::test]
    async fn old_key_ciphertext_stays_decryptable_after_rotation() {
        let old = protector("k1");
        let rotated = protector("k2");
        let ciphertext = old.protect("seed").await.expect("protect");

        assert_eq!(
            rotated
                .unprotect(&ciphertext)
                .await
                .expect("old key still readable")
                .expose(),
            "seed"
        );
        let fresh = rotated.protect("seed").await.expect("protect");
        assert!(fresh.starts_with("webx1:k2:"));
    }

    #[tokio::test]
    async fn tampered_or_unknown_ciphertexts_fail_closed() {
        let protector = protector("k2");
        let ciphertext = protector.protect("seed").await.expect("protect");

        // Flip one payload byte: authentication must fail.
        let (head, payload) = ciphertext.rsplit_once(':').expect("format");
        let mut bytes = URL_SAFE_NO_PAD.decode(payload).expect("base64");
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        let tampered = format!("{head}:{}", URL_SAFE_NO_PAD.encode(&bytes));
        assert!(protector.unprotect(&tampered).await.is_err());

        // Unknown key id.
        let unknown = format!("webx1:k9:{}", URL_SAFE_NO_PAD.encode([0u8; 32]));
        assert!(protector.unprotect(&unknown).await.is_err());

        // Wrong/absent algorithm prefix.
        assert!(protector.unprotect("plaintext").await.is_err());
        assert!(protector
            .unprotect("webx2:k2:AAAAAAAAAAAAAAAAAAAAAA")
            .await
            .is_err());
    }

    #[test]
    fn key_ring_validation_rejects_bad_material() {
        assert!(AesGcmSecretProtector::from_keys(vec![], "k1").is_err());
        assert!(
            AesGcmSecretProtector::from_keys(vec![("k1".to_string(), vec![1u8; 16])], "k1")
                .is_err()
        );
        assert!(AesGcmSecretProtector::from_keys(
            vec![("k1".to_string(), key(1)), ("k1".to_string(), key(2)),],
            "k1"
        )
        .is_err());
        assert!(AesGcmSecretProtector::from_keys(vec![("k1".to_string(), key(1))], "k2").is_err());
        assert!(
            AesGcmSecretProtector::from_keys(vec![("bad id".to_string(), key(1))], "bad id")
                .is_err(),
            "key ids are embedded in ciphertext and must be safe"
        );
    }

    #[tokio::test]
    async fn keys_can_be_loaded_from_a_secret_store() {
        let material = STANDARD.encode(key(7));
        let env_name = format!("WEBX_TEST_PROTECTION_KEY_{}", std::process::id());
        std::env::set_var(&env_name, &material);

        let store = webx_core::secrets::EnvSecretStore::new();
        let protector =
            AesGcmSecretProtector::from_secret_store(&store, &[env_name.as_str()], &env_name)
                .await
                .expect("ring from store");
        assert_eq!(protector.active_key_id(), env_name);

        let ciphertext = protector.protect("value").await.expect("protect");
        assert!(ciphertext.starts_with(&format!("webx1:{env_name}:")));
        std::env::remove_var(&env_name);
    }

    #[test]
    fn base64_key_helper_round_trips() {
        let protector =
            protector_from_base64_keys(vec![("k1".to_string(), STANDARD.encode(key(3)))], "k1")
                .expect("valid base64 key");
        assert_eq!(protector.key_count(), 1);
    }
}
