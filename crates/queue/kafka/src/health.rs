use std::sync::Arc;

use webx_host::health::{HealthStatus, IHealthCheck};

use crate::connection::KafkaConnection;

/// Readiness probe for the Kafka metadata connection.
pub struct KafkaHealthCheck {
    connection: Arc<KafkaConnection>,
}

impl KafkaHealthCheck {
    pub(crate) fn new(connection: Arc<KafkaConnection>) -> Self {
        Self { connection }
    }
}

#[async_trait::async_trait]
impl IHealthCheck for KafkaHealthCheck {
    fn name(&self) -> &str {
        "kafka"
    }

    async fn check(&self) -> HealthStatus {
        match self.connection.ping().await {
            Ok(()) => HealthStatus::pass(),
            Err(error) => HealthStatus::fail(format!("kafka: {error}")),
        }
    }
}
