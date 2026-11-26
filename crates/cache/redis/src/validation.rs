//! Startup validation for [`RedisOptions`].
//!
//! Configuration errors such as a blank stream/group, an unparseable start id
//! or an invalid tuning value otherwise surface only inside the background
//! consumer's reconnect loop. The host awaits hosted services before accepting
//! requests, so a typed error here fails startup once instead of hiding a
//! permanently misconfigured transport.

use async_trait::async_trait;

use webx_core::error::{Error, Result};
use webx_core::handler::IHostedService;

use crate::options::RedisOptions;

pub(crate) struct RedisOptionsValidation {
    options: RedisOptions,
}

impl RedisOptionsValidation {
    pub(crate) fn new(options: RedisOptions) -> Self {
        Self { options }
    }
}

#[async_trait]
impl IHostedService for RedisOptionsValidation {
    async fn start(&self) -> Result<()> {
        self.options
            .validate()
            .map_err(|error| Error::di(format!("invalid Redis configuration: {error}")))
    }

    /// Validation must run before any consumer/worker starts; stopping earlier
    /// than everyone else is harmless because it holds no resources.
    fn order(&self) -> i32 {
        -10_000
    }
}
