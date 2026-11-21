use std::sync::Arc;

use webx_host::health::{HealthStatus, IHealthCheck};

use crate::connection::NatsConnection;

/// Readiness probe for the NATS connection.
pub struct NatsHealthCheck {
    connection: Arc<NatsConnection>,
}

impl NatsHealthCheck {
    pub(crate) fn new(connection: Arc<NatsConnection>) -> Self {
        Self { connection }
    }
}

#[async_trait::async_trait]
impl IHealthCheck for NatsHealthCheck {
    fn name(&self) -> &str {
        "nats"
    }

    async fn check(&self) -> HealthStatus {
        match self.connection.ping().await {
            Ok(()) => HealthStatus::pass(),
            Err(error) => HealthStatus::fail(format!("nats: {error}")),
        }
    }
}
