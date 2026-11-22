//! Square adapter configuration (multi-account aware, secrets redacted).

use std::fmt;
use std::time::Duration;

use serde::Deserialize;
use webx_client::HttpClientOptions;
use webx_pay::ProviderId;

use crate::provider::SquareAccount;

/// Square API version pinned in the `Square-Version` header.
///
/// Square versions its REST API by date. Pinning keeps request/response shapes
/// stable until the adapter is deliberately upgraded.
pub const DEFAULT_API_VERSION: &str = "2025-09-24";

/// Square production REST endpoint.
pub const LIVE_API_BASE: &str = "https://connect.squareup.com";

/// Square sandbox REST endpoint.
pub const SANDBOX_API_BASE: &str = "https://connect.squareupsandbox.com";

fn default_account_id() -> String {
    "default".to_string()
}

fn default_api_version() -> String {
    DEFAULT_API_VERSION.to_string()
}

fn default_timeout() -> u64 {
    30
}

fn default_connect_timeout() -> u64 {
    5
}

/// Which Square environment an account talks to.
///
/// The environment selects the REST endpoint and is reported as
/// [`PaymentAccount::live`](webx_pay::PaymentAccount) (never inferred from the
/// access-token shape: Square tokens are opaque).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SquareEnvironment {
    /// Live merchant data (`https://connect.squareup.com`).
    #[default]
    Production,
    /// Sandbox merchant data (`https://connect.squareupsandbox.com`).
    Sandbox,
}

impl SquareEnvironment {
    /// Default REST base URL for this environment.
    pub fn base_url(&self) -> &'static str {
        match self {
            Self::Production => LIVE_API_BASE,
            Self::Sandbox => SANDBOX_API_BASE,
        }
    }

    /// Whether the environment handles real money.
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Production)
    }
}

/// One Square merchant account.
#[derive(Clone, Deserialize)]
pub struct SquareAccountOptions {
    /// Account id used in requests and the webhook account path segment.
    #[serde(default = "default_account_id")]
    pub id: String,
    /// Square access token (`Authorization: Bearer ...`).
    pub access_token: String,
    /// Webhook signature key; required only for webhook verification.
    #[serde(default)]
    pub signature_key: Option<String>,
    /// Square location id stamped on payments and payment links.
    #[serde(default)]
    pub location_id: Option<String>,
    /// Square application id used by the client-side Web Payments SDK.
    ///
    /// This is a **public** browser credential (not a secret): the frontend
    /// payment catalog exposes it so the SDK can be initialized.
    #[serde(default)]
    pub application_id: Option<String>,
    /// API version override (`Square-Version` header).
    #[serde(default = "default_api_version")]
    pub api_version: String,
    /// Live or sandbox endpoint.
    #[serde(default)]
    pub environment: SquareEnvironment,
    /// Explicit REST base URL override (proxies/tests); defaults per environment.
    #[serde(default)]
    pub api_base: Option<String>,
}

impl SquareAccountOptions {
    /// Account with `access_token` and the defaults (production, pinned version).
    pub fn new(access_token: impl Into<String>) -> Self {
        Self {
            id: default_account_id(),
            access_token: access_token.into(),
            signature_key: None,
            location_id: None,
            application_id: None,
            api_version: default_api_version(),
            environment: SquareEnvironment::default(),
            api_base: None,
        }
    }

    /// Rename the account (multi-account mode).
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Set the webhook signature key.
    pub fn with_signature_key(mut self, signature_key: impl Into<String>) -> Self {
        self.signature_key = Some(signature_key.into());
        self
    }

    /// Set the default location id.
    pub fn with_location_id(mut self, location_id: impl Into<String>) -> Self {
        self.location_id = Some(location_id.into());
        self
    }

    /// Set the public application id used by the Web Payments SDK.
    pub fn with_application_id(mut self, application_id: impl Into<String>) -> Self {
        self.application_id = Some(application_id.into());
        self
    }

    /// Pin a different Square API version.
    pub fn with_api_version(mut self, api_version: impl Into<String>) -> Self {
        self.api_version = api_version.into();
        self
    }

    /// Select the Square environment.
    pub fn with_environment(mut self, environment: SquareEnvironment) -> Self {
        self.environment = environment;
        self
    }

    /// Override the REST base URL (proxies/tests).
    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = Some(base.into());
        self
    }

    pub(crate) fn into_account(
        self,
        allow_private_networks: bool,
    ) -> Result<SquareAccount, String> {
        let id = webx_pay::AccountId::new(self.id.trim())?;
        if self.access_token.trim().is_empty() {
            return Err(format!(
                "square account `{id}` access_token must not be empty"
            ));
        }
        let signature_key = self
            .signature_key
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty());
        let location_id = self
            .location_id
            .map(|location| location.trim().to_string())
            .filter(|location| !location.is_empty());
        let application_id = self
            .application_id
            .map(|application_id| application_id.trim().to_string())
            .filter(|application_id| !application_id.is_empty());
        let api_version = match self.api_version.trim() {
            "" => DEFAULT_API_VERSION.to_string(),
            version => version.to_string(),
        };
        let base_url = match self
            .api_base
            .map(|base| base.trim().trim_end_matches('/').to_string())
            .filter(|base| !base.is_empty())
        {
            Some(base) => base,
            None => self.environment.base_url().to_string(),
        };
        if !(base_url.starts_with("https://")
            || (allow_private_networks && base_url.starts_with("http://")))
        {
            return Err(format!(
                "square account `{id}` api_base must be an https URL (http is allowed only with allow_private_networks)"
            ));
        }
        Ok(SquareAccount {
            id,
            access_token: self.access_token.trim().to_string(),
            signature_key,
            location_id,
            application_id,
            live: self.environment.is_live(),
            api_version,
            base_url,
        })
    }
}

impl fmt::Debug for SquareAccountOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SquareAccountOptions")
            .field("id", &self.id)
            .field("access_token", &"<redacted>")
            .field(
                "signature_key",
                &self.signature_key.as_ref().map(|_| "<redacted>"),
            )
            .field("location_id", &self.location_id)
            .field("application_id", &self.application_id)
            .field("api_version", &self.api_version)
            .field("environment", &self.environment)
            .field("api_base", &self.api_base)
            .finish()
    }
}

/// Square adapter options.
///
/// `HostBuilder::add_square()` binds the `"Square"` configuration section
/// through `add_options_monitor`; a prebuilt configuration is passed to
/// `HostBuilder::add_square_with(options)`. Settings can also be built in code
/// with [`SquareOptions::new`] / [`SquareOptions::with_account`].
///
/// `webhook_url` is the exact notification URL configured in the Square
/// developer dashboard: Square signs `notification_url + raw_body`, so a
/// trailing slash or scheme mismatch makes every signature fail.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct SquareOptions {
    /// Merchant accounts; at least one is required.
    pub accounts: Vec<SquareAccountOptions>,
    /// Account used when a request does not select one (defaults to the first).
    pub default_account: Option<String>,
    /// Notification URL registered with Square (webhook HMAC input).
    pub webhook_url: Option<String>,
    /// Response-header timeout in seconds.
    pub timeout_seconds: u64,
    /// TCP/TLS connect timeout in seconds.
    pub connect_timeout_seconds: u64,
    /// Allow loopback/intranet endpoints (tests only; production stays `false`).
    pub allow_private_networks: bool,
}

impl Default for SquareOptions {
    fn default() -> Self {
        Self {
            accounts: Vec::new(),
            default_account: None,
            webhook_url: None,
            timeout_seconds: default_timeout(),
            connect_timeout_seconds: default_connect_timeout(),
            allow_private_networks: false,
        }
    }
}

impl SquareOptions {
    /// Single-account options for `access_token` (production environment).
    pub fn new(access_token: impl Into<String>) -> Self {
        Self {
            accounts: vec![SquareAccountOptions::new(access_token)],
            ..Self::default()
        }
    }

    /// Options with no account (bind from configuration, then populate).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Add a fully configured merchant account.
    pub fn with_account(mut self, account: SquareAccountOptions) -> Self {
        self.accounts.push(account);
        self
    }

    /// Select the default account.
    pub fn with_default_account(mut self, id: impl Into<String>) -> Self {
        self.default_account = Some(id.into());
        self
    }

    /// Set the notification URL registered with Square.
    pub fn with_webhook_url(mut self, webhook_url: impl Into<String>) -> Self {
        self.webhook_url = Some(webhook_url.into());
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

    /// Validate the configuration without network access.
    ///
    /// Checks required fields, account ids, URL schemes, the signature-key /
    /// webhook-URL pairing and the default account. Network reachability is
    /// deliberately not checked here (that is a readiness concern).
    pub fn validate(&self) -> Result<(), webx_pay::PaymentError> {
        self.build_accounts()
            .map(|_| ())
            .map_err(|error| webx_pay::PaymentError::config(ProviderId::Square, error))
    }

    /// The configured notification URL, when non-empty.
    pub(crate) fn webhook_url(&self) -> Option<String> {
        self.webhook_url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_string)
    }

    pub(crate) fn http_options(&self) -> HttpClientOptions {
        // Requests carry absolute per-account URLs (accounts can mix live and
        // sandbox), so the client base URL is only a placeholder for this one.
        HttpClientOptions {
            base_url: LIVE_API_BASE.to_string(),
            connect_timeout: Duration::from_secs(self.connect_timeout_seconds.max(1)),
            request_timeout: Some(Duration::from_secs(self.timeout_seconds.max(1))),
            read_timeout: Some(Duration::from_secs(self.timeout_seconds.max(1))),
            allow_private_networks: self.allow_private_networks,
            user_agent: format!("rust-webx-pay-square/{}", env!("CARGO_PKG_VERSION")),
            ..HttpClientOptions::default()
        }
    }

    /// Parse the configured accounts into credentials and validate every field
    /// that does not need a network call.
    pub(crate) fn build_accounts(&self) -> Result<webx_pay::AccountSet<SquareAccount>, String> {
        if self.accounts.is_empty() {
            return Err("square requires at least one account".to_string());
        }
        if let Some(webhook_url) = self.webhook_url() {
            if !(webhook_url.starts_with("https://")
                || (self.allow_private_networks && webhook_url.starts_with("http://")))
            {
                return Err(
                    "square webhook_url must be an https URL (http is allowed only with allow_private_networks)"
                        .to_string(),
                );
            }
        }
        let mut converted = Vec::new();
        for account in &self.accounts {
            let credentials = account.clone().into_account(self.allow_private_networks)?;
            let info = webx_pay::PaymentAccount {
                id: credentials.id.clone(),
                provider: ProviderId::Square,
                live: credentials.live,
                label: None,
            };
            converted.push((credentials.id.clone(), credentials, info));
        }
        let has_signature_key = converted
            .iter()
            .any(|(_, account, _)| account.signature_key.is_some());
        if has_signature_key && self.webhook_url().is_none() {
            return Err(
                "square webhook_url is required to verify webhooks when a signature_key is configured"
                    .to_string(),
            );
        }
        let default = self
            .default_account
            .clone()
            .map(webx_pay::AccountId::from)
            .unwrap_or_else(|| converted[0].0.clone());
        webx_pay::AccountSet::new(ProviderId::Square, default, converted)
            .map_err(|error| error.to_string())
    }
}

impl fmt::Debug for SquareOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SquareOptions")
            .field("accounts", &self.accounts)
            .field("default_account", &self.default_account)
            .field("webhook_url", &self.webhook_url)
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
    fn single_account_defaults_to_production_and_pinned_version() {
        let options = SquareOptions::new("test-access-token");
        let accounts = options.build_accounts().unwrap();
        assert_eq!(accounts.default_id().as_str(), "default");
        assert_eq!(accounts.accounts().len(), 1);
        assert!(accounts.accounts()[0].live);
        assert_eq!(
            options.accounts[0].environment,
            SquareEnvironment::Production
        );
        assert_eq!(options.accounts[0].api_version, DEFAULT_API_VERSION);
    }

    #[test]
    fn sandbox_accounts_are_not_live() {
        let options = SquareOptions::new("test-access-token").with_account(
            SquareAccountOptions::new("sandbox-test-token")
                .with_id("sandbox")
                .with_environment(SquareEnvironment::Sandbox)
                .with_location_id("TESTLOCATION1")
                .with_signature_key("test-signature-key"),
        );
        let options = options.with_webhook_url("https://example.com/webhooks/payments/square");
        let accounts = options.build_accounts().unwrap();
        assert_eq!(accounts.accounts().len(), 2);
        assert!(!accounts.accounts()[1].live);
        let sandbox = accounts
            .resolve(Some(&webx_pay::AccountId::from("sandbox")))
            .unwrap();
        assert!(sandbox.location_id.is_some());
    }

    #[test]
    fn application_id_is_optional_public_and_trimmed() {
        assert!(SquareAccountOptions::new("test-access-token")
            .application_id
            .is_none());
        let parsed: SquareAccountOptions =
            serde_json::from_str(r#"{"access_token": "test-access-token"}"#).unwrap();
        assert!(parsed.application_id.is_none());

        let options = SquareOptions::empty().with_account(
            SquareAccountOptions::new("test-access-token")
                .with_application_id("  test-application-id  "),
        );
        let accounts = options.build_accounts().unwrap();
        assert_eq!(
            accounts.resolve(None).unwrap().application_id.as_deref(),
            Some("test-application-id")
        );
        // A public identifier is safe to print, unlike the credentials.
        assert!(format!("{options:?}").contains("test-application-id"));

        let blank = SquareOptions::empty().with_account(
            SquareAccountOptions::new("test-access-token").with_application_id("   "),
        );
        assert!(blank
            .build_accounts()
            .unwrap()
            .resolve(None)
            .unwrap()
            .application_id
            .is_none());
    }

    #[test]
    fn debug_never_prints_secrets() {
        let options = SquareOptions::new("super-secret-access-token").with_account(
            SquareAccountOptions::new("other-super-secret-token")
                .with_signature_key("super-secret-signature-key")
                .with_application_id("test-application-id"),
        );
        let text = format!("{options:?}");
        assert!(!text.contains("super-secret"));
        assert!(text.contains("<redacted>"));
        assert!(text.contains("test-application-id"));
    }

    #[test]
    fn rejects_missing_accounts_bad_defaults_and_plaintext_bases() {
        assert!(SquareOptions::empty().build_accounts().is_err());

        let options = SquareOptions::new("test-access-token").with_default_account("missing");
        assert!(options.build_accounts().is_err());

        let options = SquareOptions::new("test-access-token").with_account(
            SquareAccountOptions::new("second")
                .with_id("second")
                .with_api_base("http://api.example.com"),
        );
        assert!(options.clone().build_accounts().is_err());
        assert!(options
            .allow_private_networks(true)
            .build_accounts()
            .is_ok());
    }

    #[test]
    fn signature_key_requires_a_webhook_url() {
        let options = SquareOptions::new("test-access-token").with_account(
            SquareAccountOptions::new("second")
                .with_id("second")
                .with_signature_key("test-signature-key"),
        );
        assert!(options.build_accounts().is_err());
        assert!(options
            .with_webhook_url("https://example.com/webhooks/payments/square")
            .build_accounts()
            .is_ok());
    }

    #[test]
    fn validate_reports_configuration_errors() {
        assert!(SquareOptions::new("test-access-token").validate().is_ok());
        let error = SquareOptions::empty().validate().unwrap_err();
        assert_eq!(error.http_status(), 500);
        assert!(error.to_string().contains("at least one account"));
    }

    #[test]
    fn binds_from_configuration_json() {
        // Shape of an appsettings `Square` section.
        let options: SquareOptions = serde_json::from_str(
            r#"{
                "accounts": [
                    {
                        "id": "us",
                        "access_token": "test-access-token",
                        "signature_key": "test-signature-key",
                        "location_id": "TESTLOCATION1",
                        "application_id": "test-application-id",
                        "api_version": "2025-06-18",
                        "environment": "sandbox"
                    }
                ],
                "default_account": "us",
                "webhook_url": "https://example.com/webhooks/payments/square",
                "timeout_seconds": 15
            }"#,
        )
        .unwrap();
        assert_eq!(options.timeout_seconds, 15);
        assert_eq!(options.accounts[0].api_version, "2025-06-18");
        assert_eq!(options.accounts[0].environment, SquareEnvironment::Sandbox);
        assert_eq!(
            options.accounts[0].application_id.as_deref(),
            Some("test-application-id")
        );
        let accounts = options.build_accounts().unwrap();
        assert_eq!(accounts.default_id().as_str(), "us");
        assert!(!accounts.accounts()[0].live);
    }
}
