//! Stripe adapter configuration (multi-account aware).
//!
//! Secret credentials are redacted from [`Debug`](fmt::Debug); the publishable
//! key is public client configuration and is surfaced through the frontend
//! payment catalog.

use std::fmt;
use std::time::Duration;

use serde::Deserialize;
use webx_client::HttpClientOptions;
use webx_pay::ProviderId;

use crate::provider::StripeAccount;

fn default_api_base() -> String {
    "https://api.stripe.com".to_string()
}

fn default_timeout() -> u64 {
    30
}

fn default_connect_timeout() -> u64 {
    5
}

/// One Stripe merchant account.
#[derive(Clone, Deserialize)]
pub struct StripeAccountOptions {
    /// Account id used in requests and the webhook account path segment.
    #[serde(default = "default_account_id")]
    pub id: String,
    /// Secret API key (`sk_live_...`, `sk_test_...`, restricted `rk_...`).
    pub secret_key: String,
    /// Publishable key (`pk_live_...`, `pk_test_...`) handed to the frontend SDK.
    ///
    /// This is **not a secret**: it is exposed through the frontend payment
    /// catalog (`webx_pay::IPaymentProvider::payment_options`). Blank values are
    /// treated as absent.
    #[serde(default)]
    pub publishable_key: Option<String>,
    /// Endpoint signing secret (`whsec_...`); required only for webhooks.
    #[serde(default)]
    pub webhook_secret: Option<String>,
    /// Force live/test classification instead of deriving it from the key.
    #[serde(default)]
    pub live: Option<bool>,
    /// Pin the Stripe API version for this account.
    #[serde(default)]
    pub api_version: Option<String>,
}

fn default_account_id() -> String {
    "default".to_string()
}

impl StripeAccountOptions {
    /// Account using `secret_key` (and optional webhook secret).
    pub fn new(secret_key: impl Into<String>, webhook_secret: Option<String>) -> Self {
        Self {
            id: default_account_id(),
            secret_key: secret_key.into(),
            publishable_key: None,
            webhook_secret,
            live: None,
            api_version: None,
        }
    }

    /// Rename the account (multi-account mode).
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Set the publishable key exposed to the frontend SDK.
    pub fn with_publishable_key(mut self, publishable_key: impl Into<String>) -> Self {
        self.publishable_key = Some(publishable_key.into());
        self
    }

    /// Pin the Stripe API version.
    pub fn with_api_version(mut self, version: impl Into<String>) -> Self {
        self.api_version = Some(version.into());
        self
    }

    pub(crate) fn into_account(self) -> Result<StripeAccount, String> {
        let id = webx_pay::AccountId::new(self.id.clone())?;
        if self.secret_key.trim().is_empty() {
            return Err(format!(
                "stripe account `{id}` secret_key must not be empty"
            ));
        }
        let live = self.live.unwrap_or_else(|| {
            self.secret_key.starts_with("sk_live_") || self.secret_key.starts_with("rk_live_")
        });
        Ok(StripeAccount {
            id,
            secret_key: self.secret_key,
            publishable_key: self.publishable_key.filter(|key| !key.trim().is_empty()),
            webhook_secret: self
                .webhook_secret
                .filter(|secret| !secret.trim().is_empty()),
            live,
            api_version: self.api_version,
        })
    }
}

impl fmt::Debug for StripeAccountOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StripeAccountOptions")
            .field("id", &self.id)
            .field("secret_key", &"<redacted>")
            .field("publishable_key", &self.publishable_key)
            .field(
                "webhook_secret",
                &self.webhook_secret.as_ref().map(|_| "<redacted>"),
            )
            .field("live", &self.live)
            .field("api_version", &self.api_version)
            .finish()
    }
}

/// Stripe adapter options.
///
/// Bind from configuration with `add_options::<StripeOptions>("Stripe")` or
/// build in code with [`StripeOptions::new`]. The single-account constructor is
/// the common case; [`StripeOptions::with_account`] adds more accounts (service
/// provider / multi-tenant mode).
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct StripeOptions {
    /// Merchant accounts; at least one is required.
    pub accounts: Vec<StripeAccountOptions>,
    /// Account used when a request does not select one (defaults to the first).
    #[serde(default)]
    pub default_account: Option<String>,
    /// API base URL (override for tests/proxies).
    pub api_base: String,
    /// Response-header timeout in seconds.
    pub timeout_seconds: u64,
    /// TCP/TLS connect timeout in seconds.
    pub connect_timeout_seconds: u64,
    /// Allow loopback/intranet endpoints (tests only; production stays `false`).
    pub allow_private_networks: bool,
}

impl Default for StripeOptions {
    fn default() -> Self {
        Self {
            accounts: Vec::new(),
            default_account: None,
            api_base: default_api_base(),
            timeout_seconds: default_timeout(),
            connect_timeout_seconds: default_connect_timeout(),
            allow_private_networks: false,
        }
    }
}

impl StripeOptions {
    /// Single-account options from a secret key and optional endpoint secret.
    pub fn new(secret_key: impl Into<String>, webhook_secret: impl Into<String>) -> Self {
        let webhook_secret = webhook_secret.into();
        Self {
            accounts: vec![StripeAccountOptions::new(
                secret_key,
                (!webhook_secret.is_empty()).then_some(webhook_secret),
            )],
            ..Self::default()
        }
    }

    /// Options with no account (bind from config, then populate `accounts`).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Add a merchant account.
    pub fn with_account(
        mut self,
        id: impl Into<String>,
        secret_key: impl Into<String>,
        webhook_secret: Option<String>,
    ) -> Self {
        self.accounts
            .push(StripeAccountOptions::new(secret_key, webhook_secret).with_id(id));
        self
    }

    /// Select the default account.
    pub fn with_default_account(mut self, id: impl Into<String>) -> Self {
        self.default_account = Some(id.into());
        self
    }

    /// Override the Stripe API base URL.
    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = base.into();
        self
    }

    /// Response-header timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout_seconds = timeout.as_secs().max(1);
        self
    }

    /// Allow loopback/intranet destinations (tests only).
    pub fn allow_private_networks(mut self, allow: bool) -> Self {
        self.allow_private_networks = allow;
        self
    }

    pub(crate) fn http_options(&self) -> HttpClientOptions {
        HttpClientOptions {
            base_url: self.api_base.clone(),
            connect_timeout: Duration::from_secs(self.connect_timeout_seconds.max(1)),
            request_timeout: Some(Duration::from_secs(self.timeout_seconds.max(1))),
            read_timeout: Some(Duration::from_secs(self.timeout_seconds.max(1))),
            allow_private_networks: self.allow_private_networks,
            user_agent: format!("rust-webx-pay-stripe/{}", env!("CARGO_PKG_VERSION")),
            ..HttpClientOptions::default()
        }
    }

    /// Parse the configured accounts into credentials and validate fields that
    /// do not need a network call.
    pub(crate) fn build_accounts(&self) -> Result<webx_pay::AccountSet<StripeAccount>, String> {
        if self.accounts.is_empty() {
            return Err("stripe requires at least one account".to_string());
        }
        if !(self.api_base.starts_with("https://")
            || (self.allow_private_networks && self.api_base.starts_with("http://")))
        {
            return Err(
                "stripe api_base must be an https URL (http is allowed only with allow_private_networks)"
                    .to_string(),
            );
        }
        let mut converted = Vec::new();
        for account in &self.accounts {
            let credentials = account.clone().into_account()?;
            let info = webx_pay::PaymentAccount {
                id: credentials.id.clone(),
                provider: ProviderId::Stripe,
                live: credentials.live,
                label: None,
            };
            converted.push((credentials.id.clone(), credentials, info));
        }
        let default = self
            .default_account
            .clone()
            .map(webx_pay::AccountId::from)
            .unwrap_or_else(|| converted[0].0.clone());
        webx_pay::AccountSet::new(ProviderId::Stripe, default, converted)
            .map_err(|error| error.to_string())
    }
}

impl fmt::Debug for StripeOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StripeOptions")
            .field("accounts", &self.accounts)
            .field("default_account", &self.default_account)
            .field("api_base", &self.api_base)
            .field("timeout_seconds", &self.timeout_seconds)
            .field("connect_timeout_seconds", &self.connect_timeout_seconds)
            .field("allow_private_networks", &self.allow_private_networks)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_account_defaults_to_the_default_id_and_detects_live_keys() {
        let options = StripeOptions::new("sk_live_abc", "whsec_1");
        let accounts = options.build_accounts().unwrap();
        assert_eq!(accounts.default_id().as_str(), "default");
        assert_eq!(accounts.accounts().len(), 1);
        assert!(accounts.accounts()[0].live);
    }

    #[test]
    fn live_test_classification_is_overridable() {
        let mut options = StripeOptions::new("sk_test_abc", "");
        options.accounts[0].live = Some(true);
        let accounts = options.build_accounts().unwrap();
        assert!(accounts.accounts()[0].live);
    }

    #[test]
    fn debug_never_prints_secrets() {
        let mut options = StripeOptions::new("sk_test_super_secret", "whsec_super_secret");
        options.accounts[0].publishable_key = Some("pk_test_public".to_string());
        let text = format!("{options:?}");
        assert!(!text.contains("super_secret"));
        assert!(text.contains("<redacted>"));
        assert!(text.contains("pk_test_public"));
    }

    #[test]
    fn publishable_key_copies_into_accounts_and_blank_is_treated_as_absent() {
        let mut options = StripeOptions::new("sk_test_abc", "");
        options.accounts[0] = options.accounts[0].clone().with_publishable_key("   ");
        let accounts = options.build_accounts().unwrap();
        assert!(accounts.iter().next().unwrap().1.publishable_key.is_none());

        options.accounts[0] = options.accounts[0]
            .clone()
            .with_publishable_key("pk_test_123");
        let accounts = options.build_accounts().unwrap();
        assert_eq!(
            accounts.iter().next().unwrap().1.publishable_key.as_deref(),
            Some("pk_test_123")
        );
    }

    #[test]
    fn missing_or_plaintext_http_base_is_a_configuration_error() {
        assert!(StripeOptions::empty().build_accounts().is_err());
        let options = StripeOptions::new("sk_test_a", "").with_api_base("http://api.example.com");
        assert!(options.build_accounts().is_err());
        let options = options.allow_private_networks(true);
        assert!(options.build_accounts().is_ok());
    }
}
