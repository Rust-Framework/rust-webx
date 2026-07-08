//! In-process memory cache —matches ASP.NET Core's `MemoryCache`.
//!
//! Implements [`IDistributedCache`] and provides typed access
//! via [`DistributedCacheExtensions`].

use rust_webx_core::cache::options::DistributedCacheEntryOptions;
use rust_webx_core::cache::trait_def::{CacheError, IDistributedCache, Result};
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::RwLock;

struct CacheEntry {
    data: Vec<u8>,
    expires_at: Option<Instant>,
    sliding_ttl: Option<std::time::Duration>,
}

impl CacheEntry {
    fn new(data: Vec<u8>, options: &DistributedCacheEntryOptions) -> Self {
        let expires_at = options
            .absolute_expiration_relative_to_now
            .map(|d| Instant::now() + d);
        let sliding_ttl = options.sliding_expiration;
        let expires_at = if expires_at.is_some() {
            expires_at
        } else {
            sliding_ttl.map(|d| Instant::now() + d)
        };
        Self {
            data,
            expires_at,
            sliding_ttl,
        }
    }
    fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|t| Instant::now() >= t)
    }
    fn refresh(&mut self) {
        if let Some(ttl) = self.sliding_ttl {
            self.expires_at = Some(Instant::now() + ttl);
        }
    }
}

pub struct MemoryCache {
    inner: RwLock<HashMap<String, CacheEntry>>,
    max_entries: usize,
}

impl MemoryCache {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            max_entries: 0,
        }
    }
    pub fn with_max_entries(mut self, n: usize) -> Self {
        self.max_entries = n;
        self
    }
    pub async fn compact(&self) {
        self.inner.write().await.retain(|_, v| !v.is_expired());
    }
    pub async fn count(&self) -> usize {
        self.inner.read().await.len()
    }
}

impl Default for MemoryCache {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl IDistributedCache for MemoryCache {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let mut inner = self.inner.write().await;
        match inner.get_mut(key) {
            Some(e) if !e.is_expired() => {
                e.refresh();
                Ok(Some(e.data.clone()))
            }
            Some(_) => {
                inner.remove(key);
                Ok(None)
            }
            None => Ok(None),
        }
    }

    async fn set(
        &self,
        key: &str,
        value: Vec<u8>,
        options: Option<&DistributedCacheEntryOptions>,
    ) -> Result<()> {
        let opts = options.cloned().unwrap_or_default();
        if opts.size_limit > 0 && value.len() > opts.size_limit {
            return Err(CacheError::Message(format!(
                "size {} exceeds limit {}",
                value.len(),
                opts.size_limit
            )));
        }
        let mut inner = self.inner.write().await;
        if self.max_entries > 0 && inner.len() >= self.max_entries && !inner.contains_key(key) {
            inner.retain(|_, v| !v.is_expired());
            if inner.len() >= self.max_entries {
                if let Some(k) = inner.keys().next().cloned() {
                    inner.remove(&k);
                }
            }
        }
        inner.insert(key.to_string(), CacheEntry::new(value, &opts));
        Ok(())
    }

    async fn remove(&self, key: &str) -> Result<()> {
        self.inner.write().await.remove(key);
        Ok(())
    }

    async fn refresh(&self, key: &str) -> Result<()> {
        let mut inner = self.inner.write().await;
        if let Some(e) = inner.get_mut(key) {
            e.refresh();
        }
        Ok(())
    }

    async fn exists(&self, key: &str) -> Result<bool> {
        let mut inner = self.inner.write().await;
        match inner.get_mut(key) {
            Some(e) if !e.is_expired() => {
                e.refresh();
                Ok(true)
            }
            Some(_) => {
                inner.remove(key);
                Ok(false)
            }
            None => Ok(false),
        }
    }

    async fn clear(&self) -> Result<()> {
        self.inner.write().await.clear();
        Ok(())
    }
}
