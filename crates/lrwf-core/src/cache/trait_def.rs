//! Core caching trait — `IDistributedCache`.
//!
//! Matches ASP.NET Core `IDistributedCache` interface:
//| ASP.NET Core                    | LRWF                            |
//|---------------------------------|---------------------------------|
//| `GetAsync(string, tok)`         | `get(&self, key)`               |
//| `SetAsync(string, byte[], opts)`| `set(&self, key, value, opts)`  |
//| `RefreshAsync(string)`          | `refresh(&self, key)`           |
//| `RemoveAsync(string)`           | `remove(&self, key)`            |

use super::options::DistributedCacheEntryOptions;

#[derive(Debug, Clone)]
pub enum CacheError {
    NotFound(String),
    Serialization(String),
    Unavailable(String),
    Message(String),
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheError::NotFound(k) => write!(f, "cache key not found: {}", k),
            CacheError::Serialization(e) => write!(f, "cache serialization error: {}", e),
            CacheError::Unavailable(e) => write!(f, "cache unavailable: {}", e),
            CacheError::Message(m) => write!(f, "{}", m),
        }
    }
}

impl std::error::Error for CacheError {}

pub type Result<T> = std::result::Result<T, CacheError>;

#[async_trait::async_trait]
pub trait IDistributedCache: Send + Sync + 'static {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    async fn set(
        &self,
        key: &str,
        value: Vec<u8>,
        options: Option<&DistributedCacheEntryOptions>,
    ) -> Result<()>;
    async fn remove(&self, key: &str) -> Result<()>;
    async fn refresh(&self, key: &str) -> Result<()>;
    async fn exists(&self, key: &str) -> Result<bool>;
    async fn clear(&self) -> Result<()> {
        Ok(())
    }
}
