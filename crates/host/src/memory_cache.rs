//! In-process memory cache —matches ASP.NET Core's `MemoryCache`.
//!
//! Implements [`IDistributedCache`] and provides typed access
//! via [`DistributedCacheExtensions`].

use rust_webx_core::cache::options::DistributedCacheEntryOptions;
use rust_webx_core::cache::trait_def::{CacheError, IDistributedCache, Result};
use std::collections::{HashMap, VecDeque};
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

struct CacheInner {
    entries: HashMap<String, CacheEntry>,
    insertion_order: VecDeque<String>,
}

impl CacheInner {
    fn evict_expired(&mut self) {
        let expired: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, v)| v.is_expired())
            .map(|(k, _)| k.clone())
            .collect();
        for k in expired {
            self.entries.remove(&k);
            self.insertion_order.retain(|x| x != &k);
        }
    }
}

pub struct MemoryCache {
    inner: RwLock<CacheInner>,
    max_entries: usize,
}

impl MemoryCache {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(CacheInner {
                entries: HashMap::new(),
                insertion_order: VecDeque::new(),
            }),
            max_entries: 0,
        }
    }
    pub fn with_max_entries(mut self, n: usize) -> Self {
        self.max_entries = n;
        self
    }
    pub async fn compact(&self) {
        let mut inner = self.inner.write().await;
        inner.evict_expired();
    }
    pub async fn count(&self) -> usize {
        self.inner.read().await.entries.len()
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
        // Fast path: read lock — clone data, check freshness
        {
            let inner = self.inner.read().await;
            match inner.entries.get(key) {
                Some(e) if !e.is_expired() => {
                    let data = e.data.clone();
                    let needs_refresh = e.sliding_ttl.is_some();
                    drop(inner);
                    if needs_refresh {
                        let mut inner = self.inner.write().await;
                        if let Some(e) = inner.entries.get_mut(key) {
                            e.refresh();
                        }
                    }
                    return Ok(Some(data));
                }
                Some(_) => {}
                None => return Ok(None),
            }
        }
        // Slow path: expired entry — remove under write lock
        let mut inner = self.inner.write().await;
        match inner.entries.get_mut(key) {
            Some(e) if !e.is_expired() => {
                e.refresh();
                Ok(Some(e.data.clone()))
            }
            Some(_) => {
                inner.entries.remove(key);
                inner.insertion_order.retain(|x| x != key);
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
        let is_new = !inner.entries.contains_key(key);

        if is_new && self.max_entries > 0 && inner.entries.len() >= self.max_entries {
            inner.evict_expired();
            while inner.entries.len() >= self.max_entries {
                match inner.insertion_order.pop_front() {
                    Some(k) => {
                        inner.entries.remove(&k);
                    }
                    None => break,
                }
            }
        }

        inner
            .entries
            .insert(key.to_string(), CacheEntry::new(value, &opts));
        if is_new {
            inner.insertion_order.push_back(key.to_string());
        }
        Ok(())
    }

    async fn remove(&self, key: &str) -> Result<()> {
        let mut inner = self.inner.write().await;
        inner.entries.remove(key);
        inner.insertion_order.retain(|x| x != key);
        Ok(())
    }

    async fn refresh(&self, key: &str) -> Result<()> {
        let mut inner = self.inner.write().await;
        if let Some(e) = inner.entries.get_mut(key) {
            e.refresh();
        }
        Ok(())
    }

    async fn exists(&self, key: &str) -> Result<bool> {
        // Fast path: read lock
        {
            let inner = self.inner.read().await;
            match inner.entries.get(key) {
                Some(e) if !e.is_expired() => {
                    let needs_refresh = e.sliding_ttl.is_some();
                    drop(inner);
                    if needs_refresh {
                        let mut inner = self.inner.write().await;
                        if let Some(e) = inner.entries.get_mut(key) {
                            e.refresh();
                        }
                    }
                    return Ok(true);
                }
                Some(_) => {}
                None => return Ok(false),
            }
        }
        // Slow path: expired entry — remove under write lock
        let mut inner = self.inner.write().await;
        match inner.entries.get_mut(key) {
            Some(e) if !e.is_expired() => {
                e.refresh();
                Ok(true)
            }
            Some(_) => {
                inner.entries.remove(key);
                inner.insertion_order.retain(|x| x != key);
                Ok(false)
            }
            None => Ok(false),
        }
    }

    async fn clear(&self) -> Result<()> {
        let mut inner = self.inner.write().await;
        inner.entries.clear();
        inner.insertion_order.clear();
        Ok(())
    }
}
