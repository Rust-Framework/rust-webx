use std::sync::Arc;

use webx_host::health::{HealthStatus, IHealthCheck};

use crate::connection::RedisConnection;

/// Readiness probe for the Redis connection.
///
/// Registered by [`RedisHostBuilderExt::add_redis`](crate::RedisHostBuilderExt::add_redis);
/// a failed `PING` reports `fail`, so `/health/ready` returns 503.
pub struct RedisHealthCheck {
    connection: Arc<RedisConnection>,
}

impl RedisHealthCheck {
    pub(crate) fn new(connection: Arc<RedisConnection>) -> Self {
        Self { connection }
    }
}

#[async_trait::async_trait]
impl IHealthCheck for RedisHealthCheck {
    fn name(&self) -> &str {
        "redis"
    }

    async fn check(&self) -> HealthStatus {
        match self.connection.ping().await {
            Ok(()) => HealthStatus::pass(),
            Err(error) => HealthStatus::fail(format!("redis: {error}")),
        }
    }
}
