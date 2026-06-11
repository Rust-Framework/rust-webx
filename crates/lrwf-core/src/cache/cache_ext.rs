//! Typed cache extensions — matches ASP.NET Core's `DistributedCacheExtensions`.
//!
//| ASP.NET Core                          | LRWF                                    |
//|---------------------------------------|-----------------------------------------|
//| `GetStringAsync(cache, key, token)`   | `get_string(&self, key)`                |
//| `SetStringAsync(cache, key, val, opts)`| `set_string(&self, key, &val, opts)`    |

use super::options::DistributedCacheEntryOptions;
use super::trait_def::{CacheError, IDistributedCache, Result};
use std::future::Future;

#[async_trait::async_trait]
pub trait DistributedCacheExtensions: IDistributedCache {
    async fn get_string<T: serde::de::DeserializeOwned + Send>(
        &self,
        key: &str,
    ) -> Result<Option<T>>;
    async fn set_string<T: serde::Serialize + Send + Sync>(
        &self,
        key: &str,
        val: &T,
        opts: &DistributedCacheEntryOptions,
    ) -> Result<()>;
    async fn get_or_create<T, F, Fut>(
        &self,
        key: &str,
        factory: F,
        opts: &DistributedCacheEntryOptions,
    ) -> Result<T>
    where
        T: serde::Serialize + serde::de::DeserializeOwned + Send + Sync + Clone + 'static,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = T> + Send;
    async fn get_or_try_create<T, F, Fut, E>(
        &self,
        key: &str,
        factory: F,
        opts: &DistributedCacheEntryOptions,
    ) -> Result<T>
    where
        T: serde::Serialize + serde::de::DeserializeOwned + Send + Sync + Clone + 'static,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = std::result::Result<T, E>> + Send,
        E: std::fmt::Display;
}

#[async_trait::async_trait]
impl<T: IDistributedCache + ?Sized + Sync> DistributedCacheExtensions for T {
    async fn get_string<U: serde::de::DeserializeOwned + Send>(
        &self,
        key: &str,
    ) -> Result<Option<U>> {
        let bytes = self.get(key).await?;
        match bytes {
            Some(data) => Ok(Some(
                serde_json::from_slice(&data)
                    .map_err(|e| CacheError::Serialization(e.to_string()))?,
            )),
            None => Ok(None),
        }
    }

    async fn set_string<U: serde::Serialize + Send + Sync>(
        &self,
        key: &str,
        val: &U,
        opts: &DistributedCacheEntryOptions,
    ) -> Result<()> {
        let data = serde_json::to_vec(val).map_err(|e| CacheError::Serialization(e.to_string()))?;
        if opts.size_limit > 0 && data.len() > opts.size_limit {
            return Err(CacheError::Message(format!(
                "value size {} exceeds limit {}",
                data.len(),
                opts.size_limit
            )));
        }
        self.set(key, data, Some(opts)).await
    }

    async fn get_or_create<U, F, Fut>(
        &self,
        key: &str,
        factory: F,
        opts: &DistributedCacheEntryOptions,
    ) -> Result<U>
    where
        U: serde::Serialize + serde::de::DeserializeOwned + Send + Sync + Clone + 'static,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = U> + Send,
    {
        if let Some(val) = self.get_string::<U>(key).await? {
            return Ok(val);
        }
        let val = factory().await;
        self.set_string(key, &val, opts).await?;
        Ok(val)
    }

    async fn get_or_try_create<U, F, Fut, E>(
        &self,
        key: &str,
        factory: F,
        opts: &DistributedCacheEntryOptions,
    ) -> Result<U>
    where
        U: serde::Serialize + serde::de::DeserializeOwned + Send + Sync + Clone + 'static,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = std::result::Result<U, E>> + Send,
        E: std::fmt::Display,
    {
        if let Some(val) = self.get_string::<U>(key).await? {
            return Ok(val);
        }
        let val = factory()
            .await
            .map_err(|e| CacheError::Message(e.to_string()))?;
        self.set_string(key, &val, opts).await?;
        Ok(val)
    }
}
