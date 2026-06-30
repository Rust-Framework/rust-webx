//! Distributed cache entry options — matches ASP.NET Core's `DistributedCacheEntryOptions`.

use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct DistributedCacheEntryOptions {
    pub absolute_expiration_relative_to_now: Option<Duration>,
    pub sliding_expiration: Option<Duration>,
    pub size_limit: usize,
}

impl DistributedCacheEntryOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_absolute_expiration_relative_to_now(mut self, d: Duration) -> Self {
        self.absolute_expiration_relative_to_now = Some(d);
        self
    }

    pub fn set_sliding_expiration(mut self, d: Duration) -> Self {
        self.sliding_expiration = Some(d);
        self
    }

    pub fn set_size_limit(mut self, bytes: usize) -> Self {
        self.size_limit = bytes;
        self
    }
}
