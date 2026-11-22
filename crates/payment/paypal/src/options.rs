//! PayPal adapter configuration (multi-account aware, secrets redacted).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use webx_client::HttpClientOptions;
use webx_pay::{AccountId, AccountSet, PaymentAccount, PaymentError, ProviderId};

use crate::auth::{PayPalClock, SystemClock, TokenCache};

/// PayPal live REST base URL.
pub const LIVE_BASE_URL: &str = "https://api-m.paypal.com";
/// PayPal sandbox REST base URL.
pub const SANDBOX_BASE_URL: &str = "https://api-m.sandbox.paypal.com";

fn default_account_id() -> String {
    AccountId::DEFAULT.to_string()
}

fn default_timeout_seconds() -> u64 {
    30
}

fn default_connect_timeout_seconds() -> u64 {
    5
}

/// PayPal environment selected for an account.
///
/// The same client id/secret pair exists in both environments, so the adapter
/// requires the environment to be stated instead of guessing it from the
/// credentials. Defaults to `sandbox` to fail safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PayPalEnvironment {
    /// Sandbox (`https://api-m.sandbox.paypal.com`).
    #[default]
    Sandbox,
    /// Live (`https://api-m.paypal.com`).
    Live,
}

impl PayPalEnvironment {
    /// Default REST base URL for the environment.
    pub fn base_url(&self) -> &'static str {
        match self {
            Self::Sandbox => SANDBOX_BASE_URL,
            Self::Live => LIVE_BASE_URL,
        }
    }

    /// Whether this environment moves real money.
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live)
    }
}

/// One PayPal merchant account.
#[derive(Clone, Default, Deserialize)]
pub struct PayPalAccountOptions {
    /// Account id used in requests and the webhook account path segment.
    #[serde(default = "default_account_id")]
    pub id: String,
    /// PayPal REST app client id.
    ///
    /// Public: it is surfaced to the frontend catalog as
    /// `client_config.client_id`.
    pub client_id: String,
    /// PayPal REST app client secret.
    ///
    /// Never exposed by the frontend catalog or `Debug`.
    pub client_secret: String,
    /// Webhook id from the PayPal app settings (Webhooks v1 verification).
    #[serde(default)]
    pub webhook_id: Option<String>,
    /// Sandbox or live.
    ///
    /// Surfaced to the frontend catalog as `client_config.environment`
    /// (`sandbox` / `live`); an `api_base` override does not change it.
    #[serde(default)]
    pub environment: PayPalEnvironment,
    /// Override the REST base URL (proxies/tests take precedence over
    /// `environment`).
    #[serde(default)]
    pub api_base: Option<String>,
    /// Override the account's response-header timeout in seconds.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

impl PayPalAccountOptions {
    /// Account using `client_id`/`client_secret` in the sandbox environment.
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self {
            id: default_account_id(),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            webhook_id: None,
            environment: PayPalEnvironment::Sandbox,
            api_base: None,
            timeout_seconds: None,
        }
    }

    /// Rename the account (multi-account mode).
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Configure the webhook id used to verify callback signatures.
    pub fn with_webhook_id(mut self, webhook_id: impl Into<String>) -> Self {
        self.webhook_id = Some(webhook_id.into());
        self
    }

    /// Select the sandbox or live environment.
    pub fn with_environment(mut self, environment: PayPalEnvironment) -> Self {
        self.environment = environment;
        self
    }

    /// Override the REST base URL for this account.
    pub fn with_api_base(mut self, api_base: impl Into<String>) -> Self {
        self.api_base = Some(api_base.into());
        self
    }

    /// Override this account's response-header timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout_seconds = Some(timeout.as_secs().max(1));
        self
    }

    /// Static validation of this account: id, credentials and webhook id shape.
    ///
    /// Performs no network call; the base URL is validated at adapter level
    /// because it depends on the adapter-wide private-network policy.
    pub fn validate(&self) -> Result<(), String> {
        self.checked_id().map(|_| ())
    }

    fn checked_id(&self) -> Result<AccountId, String> {
        let id = AccountId::new(self.id.clone())
            .map_err(|error| format!("paypal account id is invalid: {error}"))?;
        if self.client_id.trim().is_empty() {
            return Err(format!("paypal account `{id}` client_id must not be empty"));
        }
        if self.client_secret.trim().is_empty() {
            return Err(format!(
                "paypal account `{id}` client_secret must not be empty"
            ));
        }
        Ok(id)
    }
}

impl fmt::Debug for PayPalAccountOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayPalAccountOptions")
            .field("id", &self.id)
            .field("client_id", &"<redacted>")
            .field("client_secret", &"<redacted>")
            .field("webhook_id", &self.webhook_id.as_ref().map(|_| "<set>"))
            .field("environment", &self.environment)
            .field("api_base", &self.api_base)
            .field("timeout_seconds", &self.timeout_seconds)
            .finish()
    }
}

/// PayPal adapter options.
///
/// Bind from configuration with `add_options::<PayPalOptions>("PayPal")` or
/// build in code with [`PayPalOptions::new`]. The single-account constructor is
/// the common case; [`PayPalOptions::with_account`] adds more accounts (service
/// provider / multi-tenant mode).
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct PayPalOptions {
    /// Merchant accounts; at least one is required.
    pub accounts: Vec<PayPalAccountOptions>,
    /// Account used when a request does not select one (defaults to the first).
    pub default_account: Option<String>,
    /// Response-header timeout in seconds (per-account override available).
    pub timeout_seconds: u64,
    /// TCP/TLS connect timeout in seconds.
    pub connect_timeout_seconds: u64,
    /// Allow loopback/intranet endpoints (tests only; production stays `false`).
    pub allow_private_networks: bool,
    /// Global REST base URL override for every account (tests/proxies).
    pub api_base: Option<String>,
}

impl Default for PayPalOptions {
    fn default() -> Self {
        Self {
            accounts: Vec::new(),
            default_account: None,
            timeout_seconds: default_timeout_seconds(),
            connect_timeout_seconds: default_connect_timeout_seconds(),
            allow_private_networks: false,
            api_base: None,
        }
    }
}

impl PayPalOptions {
    /// Single sandbox account from a REST app client id/secret and webhook id.
    ///
    /// An empty `webhook_id` leaves callback verification unconfigured (orders
    /// and refunds still work; [`verify`](webx_pay::IPaymentWebhookVerifier::verify)
    /// reports a configuration error).
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        webhook_id: impl Into<String>,
    ) -> Self {
        let webhook_id = webhook_id.into();
        let account = PayPalAccountOptions::new(client_id, client_secret);
        let account = if webhook_id.trim().is_empty() {
            account
        } else {
            account.with_webhook_id(webhook_id)
        };
        Self {
            accounts: vec![account],
            ..Self::default()
        }
    }

    /// Options with no account (bind from config, then populate `accounts`).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Add a merchant account.
    pub fn with_account(mut self, account: PayPalAccountOptions) -> Self {
        self.accounts.push(account);
        self
    }

    /// Select the default account.
    pub fn with_default_account(mut self, id: impl Into<String>) -> Self {
        self.default_account = Some(id.into());
        self
    }

    /// Override the REST base URL for every account.
    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = Some(base.into());
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

    /// Base URL used for the shared HTTP client.
    ///
    /// Requests are signed against absolute per-account URLs; this only needs to
    /// be a valid absolute URI for client construction.
    pub(crate) fn client_base_url(&self) -> String {
        if let Some(base) = &self.api_base {
            return base.trim_end_matches('/').to_string();
        }
        if let Some(account) = self.accounts.first() {
            return account
                .api_base
                .clone()
                .unwrap_or_else(|| account.environment.base_url().to_string())
                .trim_end_matches('/')
                .to_string();
        }
        SANDBOX_BASE_URL.to_string()
    }

    pub(crate) fn http_options(&self, base_url: String) -> HttpClientOptions {
        let timeout = Duration::from_secs(self.timeout_seconds.max(1));
        HttpClientOptions {
            base_url,
            connect_timeout: Duration::from_secs(self.connect_timeout_seconds.max(1)),
            request_timeout: Some(timeout),
            read_timeout: Some(timeout),
            allow_private_networks: self.allow_private_networks,
            user_agent: format!("rust-webx-pay-paypal/{}", env!("CARGO_PKG_VERSION")),
            ..HttpClientOptions::default()
        }
    }

    /// Resolve configured accounts into credentials and validate fields that do
    /// not need a network call.
    pub(crate) fn build_accounts(
        &self,
        clock: Arc<dyn PayPalClock>,
    ) -> Result<AccountSet<PayPalAccount>, String> {
        if self.accounts.is_empty() {
            return Err("paypal requires at least one account".to_string());
        }
        let mut converted = Vec::new();
        for raw in &self.accounts {
            let id = raw.checked_id()?;
            let base_url = raw
                .api_base
                .clone()
                .or_else(|| self.api_base.clone())
                .unwrap_or_else(|| raw.environment.base_url().to_string());
            let base_url = base_url.trim_end_matches('/').to_string();
            validate_base_url(&base_url, id.as_str(), self.allow_private_networks)?;
            let timeout_seconds = raw.timeout_seconds.unwrap_or(self.timeout_seconds).max(1);
            let account = PayPalAccount {
                id: id.clone(),
                client_id: raw.client_id.clone(),
                client_secret: raw.client_secret.clone(),
                webhook_id: raw
                    .webhook_id
                    .clone()
                    .filter(|webhook_id| !webhook_id.trim().is_empty()),
                live: raw.environment.is_live(),
                base_url,
                timeout: Duration::from_secs(timeout_seconds),
                tokens: TokenCache::with_clock(Arc::clone(&clock)),
            };
            let info = PaymentAccount {
                id: id.clone(),
                provider: ProviderId::PayPal,
                live: account.live,
                label: None,
            };
            converted.push((id, account, info));
        }
        let default = self
            .default_account
            .clone()
            .map(AccountId::from)
            .unwrap_or_else(|| converted[0].0.clone());
        AccountSet::new(ProviderId::PayPal, default, converted).map_err(|error| error.to_string())
    }

    /// Static configuration validation: accounts, credentials and base URLs.
    ///
    /// Performs no network call; host startup and the adapter health check run
    /// this before the provider serves traffic.
    pub fn validate(&self) -> Result<(), PaymentError> {
        self.build_accounts(Arc::new(SystemClock))
            .map(|_| ())
            .map_err(|error| PaymentError::config(ProviderId::PayPal, error))
    }
}

impl fmt::Debug for PayPalOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayPalOptions")
            .field("accounts", &self.accounts)
            .field("default_account", &self.default_account)
            .field("timeout_seconds", &self.timeout_seconds)
            .field("connect_timeout_seconds", &self.connect_timeout_seconds)
            .field("allow_private_networks", &self.allow_private_networks)
            .field("api_base", &self.api_base)
            .finish()
    }
}

fn validate_base_url(url: &str, account: &str, allow_private_networks: bool) -> Result<(), String> {
    if url.is_empty() {
        return Err(format!(
            "paypal account `{account}` api_base must not be empty"
        ));
    }
    if url.starts_with("https://") || (allow_private_networks && url.starts_with("http://")) {
        return Ok(());
    }
    Err(format!(
        "paypal account `{account}` api_base must be an https URL (http is allowed only with allow_private_networks)"
    ))
}

/// Resolved credentials and transport settings for one merchant account.
#[derive(Clone)]
pub(crate) struct PayPalAccount {
    pub(crate) id: AccountId,
    pub(crate) client_id: String,
    pub(crate) client_secret: String,
    pub(crate) webhook_id: Option<String>,
    pub(crate) live: bool,
    pub(crate) base_url: String,
    pub(crate) timeout: Duration,
    pub(crate) tokens: TokenCache,
}

impl PayPalAccount {
    /// Absolute URL for `path` on this account's REST base.
    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }
}

impl fmt::Debug for PayPalAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayPalAccount")
            .field("id", &self.id)
            .field("client_id", &"<redacted>")
            .field("client_secret", &"<redacted>")
            .field("webhook_id", &self.webhook_id.as_ref().map(|_| "<set>"))
            .field("live", &self.live)
            .field("base_url", &self.base_url)
            .field("timeout", &self.timeout)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::SystemClock;

    fn sample_options() -> PayPalOptions {
        PayPalOptions::new("test-client", "test-secret", "test-webhook")
    }

    fn build(options: &PayPalOptions) -> AccountSet<PayPalAccount> {
        options
            .build_accounts(Arc::new(SystemClock))
            .expect("options should build")
    }

    #[test]
    fn single_account_defaults_to_sandbox() {
        let accounts = build(&sample_options());
        assert_eq!(accounts.default_id().as_str(), "default");
        assert_eq!(accounts.accounts().len(), 1);
        let public = &accounts.accounts()[0];
        assert!(!public.live);
        assert_eq!(public.provider, ProviderId::PayPal);
        let account = accounts.resolve(None).unwrap();
        assert_eq!(account.base_url, SANDBOX_BASE_URL);
        assert!(account.webhook_id.is_some());
        assert_eq!(account.timeout, Duration::from_secs(30));
    }

    #[test]
    fn live_environment_uses_the_live_base_url() {
        let mut options = sample_options();
        options.accounts[0].environment = PayPalEnvironment::Live;
        let accounts = build(&options);
        let account = accounts.resolve(None).unwrap();
        assert!(account.live);
        assert!(accounts.accounts()[0].live);
        assert_eq!(account.base_url, LIVE_BASE_URL);
    }

    #[test]
    fn api_base_override_wins_over_the_environment() {
        let options = sample_options().with_api_base("https://paypal-proxy.example.com");
        let accounts = build(&options);
        assert_eq!(
            accounts.resolve(None).unwrap().base_url,
            "https://paypal-proxy.example.com"
        );

        let mut options = sample_options();
        options.accounts[0].api_base = Some("https://sandbox-proxy.example.com/".to_string());
        let accounts = build(&options);
        assert_eq!(
            accounts.resolve(None).unwrap().base_url,
            "https://sandbox-proxy.example.com"
        );
    }

    #[test]
    fn rejects_missing_accounts_and_credentials() {
        assert!(PayPalOptions::empty()
            .build_accounts(Arc::new(SystemClock))
            .is_err());

        let mut options = sample_options();
        options.accounts[0].client_id = "  ".to_string();
        assert!(options.build_accounts(Arc::new(SystemClock)).is_err());

        let mut options = sample_options();
        options.accounts[0].client_secret = String::new();
        assert!(options.build_accounts(Arc::new(SystemClock)).is_err());
    }

    #[test]
    fn rejects_plaintext_http_base_unless_private_networks_are_allowed() {
        let options = sample_options().with_api_base("http://127.0.0.1:8080");
        assert!(options.build_accounts(Arc::new(SystemClock)).is_err());
        let options = options.allow_private_networks(true);
        assert_eq!(
            options
                .build_accounts(Arc::new(SystemClock))
                .unwrap()
                .resolve(None)
                .unwrap()
                .base_url,
            "http://127.0.0.1:8080"
        );
    }

    #[test]
    fn multi_account_default_and_duplicates() {
        let options = sample_options()
            .with_account(PayPalAccountOptions::new("client-2", "secret-2").with_id("second"))
            .with_default_account("second");
        let accounts = build(&options);
        assert_eq!(accounts.accounts().len(), 2);
        assert_eq!(accounts.resolve(None).unwrap().id.as_str(), "second");
        assert_eq!(
            accounts
                .resolve(Some(&AccountId::from("default")))
                .unwrap()
                .id
                .as_str(),
            "default"
        );

        let duplicates =
            sample_options().with_account(PayPalAccountOptions::new("client-3", "secret-3"));
        assert!(duplicates.build_accounts(Arc::new(SystemClock)).is_err());
    }

    #[test]
    fn per_account_timeout_overrides_the_global_one() {
        let mut options = sample_options();
        options.accounts[0].timeout_seconds = Some(7);
        let accounts = build(&options);
        assert_eq!(
            accounts.resolve(None).unwrap().timeout,
            Duration::from_secs(7)
        );
    }

    #[test]
    fn debug_never_prints_secrets() {
        let options = sample_options();
        let text = format!("{options:?}");
        assert!(!text.contains("test-secret"));
        assert!(!text.contains("test-client"));
        assert!(text.contains("<redacted>"));
        assert!(format!("{:?}", options.accounts[0]).contains("<redacted>"));

        let accounts = build(&options);
        let account = accounts.resolve(None).unwrap();
        let text = format!("{account:?}");
        assert!(!text.contains("test-secret"));
        assert!(text.contains("<redacted>"));
    }

    #[test]
    fn binding_from_config_section() {
        let options: PayPalOptions = serde_json::from_value(serde_json::json!({
            "default_account": "eu",
            "timeout_seconds": 12,
            "accounts": [{
                "id": "eu",
                "client_id": "config-client",
                "client_secret": "config-secret",
                "webhook_id": "config-webhook",
                "environment": "live"
            }]
        }))
        .unwrap();
        let accounts = build(&options);
        let account = accounts.resolve(None).unwrap();
        assert!(account.live);
        assert_eq!(account.base_url, LIVE_BASE_URL);
        assert_eq!(account.timeout, Duration::from_secs(12));
    }

    #[test]
    fn empty_webhook_id_is_treated_as_unconfigured() {
        let options = PayPalOptions::new("test-client", "test-secret", "");
        let accounts = build(&options);
        assert!(accounts.resolve(None).unwrap().webhook_id.is_none());
    }

    #[test]
    fn validate_is_network_free_and_reports_missing_fields() {
        assert!(sample_options().validate().is_ok());
        let error = PayPalOptions::empty().validate().unwrap_err();
        assert!(matches!(error, PaymentError::Config { .. }));

        assert!(PayPalAccountOptions::default().validate().is_err());
        assert!(PayPalAccountOptions::new("test-client", "test-secret")
            .validate()
            .is_ok());
        assert!(PayPalAccountOptions::new("test-client", "test-secret")
            .with_id("../escape")
            .validate()
            .is_err());
    }
}
