//! APNs connection configuration.

use std::fmt;

use webx_core::error::{Error, Result};

/// APNs environment; selects the HTTP/2 host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApnsEnvironment {
    /// Production: `https://api.push.apple.com` (App Store builds).
    Production,
    /// Development sandbox: `https://api.sandbox.push.apple.com`.
    Sandbox,
}

impl ApnsEnvironment {
    /// Base URL of this environment.
    ///
    /// Build the injected [`IHttpClient`](webx_client::IHttpClient) with
    /// `HttpClientOptions { http2_only: true, ..HttpClientOptions::new(env.base_url()) }`.
    pub fn base_url(self) -> &'static str {
        match self {
            Self::Production => "https://api.push.apple.com",
            Self::Sandbox => "https://api.sandbox.push.apple.com",
        }
    }
}

/// APNs token-auth configuration: team id, key id, `.p8` signing key, topic.
///
/// Read the key from the environment or a secret store; do not keep it in
/// `appsettings.json` or source control. [`ApnsConfig`]'s `Debug` output
/// redacts the PEM.
pub struct ApnsConfig {
    /// Apple Developer team identifier (`iss` of the provider token).
    pub team_id: String,
    /// Key identifier of the `.p8` signing key (`kid` of the provider token).
    pub key_id: String,
    /// PEM contents of the `AuthKey_<key_id>.p8` file (PKCS#8, ES256).
    pub private_key_pem: String,
    /// App topic, normally the bundle id (`apns-topic` header).
    pub topic: String,
    /// Which APNs host to talk to.
    pub environment: ApnsEnvironment,
}

impl ApnsConfig {
    /// Build a configuration from its five fields.
    pub fn new(
        team_id: impl Into<String>,
        key_id: impl Into<String>,
        private_key_pem: impl Into<String>,
        topic: impl Into<String>,
        environment: ApnsEnvironment,
    ) -> Self {
        Self {
            team_id: team_id.into(),
            key_id: key_id.into(),
            private_key_pem: private_key_pem.into(),
            topic: topic.into(),
            environment,
        }
    }

    /// Reject an incomplete configuration before any request is attempted.
    ///
    /// The messages are fixed so nothing derived from the key can leak.
    pub(crate) fn validate(&self) -> Result<()> {
        let incomplete = self.team_id.trim().is_empty()
            || self.key_id.trim().is_empty()
            || self.private_key_pem.trim().is_empty()
            || self.topic.trim().is_empty();
        if incomplete {
            return Err(Error::internal(
                "APNs configuration is incomplete: team_id, key_id, topic and the .p8 signing key are required",
            ));
        }
        Ok(())
    }
}

impl fmt::Debug for ApnsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApnsConfig")
            .field("team_id", &self.team_id)
            .field("key_id", &self.key_id)
            .field("private_key_pem", &"<redacted>")
            .field("topic", &self.topic)
            .field("environment", &self.environment)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_never_contains_the_signing_key() {
        let config = ApnsConfig::new(
            "TEAM",
            "KEY",
            "-----BEGIN PRIVATE KEY-----\nsecret\n-----END PRIVATE KEY-----\n",
            "com.example.app",
            ApnsEnvironment::Sandbox,
        );
        let debug = format!("{config:?}");
        assert!(!debug.contains("BEGIN PRIVATE KEY"));
        assert!(!debug.contains("secret"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn the_environment_maps_to_the_apns_hosts() {
        assert_eq!(
            ApnsEnvironment::Production.base_url(),
            "https://api.push.apple.com"
        );
        assert_eq!(
            ApnsEnvironment::Sandbox.base_url(),
            "https://api.sandbox.push.apple.com"
        );
    }

    #[test]
    fn an_incomplete_configuration_is_rejected() {
        let error = ApnsConfig::new("", "KEY", "key", "topic", ApnsEnvironment::Production)
            .validate()
            .expect_err("missing team id must fail");
        assert_eq!(error.status_code(), 500);
        assert!(error.public_detail().is_none());
    }
}
