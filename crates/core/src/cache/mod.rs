//! Unified caching abstraction — `IDistributedCache` trait + `MemoryCache` implementation.
//!
//| ASP.NET Core                  | rust-webx                               |
//|-------------------------------|-----------------------------------------|
//| `IDistributedCache`           | `IDistributedCache` (trait_def)         |
//| `IMemoryCache`                | `MemoryCache` (in rust-webx-host)       |
//| `DistributedCacheExtensions`  | `DistributedCacheExtensions` (cache_ext)|
//| `DistributedCacheEntryOptions`| `DistributedCacheEntryOptions` (options)|

pub mod cache_ext;
pub mod options;
pub mod trait_def;

pub use cache_ext::DistributedCacheExtensions;
pub use options::DistributedCacheEntryOptions;
pub use trait_def::{CacheError, IDistributedCache, Result as CacheResult};
