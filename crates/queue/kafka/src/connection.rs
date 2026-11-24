use std::sync::Arc;
use std::time::Duration;

use rskafka::client::{Client, ClientBuilder};
use tokio::sync::Mutex;
use webx_core::error::{Error, Result};

/// Lazily connected Kafka client.
///
/// `rskafka` keeps broker metadata and reconnects per request; a temporary
/// outage before the first connect is retried by the consumer and surfaced as a
/// 503 by `publish`.
pub struct KafkaConnection {
    brokers: Vec<String>,
    connect_timeout: Duration,
    client: Mutex<Option<Arc<Client>>>,
}

impl KafkaConnection {
    /// A lazy client for `brokers`; the first attempt is bounded by `connect_timeout`.
    pub fn new(brokers: Vec<String>, connect_timeout: Duration) -> Self {
        Self {
            brokers,
            connect_timeout,
            client: Mutex::new(None),
        }
    }

    /// The configured bootstrap brokers.
    pub fn brokers(&self) -> &[String] {
        &self.brokers
    }

    /// Shared client, connected on first use.
    pub async fn client(&self) -> Result<Arc<Client>> {
        let mut slot = self.client.lock().await;
        if let Some(client) = slot.as_ref() {
            return Ok(Arc::clone(client));
        }

        let builder = ClientBuilder::new(self.brokers.clone());
        let connecting = builder.build();
        let client = tokio::time::timeout(self.connect_timeout, connecting)
            .await
            .map_err(|_| {
                Error::status(
                    503,
                    format!(
                        "connecting to Kafka {:?} timed out after {:?}",
                        self.brokers, self.connect_timeout
                    ),
                )
            })?
            .map_err(|error| {
                Error::status(
                    503,
                    format!("cannot connect to Kafka {:?}: {error}", self.brokers),
                )
            })?;
        let client = Arc::new(client);
        *slot = Some(Arc::clone(&client));
        Ok(client)
    }

    /// Readiness probe: broker metadata must be reachable.
    pub async fn ping(&self) -> Result<()> {
        let client = self.client().await?;
        client
            .list_topics()
            .await
            .map(|_| ())
            .map_err(|error| Error::status(503, format!("Kafka metadata request failed: {error}")))
    }
}
