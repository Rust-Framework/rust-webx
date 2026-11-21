use std::sync::Arc;

use webx_host::health::{HealthStatus, IHealthCheck};

use crate::connection::RabbitConnection;

/// Readiness probe for the RabbitMQ connection.
pub struct RabbitMqHealthCheck {
    connection: Arc<RabbitConnection>,
}

impl RabbitMqHealthCheck {
    pub(crate) fn new(connection: Arc<RabbitConnection>) -> Self {
        Self { connection }
    }
}

#[async_trait::async_trait]
impl IHealthCheck for RabbitMqHealthCheck {
    fn name(&self) -> &str {
        "rabbitmq"
    }

    async fn check(&self) -> HealthStatus {
        match self.connection.ping().await {
            Ok(()) => HealthStatus::pass(),
            Err(error) => HealthStatus::fail(format!("rabbitmq: {error}")),
        }
    }
}
