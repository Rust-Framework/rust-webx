use std::time::Duration;

use async_nats::Client;
use tokio::sync::OnceCell;
use webx_core::error::{Error, Result};

/// Lazily connected NATS client.
///
/// `async-nats` reconnects in the background after the first successful
/// connection; a temporary outage before the first connect is retried by the
/// consumer worker, and surfaced as a 503 by `publish`.
pub struct NatsConnection {
    url: String,
    connect_timeout: Duration,
    client: OnceCell<Client>,
}

impl NatsConnection {
    pub fn new(url: impl Into<String>, connect_timeout: Duration) -> Self {
        Self {
            url: url.into(),
            connect_timeout,
            client: OnceCell::new(),
        }
    }

    /// The configured server URL.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Shared client, connected on first use.
    pub async fn client(&self) -> Result<Client> {
        self.client
            .get_or_try_init(|| async {
                async_nats::ConnectOptions::new()
                    .connection_timeout(self.connect_timeout)
                    .connect(self.url.clone())
                    .await
                    .map_err(|error| {
                        Error::status(
                            503,
                            format!(
                                "cannot connect to NATS '{}': {error}",
                                redact_url(&self.url)
                            ),
                        )
                    })
            })
            .await
            .cloned()
    }

    /// Round-trip probe used by the readiness check.
    pub async fn ping(&self) -> Result<()> {
        let client = self.client().await?;
        client
            .flush()
            .await
            .map_err(|error| Error::status(503, format!("NATS flush failed: {error}")))
    }
}

/// Hide the password in a NATS URL before it reaches logs/health details.
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
            redact_url("nats://alice:secret@nats:4222"),
            "nats://alice:***@nats:4222"
        );
        assert_eq!(redact_url("nats://nats:4222"), "nats://nats:4222");
    }
}
