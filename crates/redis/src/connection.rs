use redis::aio::ConnectionManager;
use redis::Client;
use tokio::sync::OnceCell;
use webx_core::cache::CacheError;
use webx_core::error::Error;

/// Lazily connected, auto-reconnecting Redis handle.
///
/// The URL is parsed once (synchronously, so a typo is reported on first use);
/// the multiplexed connection is created on the first cache/event/health
/// operation and then reconnects in the background. Registration itself never
/// blocks on a broker.
pub struct RedisConnection {
    url: String,
    connect_timeout: std::time::Duration,
    command_timeout: std::time::Duration,
    client: std::result::Result<Client, String>,
    manager: OnceCell<ConnectionManager>,
}

impl RedisConnection {
    /// A lazy connection to `url`; the first attempt is bounded by
    /// `connect_timeout` and every command by `command_timeout`.
    pub fn new(
        url: impl Into<String>,
        connect_timeout: std::time::Duration,
        command_timeout: std::time::Duration,
    ) -> Self {
        let url = url.into();
        let client = Client::open(url.as_str()).map_err(|error| error.to_string());
        Self {
            url,
            connect_timeout,
            command_timeout,
            client,
            manager: OnceCell::new(),
        }
    }

    /// The configured URL (credentials should not be logged).
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Multiplexed connection shared by every operation; connects on first use.
    pub async fn manager(&self) -> std::result::Result<ConnectionManager, CacheError> {
        self.manager
            .get_or_try_init(|| async {
                let client = self.client.as_ref().map_err(|error| {
                    CacheError::Unavailable(format!(
                        "invalid Redis URL '{}': {error}",
                        redact_url(&self.url)
                    ))
                })?;
                let config = redis::aio::ConnectionManagerConfig::new()
                    .set_connection_timeout(self.connect_timeout)
                    .set_response_timeout(self.command_timeout);
                let connecting = ConnectionManager::new_with_config(client.clone(), config);
                tokio::time::timeout(self.connect_timeout, connecting)
                    .await
                    .map_err(|_| {
                        CacheError::Unavailable(format!(
                            "connecting to Redis '{}' timed out after {:?}",
                            redact_url(&self.url),
                            self.connect_timeout
                        ))
                    })?
                    .map_err(|error| {
                        CacheError::Unavailable(format!(
                            "cannot connect to Redis '{}': {error}",
                            redact_url(&self.url)
                        ))
                    })
            })
            .await
            .cloned()
    }

    /// `PING` the server (health probe).
    pub async fn ping(&self) -> std::result::Result<(), CacheError> {
        let mut connection = self.manager().await?;
        redis::cmd("PING")
            .query_async::<String>(&mut connection)
            .await
            .map(|_| ())
            .map_err(|error| CacheError::Unavailable(format!("Redis PING failed: {error}")))
    }
}

/// Map a Redis command failure to a retryable framework error.
pub(crate) fn transport_error(context: &str, error: impl std::fmt::Display) -> Error {
    Error::Status(503, format!("{context}: {error}"))
}

/// Hide the password in a Redis URL before it reaches logs/health details.
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
            redact_url("redis://alice:secret@cache:6379/0"),
            "redis://alice:***@cache:6379/0"
        );
        assert_eq!(redact_url("redis://cache:6379/0"), "redis://cache:6379/0");
    }
}
