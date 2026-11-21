use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use webx_core::cache::{CacheError, CacheResult, DistributedCacheEntryOptions, IDistributedCache};

use crate::connection::RedisConnection;
use crate::options::RedisOptions;

/// Header byte identifying this cache's envelope. Values written by other
/// clients (or an older rust-webx build) are returned unchanged.
const ENTRY_VERSION: u8 = 1;
const ENTRY_HEADER_LEN: usize = 13;

/// Redis-backed [`IDistributedCache`].
///
/// Values carry a 13-byte envelope (version, absolute expiry, sliding window)
/// so multi-instance deployments implement expiration and sliding refresh
/// without a process-local registry. A `GET` refreshes the sliding window;
/// an expired absolute deadline deletes the key and returns `None`.
pub struct RedisCache {
    connection: Arc<RedisConnection>,
    key_prefix: String,
    default_ttl: Option<Duration>,
    sliding_ttl: Option<Duration>,
}

impl RedisCache {
    /// Create a cache over a shared Redis connection.
    pub fn new(connection: Arc<RedisConnection>, options: &RedisOptions) -> Self {
        Self {
            connection,
            key_prefix: options.key_prefix.clone(),
            default_ttl: options.default_ttl(),
            sliding_ttl: options.sliding_ttl(),
        }
    }

    /// Prefix applied to every key.
    pub fn key_prefix(&self) -> &str {
        &self.key_prefix
    }

    fn key(&self, key: &str) -> String {
        if self.key_prefix.is_empty() {
            key.to_string()
        } else {
            format!("{}{key}", self.key_prefix)
        }
    }

    async fn connection(&self) -> CacheResult<redis::aio::ConnectionManager> {
        self.connection.manager().await
    }
}

#[async_trait::async_trait]
impl IDistributedCache for RedisCache {
    async fn get(&self, key: &str) -> CacheResult<Option<Vec<u8>>> {
        let key = self.key(key);
        let mut connection = self.connection().await?;
        let raw: Option<Vec<u8>> = redis::cmd("GET")
            .arg(&key)
            .query_async(&mut connection)
            .await
            .map_err(unavailable)?;
        let Some(raw) = raw else {
            return Ok(None);
        };

        // Values without our envelope stay readable (drop-in cache migration).
        let Some(entry) = decode_entry(&raw) else {
            return Ok(Some(raw));
        };

        let now = unix_millis();
        if entry.absolute_ms != 0 && now >= entry.absolute_ms {
            let _ = remove_key(&mut connection, &key).await;
            return Ok(None);
        }

        if entry.sliding_ms != 0 {
            let ttl = sliding_ttl(entry, now);
            if ttl > 0 {
                if let Err(error) = expire_key(&mut connection, &key, ttl).await {
                    tracing::warn!(key = %key, error = %error, "Redis sliding refresh failed");
                }
            }
        }

        Ok(Some(entry.payload.to_vec()))
    }

    async fn set(
        &self,
        key: &str,
        value: Vec<u8>,
        options: Option<&DistributedCacheEntryOptions>,
    ) -> CacheResult<()> {
        if let Some(options) = options {
            if options.size_limit > 0 && value.len() > options.size_limit {
                return Err(CacheError::Message(format!(
                    "value size {} exceeds limit {}",
                    value.len(),
                    options.size_limit
                )));
            }
        }

        let key = self.key(key);
        let now = unix_millis();
        let explicit_sliding = options.and_then(|options| options.sliding_expiration);
        // Per-entry expiration options override the cache defaults. The
        // default absolute TTL only fills the "no expiry requested" case.
        let requested_absolute = options
            .and_then(|options| options.absolute_expiration_relative_to_now)
            .or_else(|| {
                if explicit_sliding.is_none() {
                    self.default_ttl
                } else {
                    None
                }
            });
        let requested_sliding = explicit_sliding.or(self.sliding_ttl);

        let absolute_ms = requested_absolute
            .map(|ttl| now.saturating_add(duration_ms(ttl)))
            .unwrap_or(0);
        let sliding_ms = requested_sliding
            .map(|ttl| duration_ms(ttl).min(u32::MAX as u64) as u32)
            .unwrap_or(0);

        let mut connection = self.connection().await?;
        if requested_absolute.is_some() && absolute_ms <= now {
            remove_key(&mut connection, &key).await?;
            return Ok(());
        }

        let frame = encode_entry(absolute_ms, sliding_ms, &value);
        let ttl_ms = if absolute_ms > now {
            absolute_ms - now
        } else {
            sliding_ms as u64
        };

        let mut command = redis::cmd("SET");
        command.arg(&key).arg(frame);
        if ttl_ms > 0 {
            command.arg("PX").arg(ttl_ms.min(i64::MAX as u64));
        }
        command
            .query_async::<()>(&mut connection)
            .await
            .map_err(unavailable)
    }

    async fn remove(&self, key: &str) -> CacheResult<()> {
        let key = self.key(key);
        let mut connection = self.connection().await?;
        remove_key(&mut connection, &key).await
    }

    async fn refresh(&self, key: &str) -> CacheResult<()> {
        let key = self.key(key);
        let mut connection = self.connection().await?;
        let raw: Option<Vec<u8>> = redis::cmd("GET")
            .arg(&key)
            .query_async(&mut connection)
            .await
            .map_err(unavailable)?;
        let Some(raw) = raw else {
            return Ok(());
        };
        let Some(entry) = decode_entry(&raw) else {
            return Ok(());
        };
        let now = unix_millis();
        if entry.absolute_ms != 0 && now >= entry.absolute_ms {
            return remove_key(&mut connection, &key).await;
        }
        if entry.sliding_ms != 0 {
            let ttl = sliding_ttl(entry, now);
            if ttl > 0 {
                expire_key(&mut connection, &key, ttl).await?;
            }
        }
        Ok(())
    }

    async fn exists(&self, key: &str) -> CacheResult<bool> {
        let key = self.key(key);
        let mut connection = self.connection().await?;
        let count: i64 = redis::cmd("EXISTS")
            .arg(&key)
            .query_async(&mut connection)
            .await
            .map_err(unavailable)?;
        Ok(count > 0)
    }

    async fn clear(&self) -> CacheResult<()> {
        // Refuse to scan/drop the whole database: `clear` only makes sense for
        // a namespaced cache.
        if self.key_prefix.is_empty() {
            tracing::warn!(
                "Redis cache clear() without a key_prefix is a no-op; set RedisOptions.key_prefix"
            );
            return Ok(());
        }

        let pattern = format!("{}*", self.key_prefix);
        let mut connection = self.connection().await?;
        let mut cursor = 0u64;
        loop {
            let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(500)
                .query_async(&mut connection)
                .await
                .map_err(unavailable)?;
            if !keys.is_empty() {
                redis::cmd("DEL")
                    .arg(keys)
                    .query_async::<i64>(&mut connection)
                    .await
                    .map_err(unavailable)?;
            }
            if next == 0 {
                break;
            }
            cursor = next;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct DecodedEntry<'a> {
    absolute_ms: u64,
    sliding_ms: u32,
    payload: &'a [u8],
}

fn encode_entry(absolute_ms: u64, sliding_ms: u32, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(ENTRY_HEADER_LEN + payload.len());
    frame.push(ENTRY_VERSION);
    frame.extend_from_slice(&absolute_ms.to_be_bytes());
    frame.extend_from_slice(&sliding_ms.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn decode_entry(bytes: &[u8]) -> Option<DecodedEntry<'_>> {
    if bytes.len() < ENTRY_HEADER_LEN || bytes[0] != ENTRY_VERSION {
        return None;
    }
    let absolute_ms = u64::from_be_bytes(bytes[1..9].try_into().ok()?);
    let sliding_ms = u32::from_be_bytes(bytes[9..13].try_into().ok()?);
    Some(DecodedEntry {
        absolute_ms,
        sliding_ms,
        payload: &bytes[ENTRY_HEADER_LEN..],
    })
}

fn sliding_ttl(entry: DecodedEntry<'_>, now: u64) -> u64 {
    let mut ttl = entry.sliding_ms as u64;
    if entry.absolute_ms != 0 {
        ttl = ttl.min(entry.absolute_ms.saturating_sub(now));
    }
    ttl
}

async fn remove_key(connection: &mut redis::aio::ConnectionManager, key: &str) -> CacheResult<()> {
    redis::cmd("DEL")
        .arg(key)
        .query_async::<i64>(connection)
        .await
        .map(|_| ())
        .map_err(unavailable)
}

async fn expire_key(
    connection: &mut redis::aio::ConnectionManager,
    key: &str,
    ttl_ms: u64,
) -> CacheResult<()> {
    redis::cmd("PEXPIRE")
        .arg(key)
        .arg(ttl_ms.min(i64::MAX as u64))
        .query_async::<i64>(connection)
        .await
        .map(|_| ())
        .map_err(unavailable)
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(duration_ms)
        .unwrap_or(0)
}

fn unavailable(error: redis::RedisError) -> CacheError {
    CacheError::Unavailable(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_header_round_trips_the_payload() {
        let frame = encode_entry(1234, 56, b"payload");
        let decoded = decode_entry(&frame).expect("header");
        assert_eq!(decoded.absolute_ms, 1234);
        assert_eq!(decoded.sliding_ms, 56);
        assert_eq!(decoded.payload, b"payload");
    }

    #[test]
    fn unknown_or_truncated_values_are_legacy_raw_values() {
        assert!(decode_entry(b"plain").is_none());
        assert!(decode_entry(&[ENTRY_VERSION; ENTRY_HEADER_LEN - 1]).is_none());
    }

    #[test]
    fn sliding_ttl_is_capped_by_the_absolute_deadline() {
        let frame = encode_entry(1_000, 5_000, b"x");
        let decoded = decode_entry(&frame).expect("header");
        assert_eq!(sliding_ttl(decoded, 0), 1_000);
    }
}
