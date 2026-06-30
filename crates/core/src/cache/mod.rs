//! Unified caching abstraction — `IDistributedCache` trait + `MemoryCache` implementation.
//!
//| ASP.NET Core                  | LRWF                                    |
//|-------------------------------|-----------------------------------------|
//| `IDistributedCache`           | `IDistributedCache` (trait_def)         |
//| `IMemoryCache`                | `MemoryCache` (in lrwf-http)            |
//| `DistributedCacheExtensions`  | `DistributedCacheExtensions` (cache_ext)|
//| `DistributedCacheEntryOptions`| `DistributedCacheEntryOptions` (options)|

pub mod cache_ext;
pub mod options;
pub mod trait_def;

pub use cache_ext::DistributedCacheExtensions;
pub use options::DistributedCacheEntryOptions;
pub use trait_def::{CacheError, IDistributedCache, Result as CacheResult};
