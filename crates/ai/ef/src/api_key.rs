//! Durable API-key records backed by rust-ef.
//!
//! The gateway's hot path only needs [`IApiKeyStore::get`] by key id. This
//! implementation stores the HMAC digest (never the secret) as lowercase hex,
//! plus the tenant, roles, permissions and lifecycle columns an admin surface
//! edits. Wrap it in [`webx_ai::CachedApiKeyStore`] to keep the per-request read
//! off the database; call `revoke`/`upsert` here and invalidate the cache entry
//! through the wrapper so the change is visible immediately.

use async_trait::async_trait;
use rust_ef::prelude::*;
use webx_ai::{now_unix_secs, ApiKeyRecord, ApiKeyStatus, ApiKeyStoreError, IApiKeyStore};

use crate::DbContextFactory;

/// One stored API key.
#[derive(Debug, Clone, EntityType)]
#[table("ai_api_keys")]
pub struct ApiKeyEntity {
    /// Indexed key id (the `sk-{id}-{secret}` middle segment).
    #[primary_key]
    #[max_length(64)]
    pub key_id: String,
    /// Owning tenant.
    #[required]
    #[max_length(64)]
    #[index]
    pub tenant_id: String,
    /// `HMAC-SHA256(pepper, secret)`, lowercase hex.
    #[required]
    #[max_length(128)]
    pub secret_hash: String,
    /// Which pepper produced the digest (supports rotation).
    #[required]
    #[max_length(32)]
    pub pepper_id: String,
    /// JSON array of roles copied into claims.
    #[required]
    pub roles: String,
    /// JSON array of permissions copied into claims.
    #[required]
    pub permissions: String,
    /// `active` / `revoked` / `suspended`.
    #[required]
    #[max_length(16)]
    pub status: String,
    /// Absolute expiry (Unix seconds); `None` = no expiry.
    pub expires_at_unix_secs: Option<i64>,
    /// Revocation time, for audit.
    pub revoked_at_unix_secs: Option<i64>,
    /// Creation time.
    #[required]
    #[index]
    pub created_at_unix_secs: i64,
    /// Last update time.
    #[required]
    pub updated_at_unix_secs: i64,
}

impl ApiKeyEntity {
    fn from_record(record: &ApiKeyRecord, created_at_unix_secs: i64) -> Self {
        Self {
            key_id: record.key_id.clone(),
            tenant_id: record.tenant_id.clone(),
            secret_hash: to_hex(&record.secret_hash),
            pepper_id: record.pepper_id.clone(),
            roles: serde_json::to_string(&record.roles).unwrap_or_else(|_| "[]".to_string()),
            permissions: serde_json::to_string(&record.permissions)
                .unwrap_or_else(|_| "[]".to_string()),
            status: status_name(record.status).to_string(),
            expires_at_unix_secs: record
                .expires_at_unix_secs
                .and_then(|value| i64::try_from(value).ok()),
            revoked_at_unix_secs: record
                .revoked_at_unix_secs
                .and_then(|value| i64::try_from(value).ok()),
            created_at_unix_secs,
            updated_at_unix_secs: i64::try_from(now_unix_secs()).unwrap_or(i64::MAX),
        }
    }

    /// Convert the row back into a record; `None` when the digest column is
    /// corrupt (a corrupt key must never authenticate).
    fn to_record(&self) -> Option<ApiKeyRecord> {
        let Some(secret_hash) = from_hex(&self.secret_hash) else {
            tracing::warn!(
                key_id = %self.key_id,
                "ai_api_keys.secret_hash is not valid hex; refusing to authenticate this key"
            );
            return None;
        };
        Some(ApiKeyRecord {
            key_id: self.key_id.clone(),
            tenant_id: self.tenant_id.clone(),
            secret_hash,
            pepper_id: self.pepper_id.clone(),
            roles: serde_json::from_str(&self.roles).unwrap_or_default(),
            permissions: serde_json::from_str(&self.permissions).unwrap_or_default(),
            status: parse_status(&self.status),
            expires_at_unix_secs: self
                .expires_at_unix_secs
                .and_then(|value| u64::try_from(value).ok()),
            revoked_at_unix_secs: self
                .revoked_at_unix_secs
                .and_then(|value| u64::try_from(value).ok()),
        })
    }
}

/// Durable [`IApiKeyStore`] over rust-ef.
pub struct EfApiKeyStore {
    factory: DbContextFactory,
}

impl EfApiKeyStore {
    /// Build a store over a `DbContext` factory.
    pub fn new(factory: DbContextFactory) -> Self {
        Self { factory }
    }

    /// Wrap a configured options object.
    pub fn from_options(options: std::sync::Arc<DbContextOptions>) -> Self {
        Self::new(std::sync::Arc::new(move || {
            DbContext::from_options(&options)
        }))
    }

    /// Insert a new key or replace the mutable columns of an existing one.
    ///
    /// Returns `true` when a row was inserted. The caller owns cache
    /// invalidation (see [`webx_ai::CachedApiKeyStore::invalidate`]).
    pub async fn upsert(&self, record: ApiKeyRecord) -> EFResult<bool> {
        let mut ctx = (self.factory)()?;
        let key_id = record.key_id.clone();
        let existing = linq!(ctx.set::<ApiKeyEntity>(), |row: ApiKeyEntity| row.key_id
            == key_id)
        .first_or_default()
        .await?;

        match existing {
            Some(mut row) => {
                let created_at = row.created_at_unix_secs;
                row = ApiKeyEntity::from_record(&record, created_at);
                ctx.update(row);
                ctx.save_changes().await?;
                Ok(false)
            }
            None => {
                let created_at = i64::try_from(now_unix_secs()).unwrap_or(i64::MAX);
                ctx.add(ApiKeyEntity::from_record(&record, created_at));
                ctx.save_changes().await?;
                Ok(true)
            }
        }
    }

    /// Mark a key revoked; returns whether it existed.
    pub async fn revoke(&self, key_id: &str) -> EFResult<bool> {
        let mut ctx = (self.factory)()?;
        let key_id = key_id.to_string();
        let Some(mut row) = linq!(ctx.set::<ApiKeyEntity>(), |row: ApiKeyEntity| row.key_id
            == key_id)
        .first_or_default()
        .await?
        else {
            return Ok(false);
        };
        row.status = status_name(ApiKeyStatus::Revoked).to_string();
        row.revoked_at_unix_secs = Some(i64::try_from(now_unix_secs()).unwrap_or(i64::MAX));
        row.updated_at_unix_secs = i64::try_from(now_unix_secs()).unwrap_or(i64::MAX);
        ctx.update(row);
        ctx.save_changes().await?;
        Ok(true)
    }
}

#[async_trait]
impl IApiKeyStore for EfApiKeyStore {
    async fn get(&self, key_id: &str) -> Result<Option<ApiKeyRecord>, ApiKeyStoreError> {
        if key_id.trim().is_empty() {
            return Err(ApiKeyStoreError::Unavailable(
                "empty key id lookup".to_string(),
            ));
        }
        let mut ctx = (self.factory)().map_err(unavailable)?;
        let key_id = key_id.to_string();
        let row = linq!(ctx.set::<ApiKeyEntity>(), |row: ApiKeyEntity| row.key_id
            == key_id)
        .first_or_default()
        .await
        .map_err(unavailable)?;
        Ok(row.and_then(|row| row.to_record()))
    }
}

fn unavailable(error: EFError) -> ApiKeyStoreError {
    ApiKeyStoreError::Unavailable(format!("api key ledger: {error}"))
}

fn status_name(status: ApiKeyStatus) -> &'static str {
    match status {
        ApiKeyStatus::Active => "active",
        ApiKeyStatus::Revoked => "revoked",
        ApiKeyStatus::Suspended => "suspended",
    }
}

fn parse_status(status: &str) -> ApiKeyStatus {
    match status {
        "revoked" => ApiKeyStatus::Revoked,
        "suspended" => ApiKeyStatus::Suspended,
        _ => ApiKeyStatus::Active,
    }
}

fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let (pairs, _) = text.as_bytes().as_chunks::<2>();
    for pair in pairs {
        let high = hex_digit(pair[0])?;
        let low = hex_digit(pair[1])?;
        out.push((high << 4) | low);
    }
    Some(out)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use webx_ai::hash_secret;

    static NEXT_DB: AtomicU64 = AtomicU64::new(0);

    fn test_options() -> (Arc<DbContextOptions>, PathBuf) {
        let sequence = NEXT_DB.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "webx-ai-ef-keys-{}-{}-{}.db",
            std::process::id(),
            now_unix_secs(),
            sequence
        ));
        let mut builder = DbContextOptionsBuilder::new();
        builder.use_sqlite(&path.to_string_lossy());
        let options = Arc::new(builder.build());
        options.create_provider().expect("provider");
        (options, path)
    }

    async fn prepare(options: &Arc<DbContextOptions>) {
        let ctx = DbContext::from_options(options).expect("context");
        ctx.ensure_created().await.expect("schema");
    }

    fn record(key_id: &str) -> ApiKeyRecord {
        ApiKeyRecord::new(
            key_id,
            "tenant-1",
            hash_secret(b"pepper-v1", "0123456789abcdef").to_vec(),
            "v1",
        )
        .with_roles(["admin"])
        .with_permissions(["chat.write"])
    }

    #[tokio::test]
    async fn upserts_and_reads_a_key() {
        let (options, path) = test_options();
        prepare(&options).await;
        let store = EfApiKeyStore::from_options(Arc::clone(&options));

        assert!(store.upsert(record("key-1")).await.expect("insert"));
        let loaded = store.get("key-1").await.expect("get").expect("exists");
        assert_eq!(loaded.tenant_id, "tenant-1");
        assert_eq!(loaded.pepper_id, "v1");
        assert_eq!(loaded.roles, vec!["admin".to_string()]);
        assert_eq!(loaded.permissions, vec!["chat.write".to_string()]);
        assert_eq!(loaded.secret_hash.len(), 32);
        assert_eq!(loaded.status, ApiKeyStatus::Active);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn upsert_updates_an_existing_row_without_duplicating_it() {
        let (options, path) = test_options();
        prepare(&options).await;
        let store = EfApiKeyStore::from_options(Arc::clone(&options));
        store.upsert(record("key-1")).await.expect("insert");

        let mut updated = record("key-1");
        updated.roles = vec!["viewer".to_string()];
        assert!(!store.upsert(updated).await.expect("update"));

        let loaded = store.get("key-1").await.expect("get").expect("exists");
        assert_eq!(loaded.roles, vec!["viewer".to_string()]);

        let mut ctx = DbContext::from_options(&options).expect("context");
        let count: i64 = linq!(ctx.set::<ApiKeyEntity>(); count)
            .await
            .expect("count");
        assert_eq!(count, 1);
        drop(ctx);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn revoke_sets_status_and_revocation_time() {
        let (options, path) = test_options();
        prepare(&options).await;
        let store = EfApiKeyStore::from_options(Arc::clone(&options));
        store.upsert(record("key-1")).await.expect("insert");

        assert!(store.revoke("key-1").await.expect("revoke"));
        let loaded = store.get("key-1").await.expect("get").expect("exists");
        assert_eq!(loaded.status, ApiKeyStatus::Revoked);
        assert!(loaded.revoked_at_unix_secs.is_some());
        assert!(!loaded.is_active_at(now_unix_secs()));
        assert!(!store.revoke("missing").await.expect("missing"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn hex_round_trips_and_rejects_malformed_input() {
        let bytes = [0x00u8, 0x0f, 0xa5, 0xff];
        assert_eq!(to_hex(&bytes), "000fa5ff");
        assert_eq!(from_hex("000fa5ff"), Some(bytes.to_vec()));
        assert_eq!(from_hex("0"), None);
        assert_eq!(from_hex("zz"), None);
    }
}
