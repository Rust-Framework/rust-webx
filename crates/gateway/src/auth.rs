//! API-key validator backed by [`IApiKeyStore`].
//!
//! The key id is the indexed lookup; the secret is verified with
//! `HMAC-SHA256(pepper, secret)` in constant time. Revocation and expiry are
//! checked on every request, so a revoked key fails on the very next call.

use std::collections::HashMap;
use std::sync::Arc;

use webx::{async_trait, ApiKeyClaims, Error, IApiKeyValidator, IClaims, Result};
use webx_ai::{now_unix_secs, parse_presented, verify_secret, IApiKeyStore, DEFAULT_PREFIX};

/// Validates presented keys against a store and a set of peppers.
pub struct StoreApiKeyValidator {
    store: Arc<dyn IApiKeyStore>,
    peppers: HashMap<String, Vec<u8>>,
    prefix: String,
}

impl StoreApiKeyValidator {
    /// A validator with one pepper (`pepper_id` is stored on each record).
    pub fn new(
        store: Arc<dyn IApiKeyStore>,
        pepper_id: impl Into<String>,
        pepper: impl Into<Vec<u8>>,
    ) -> Self {
        let mut peppers = HashMap::new();
        peppers.insert(pepper_id.into(), pepper.into());
        Self {
            store,
            peppers,
            prefix: DEFAULT_PREFIX.to_string(),
        }
    }

    /// Register an additional pepper (rotation: old keys keep working).
    pub fn with_pepper(mut self, pepper_id: impl Into<String>, pepper: impl Into<Vec<u8>>) -> Self {
        self.peppers.insert(pepper_id.into(), pepper.into());
        self
    }

    /// Override the key prefix (default `sk-`).
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }
}

#[async_trait]
impl IApiKeyValidator for StoreApiKeyValidator {
    async fn validate(&self, presented_key: &str) -> Result<Option<Box<dyn IClaims>>> {
        let Some(parsed) = parse_presented(presented_key, &self.prefix) else {
            return Ok(None);
        };
        let record = match self.store.get(parsed.key_id).await {
            Ok(Some(record)) => record,
            Ok(None) => return Ok(None),
            Err(error) => {
                return Err(Error::Internal(format!(
                    "api key store unavailable: {error}"
                )));
            }
        };
        if !record.is_active_at(now_unix_secs()) {
            return Ok(None);
        }
        let Some(pepper) = self.peppers.get(&record.pepper_id) else {
            return Ok(None);
        };
        if !verify_secret(pepper, parsed.secret, &record.secret_hash) {
            return Ok(None);
        }

        let mut claims = ApiKeyClaims::new(record.key_id)
            .with_tenant(record.tenant_id)
            .with_roles(record.roles)
            .with_permissions(record.permissions);
        claims = claims.with_claim("pepper_id", record.pepper_id);
        Ok(Some(Box::new(claims)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webx_ai::{hash_secret, ApiKeyRecord, ApiKeyStatus, ApiKeyStoreError, MemoryApiKeyStore};

    const SECRET: &str = "0123456789abcdef";

    fn store() -> Arc<MemoryApiKeyStore> {
        let store = MemoryApiKeyStore::new();
        store.insert(
            ApiKeyRecord::new(
                "key-1",
                "tenant-1",
                hash_secret(b"pepper-v1", SECRET).to_vec(),
                "v1",
            )
            .with_roles(["admin"])
            .with_permissions(["chat.write"]),
        );
        Arc::new(store)
    }

    fn validator(store: Arc<MemoryApiKeyStore>) -> StoreApiKeyValidator {
        StoreApiKeyValidator::new(store, "v1", b"pepper-v1".to_vec())
    }

    #[tokio::test]
    async fn valid_key_yields_tenant_roles_and_permissions() {
        let validator = validator(store());
        let claims = validator
            .validate(&format!("sk-key-1-{SECRET}"))
            .await
            .expect("validate")
            .expect("claims");
        assert_eq!(claims.subject(), "key-1");
        assert_eq!(claims.get_tenantid(), Some("tenant-1"));
        assert_eq!(claims.roles().len(), 1);
        assert_eq!(claims.roles()[0], "admin");
        assert_eq!(claims.permissions().len(), 1);
        assert_eq!(claims.permissions()[0], "chat.write");
    }

    #[tokio::test]
    async fn wrong_secret_unknown_key_and_bad_prefix_are_anonymous() {
        let validator = validator(store());
        assert!(validator
            .validate("sk-key-1-ffffffffffffffff")
            .await
            .expect("validate")
            .is_none());
        assert!(validator
            .validate("sk-missing-0123456789abcdef")
            .await
            .expect("validate")
            .is_none());
        assert!(validator
            .validate("ak-key-1-0123456789abcdef")
            .await
            .expect("validate")
            .is_none());
    }

    #[tokio::test]
    async fn revoked_and_expired_keys_are_rejected_immediately() {
        let store = store();
        let keys = validator(Arc::clone(&store));
        assert!(store.revoke("key-1"));
        assert!(keys
            .validate(&format!("sk-key-1-{SECRET}"))
            .await
            .expect("validate")
            .is_none());

        let store = Arc::new(MemoryApiKeyStore::new());
        store.insert(
            ApiKeyRecord::new(
                "key-1",
                "tenant-1",
                hash_secret(b"pepper-v1", SECRET).to_vec(),
                "v1",
            )
            .with_expiry(1),
        );
        assert!(validator(store)
            .validate(&format!("sk-key-1-{SECRET}"))
            .await
            .expect("validate")
            .is_none());
    }

    #[tokio::test]
    async fn unknown_pepper_is_rejected() {
        let store = Arc::new(MemoryApiKeyStore::new());
        store.insert(ApiKeyRecord::new(
            "key-1",
            "tenant-1",
            hash_secret(b"other", SECRET).to_vec(),
            "v2",
        ));
        assert!(validator(store)
            .validate(&format!("sk-key-1-{SECRET}"))
            .await
            .expect("validate")
            .is_none());
    }

    struct FailingStore;

    #[async_trait]
    impl IApiKeyStore for FailingStore {
        async fn get(
            &self,
            _key_id: &str,
        ) -> std::result::Result<Option<ApiKeyRecord>, ApiKeyStoreError> {
            Err(ApiKeyStoreError::Unavailable("db down".into()))
        }
    }

    #[tokio::test]
    async fn store_failure_is_a_server_error() {
        let validator =
            StoreApiKeyValidator::new(Arc::new(FailingStore), "v1", b"pepper-v1".to_vec());
        let error = validator
            .validate(&format!("sk-key-1-{SECRET}"))
            .await
            .err()
            .expect("store failure");
        assert_eq!(error.status_code(), 500);
    }

    #[test]
    fn records_with_suspended_status_are_inactive() {
        let mut record = ApiKeyRecord::new("key-1", "tenant-1", vec![], "v1");
        record.status = ApiKeyStatus::Suspended;
        assert!(!record.is_active_at(now_unix_secs()));
    }
}
