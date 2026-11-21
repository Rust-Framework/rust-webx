use std::sync::Arc;

use lapin::Connection;
use tokio::sync::Mutex;
use webx_core::error::{Error, Result};

/// Lazily connected RabbitMQ handle that replaces a closed connection.
///
/// The AMQP connection is opened on the first publish/consume/probe and
/// recreated after the broker closes it; registration never blocks on a
/// broker.
pub struct RabbitConnection {
    url: String,
    connect_timeout: std::time::Duration,
    connection: Mutex<Option<Arc<Connection>>>,
}

impl RabbitConnection {
    /// A lazy connection to `url`; the first attempt is bounded by `connect_timeout`.
    pub fn new(url: impl Into<String>, connect_timeout: std::time::Duration) -> Self {
        Self {
            url: url.into(),
            connect_timeout,
            connection: Mutex::new(None),
        }
    }

    /// The configured AMQP URL.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// A live connection, reconnecting when the cached one is closed.
    pub async fn connection(&self) -> Result<Arc<Connection>> {
        let mut slot = self.connection.lock().await;
        if let Some(connection) = slot.as_ref() {
            if connection.status().connected() {
                return Ok(Arc::clone(connection));
            }
        }

        *slot = None;
        let connecting = Connection::connect(&self.url, lapin::ConnectionProperties::default());
        let connection = tokio::time::timeout(self.connect_timeout, connecting)
            .await
            .map_err(|_| {
                Error::Status(
                    503,
                    format!(
                        "connecting to RabbitMQ '{}' timed out after {:?}",
                        redact_url(&self.url),
                        self.connect_timeout
                    ),
                )
            })?
            .map_err(|error| {
                Error::Status(
                    503,
                    format!(
                        "cannot connect to RabbitMQ '{}': {error}",
                        redact_url(&self.url)
                    ),
                )
            })?;
        let connection = Arc::new(connection);
        *slot = Some(Arc::clone(&connection));
        Ok(connection)
    }

    /// Readiness probe: the connection must be open.
    pub async fn ping(&self) -> Result<()> {
        let connection = self.connection().await?;
        if connection.status().connected() {
            Ok(())
        } else {
            Err(Error::Status(
                503,
                "RabbitMQ connection is closed".to_string(),
            ))
        }
    }
}

/// Hide the password in an AMQP URL before it reaches logs/health details.
fn redact_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let prefix = &url[..scheme_end + 3];
    let rest = &url[scheme_end + 3..];
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let redacted = match authority.rsplit_once('@') {
        Some((credentials, host)) => {
            let user = credentials.split(':').next().unwrap_or("");
            format!("{user}:***@{host}")
        }
        None => authority.to_string(),
    };
    format!("{prefix}{redacted}{}", &rest[authority_end..])
}

#[cfg(test)]
mod tests {
    use super::redact_url;

    #[test]
    fn credentials_are_redacted() {
        assert_eq!(
            redact_url("amqp://guest:secret@rabbit:5672/%2f"),
            "amqp://guest:***@rabbit:5672/%2f"
        );
    }
}
