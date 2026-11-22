//! Alipay adapter configuration (multi-account aware, secrets redacted).
//!
//! Bind from the `"Alipay"` section with
//! [`AlipayHostBuilderExt::add_alipay`](crate::AlipayHostBuilderExt::add_alipay)
//! or build in code with [`AlipayOptions::new`]:
//!
//! ```json
//! {
//!   "Alipay": {
//!     "accounts": [
//!       {
//!         "id": "default",
//!         "app_id": "2021...",
//!         "private_key_pem": "-----BEGIN PRIVATE KEY-----...",
//!         "mode": { "mode": "public_key", "alipay_public_key_pem": "..." },
//!         "notify_url": "https://example.com/webhooks/payment/alipay",
//!         "return_url": "https://example.com/payment/return"
//!       }
//!     ],
//!     "default_account": "default",
//!     "timeout_seconds": 30
//!   }
//! }
//! ```
//!
//! Certificate mode replaces the `public_key` mode object with
//! `{ "mode": "certificate", "app_cert_pem": "...", "alipay_public_cert_pem":
//! "...", "alipay_root_cert_pem": "..." }`.

use std::fmt;
use std::time::Duration;

use serde::Deserialize;
use webx_client::HttpClientOptions;
use webx_pay::{AccountId, AccountSet, PaymentAccount, PaymentError};

use crate::provider::AlipayAccount;
use crate::{sign, PROVIDER};

const PRODUCTION_GATEWAY: &str = "https://openapi.alipay.com/gateway.do";
const SANDBOX_GATEWAY: &str = "https://openapi-sandbox.dl.alipaydev.com/gateway.do";

fn default_timeout() -> u64 {
    30
}

fn default_connect_timeout() -> u64 {
    5
}

fn default_account_id() -> String {
    "default".to_string()
}

/// How the adapter authenticates Alipay responses and notifications.
///
/// * `public_key` — the classic 公钥模式: Alipay's public key is configured
///   directly.
/// * `certificate` — 公钥证书模式: the application certificate SN and Alipay
///   root certificate SN are sent with every request, and Alipay's public key
///   is taken from its public-key certificate.
///
/// The certificate files are read once at startup. When Alipay rotates its
/// public-key certificate, update `alipay_public_cert_pem`/`alipay_root_cert_pem`
/// and restart (or rebuild the provider) — a rotated certificate that does not
/// match the configured one makes response/notification verification fail
/// closed.
#[derive(Clone, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AlipayKeyMode {
    /// Alipay public key mode.
    PublicKey {
        /// Alipay public key (PEM or bare base64).
        alipay_public_key_pem: String,
    },
    /// Alipay public-key certificate mode.
    Certificate {
        /// Application public-key certificate (`.crt` PEM).
        app_cert_pem: String,
        /// Alipay public-key certificate (`.crt` PEM).
        alipay_public_cert_pem: String,
        /// Alipay root certificate chain (`.crt` PEM).
        alipay_root_cert_pem: String,
    },
}

impl AlipayKeyMode {
    /// Public-key mode from Alipay's public key.
    pub fn public_key(alipay_public_key_pem: impl Into<String>) -> Self {
        Self::PublicKey {
            alipay_public_key_pem: alipay_public_key_pem.into(),
        }
    }

    /// Certificate mode from the three Alipay certificate files.
    pub fn certificate(
        app_cert_pem: impl Into<String>,
        alipay_public_cert_pem: impl Into<String>,
        alipay_root_cert_pem: impl Into<String>,
    ) -> Self {
        Self::Certificate {
            app_cert_pem: app_cert_pem.into(),
            alipay_public_cert_pem: alipay_public_cert_pem.into(),
            alipay_root_cert_pem: alipay_root_cert_pem.into(),
        }
    }

    fn certificate_mode(&self) -> bool {
        matches!(self, Self::Certificate { .. })
    }
}

impl fmt::Debug for AlipayKeyMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PublicKey { .. } => f
                .debug_struct("PublicKey")
                .field("alipay_public_key_pem", &"<redacted>")
                .finish(),
            Self::Certificate { .. } => f
                .debug_struct("Certificate")
                .field("app_cert_pem", &"<redacted>")
                .field("alipay_public_cert_pem", &"<redacted>")
                .field("alipay_root_cert_pem", &"<redacted>")
                .finish(),
        }
    }
}

/// One Alipay merchant account (an Alipay "应用").
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct AlipayAccountOptions {
    /// Account id used in requests and the webhook account path segment.
    #[serde(default = "default_account_id")]
    pub id: String,
    /// Alipay application id (`app_id`).
    pub app_id: String,
    /// Application private key (RSA2, PEM or bare base64).
    pub private_key_pem: String,
    /// Response/notification verification mode.
    pub mode: Option<AlipayKeyMode>,
    /// Default asynchronous notification URL for this account.
    pub notify_url: Option<String>,
    /// Default synchronous return URL for page/WAP payments.
    pub return_url: Option<String>,
    /// Override the live/sandbox classification (`None` derives it from the
    /// adapter's `sandbox` flag).
    pub live: Option<bool>,
}

impl Default for AlipayAccountOptions {
    fn default() -> Self {
        Self {
            id: default_account_id(),
            app_id: String::new(),
            private_key_pem: String::new(),
            mode: None,
            notify_url: None,
            return_url: None,
            live: None,
        }
    }
}

impl AlipayAccountOptions {
    /// Account with `app_id`, its private key and a verification mode.
    pub fn new(
        app_id: impl Into<String>,
        private_key_pem: impl Into<String>,
        mode: AlipayKeyMode,
    ) -> Self {
        Self {
            id: default_account_id(),
            app_id: app_id.into(),
            private_key_pem: private_key_pem.into(),
            mode: Some(mode),
            notify_url: None,
            return_url: None,
            live: None,
        }
    }

    /// Rename the account (multi-account mode).
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Set the asynchronous notification URL.
    pub fn with_notify_url(mut self, notify_url: impl Into<String>) -> Self {
        self.notify_url = Some(notify_url.into());
        self
    }

    /// Set the synchronous return URL.
    pub fn with_return_url(mut self, return_url: impl Into<String>) -> Self {
        self.return_url = Some(return_url.into());
        self
    }

    /// Override the live/sandbox classification.
    pub fn with_live(mut self, live: bool) -> Self {
        self.live = Some(live);
        self
    }

    /// Parse the keys/certificates and build the runtime account.
    pub(crate) fn into_account(self, sandbox: bool) -> Result<AlipayAccount, String> {
        let id = AccountId::new(self.id.clone())?;
        let context = |detail: String| format!("alipay account `{id}`: {detail}");
        if self.app_id.trim().is_empty() {
            return Err(context("app_id must not be empty".to_string()));
        }
        if self.private_key_pem.trim().is_empty() {
            return Err(context("private_key_pem must not be empty".to_string()));
        }
        let private_key = sign::parse_private_key(&self.private_key_pem).map_err(context)?;
        let mode = self.mode.ok_or_else(|| {
            context("mode is required (`public_key` or `certificate`)".to_string())
        })?;
        let certificate_mode = mode.certificate_mode();
        let (public_key, app_cert_sn, alipay_root_cert_sn) = match mode {
            AlipayKeyMode::PublicKey {
                alipay_public_key_pem,
            } => (
                sign::parse_public_key(&alipay_public_key_pem).map_err(context)?,
                None,
                None,
            ),
            AlipayKeyMode::Certificate {
                app_cert_pem,
                alipay_public_cert_pem,
                alipay_root_cert_pem,
            } => {
                let app_cert_sn = sign::certificate_sn(&app_cert_pem).map_err(context)?;
                let public_key =
                    sign::certificate_public_key(&alipay_public_cert_pem).map_err(context)?;
                let root_cert_sn =
                    sign::root_certificate_sn(&alipay_root_cert_pem).map_err(context)?;
                (public_key, Some(app_cert_sn), Some(root_cert_sn))
            }
        };
        let notify_url = normalize_url(self.notify_url, "notify_url").map_err(context)?;
        let return_url = normalize_url(self.return_url, "return_url").map_err(context)?;
        let live = self.live.unwrap_or(!sandbox);
        Ok(AlipayAccount {
            id,
            app_id: self.app_id.trim().to_string(),
            private_key,
            public_key,
            certificate_mode,
            app_cert_sn,
            alipay_root_cert_sn,
            notify_url,
            return_url,
            live,
        })
    }
}

impl fmt::Debug for AlipayAccountOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AlipayAccountOptions")
            .field("id", &self.id)
            .field("app_id", &self.app_id)
            .field("private_key_pem", &"<redacted>")
            .field("mode", &self.mode)
            .field("notify_url", &self.notify_url)
            .field("return_url", &self.return_url)
            .field("live", &self.live)
            .finish()
    }
}

/// Alipay adapter options.
///
/// One adapter instance can hold several Alipay applications (multi-account /
/// multi-tenant); requests select an account through
/// [`PaymentScope::account`](webx_pay::PaymentScope::account), otherwise the
/// default account is used.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct AlipayOptions {
    /// Merchant accounts; at least one is required.
    pub accounts: Vec<AlipayAccountOptions>,
    /// Account used when a request does not select one (defaults to the first).
    pub default_account: Option<String>,
    /// Gateway URL override (tests/proxies); defaults to production or sandbox.
    pub gateway: Option<String>,
    /// Use the Alipay sandbox gateway and classify accounts as non-live.
    pub sandbox: bool,
    /// Response-header timeout in seconds.
    pub timeout_seconds: u64,
    /// TCP/TLS connect timeout in seconds.
    pub connect_timeout_seconds: u64,
    /// Allow loopback/intranet gateways (tests only; production stays `false`).
    pub allow_private_networks: bool,
}

impl Default for AlipayOptions {
    fn default() -> Self {
        Self {
            accounts: Vec::new(),
            default_account: None,
            gateway: None,
            sandbox: false,
            timeout_seconds: default_timeout(),
            connect_timeout_seconds: default_connect_timeout(),
            allow_private_networks: false,
        }
    }
}

impl AlipayOptions {
    /// Single-account options in public-key mode.
    ///
    /// `private_key_pem` is the application private key; the third argument is
    /// Alipay's public key. Both accept PEM or bare base64.
    pub fn new(
        app_id: impl Into<String>,
        private_key_pem: impl Into<String>,
        alipay_public_key_pem: impl Into<String>,
    ) -> Self {
        Self {
            accounts: vec![AlipayAccountOptions::new(
                app_id,
                private_key_pem,
                AlipayKeyMode::public_key(alipay_public_key_pem),
            )],
            ..Self::default()
        }
    }

    /// Single-account options in certificate mode.
    pub fn new_certificate(
        app_id: impl Into<String>,
        private_key_pem: impl Into<String>,
        app_cert_pem: impl Into<String>,
        alipay_public_cert_pem: impl Into<String>,
        alipay_root_cert_pem: impl Into<String>,
    ) -> Self {
        Self {
            accounts: vec![AlipayAccountOptions::new(
                app_id,
                private_key_pem,
                AlipayKeyMode::certificate(
                    app_cert_pem,
                    alipay_public_cert_pem,
                    alipay_root_cert_pem,
                ),
            )],
            ..Self::default()
        }
    }

    /// Options with no account (bind from configuration, then populate).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Add a merchant account.
    pub fn with_account(
        mut self,
        id: impl Into<String>,
        app_id: impl Into<String>,
        private_key_pem: impl Into<String>,
        mode: AlipayKeyMode,
    ) -> Self {
        self.accounts
            .push(AlipayAccountOptions::new(app_id, private_key_pem, mode).with_id(id));
        self
    }

    /// Select the default account.
    pub fn with_default_account(mut self, id: impl Into<String>) -> Self {
        self.default_account = Some(id.into());
        self
    }

    /// Override the gateway URL.
    pub fn with_gateway(mut self, gateway: impl Into<String>) -> Self {
        self.gateway = Some(gateway.into());
        self
    }

    /// Use the sandbox gateway (and classify accounts as non-live).
    pub fn with_sandbox(mut self, sandbox: bool) -> Self {
        self.sandbox = sandbox;
        self
    }

    /// Response-header timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout_seconds = timeout.as_secs().max(1);
        self
    }

    /// Allow loopback/intranet gateway destinations (tests only).
    pub fn allow_private_networks(mut self, allow: bool) -> Self {
        self.allow_private_networks = allow;
        self
    }

    /// The effective gateway URL.
    pub(crate) fn gateway_url(&self) -> Result<String, String> {
        let gateway = match &self.gateway {
            Some(gateway) => gateway.clone(),
            None if self.sandbox => SANDBOX_GATEWAY.to_string(),
            None => PRODUCTION_GATEWAY.to_string(),
        };
        let secure = gateway.starts_with("https://")
            || (self.allow_private_networks && gateway.starts_with("http://"));
        if !secure {
            return Err(
                "alipay gateway must be an https URL (http is allowed only with allow_private_networks)"
                    .to_string(),
            );
        }
        Ok(gateway)
    }

    pub(crate) fn http_options(&self, gateway: &str) -> HttpClientOptions {
        HttpClientOptions {
            base_url: gateway.to_string(),
            connect_timeout: Duration::from_secs(self.connect_timeout_seconds.max(1)),
            request_timeout: Some(Duration::from_secs(self.timeout_seconds.max(1))),
            read_timeout: Some(Duration::from_secs(self.timeout_seconds.max(1))),
            allow_private_networks: self.allow_private_networks,
            user_agent: format!("rust-webx-pay-alipay/{}", env!("CARGO_PKG_VERSION")),
            ..HttpClientOptions::default()
        }
    }

    /// Parse the configured accounts and validate static settings.
    pub(crate) fn build_accounts(&self) -> Result<AccountSet<AlipayAccount>, String> {
        if self.accounts.is_empty() {
            return Err("alipay requires at least one account".to_string());
        }
        self.gateway_url()?;
        let mut converted = Vec::new();
        for account in &self.accounts {
            let credentials = account.clone().into_account(self.sandbox)?;
            let info = PaymentAccount {
                id: credentials.id.clone(),
                provider: PROVIDER,
                live: credentials.live,
                label: None,
            };
            converted.push((credentials.id.clone(), credentials, info));
        }
        let default = self
            .default_account
            .clone()
            .map(AccountId::from)
            .unwrap_or_else(|| converted[0].0.clone());
        AccountSet::new(PROVIDER, default, converted).map_err(|error| error.to_string())
    }

    /// Static configuration validation (no network access).
    pub fn validate(&self) -> Result<(), PaymentError> {
        self.build_accounts()
            .map(|_| ())
            .map_err(|detail| PaymentError::config(PROVIDER, detail))
    }
}

impl fmt::Debug for AlipayOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AlipayOptions")
            .field("accounts", &self.accounts)
            .field("default_account", &self.default_account)
            .field("gateway", &self.gateway)
            .field("sandbox", &self.sandbox)
            .field("timeout_seconds", &self.timeout_seconds)
            .field("connect_timeout_seconds", &self.connect_timeout_seconds)
            .field("allow_private_networks", &self.allow_private_networks)
            .finish()
    }
}

fn normalize_url(value: Option<String>, field: &str) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim().to_string();
    if value.is_empty() {
        return Ok(None);
    }
    let parsed =
        url::Url::parse(&value).map_err(|error| format!("{field} is not a valid URL: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("{field} must be an http(s) URL"));
    }
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    #[test]
    fn single_account_defaults_to_default_id_and_is_live() {
        let options = test_support::options();
        let accounts = options.build_accounts().unwrap();
        assert_eq!(accounts.default_id().as_str(), "default");
        assert_eq!(accounts.accounts().len(), 1);
        assert!(accounts.accounts()[0].live);
        assert_eq!(accounts.accounts()[0].provider, PROVIDER);
    }

    #[test]
    fn sandbox_and_live_are_classified() {
        let options = test_support::options().with_sandbox(true);
        let accounts = options.build_accounts().unwrap();
        assert!(!accounts.accounts()[0].live);

        let options = test_support::options().with_sandbox(true).with_account(
            "eu",
            test_support::APP_ID,
            test_support::merchant_private_key_pem(),
            AlipayKeyMode::public_key(test_support::alipay_public_key_pem()),
        );
        let accounts = options.build_accounts().unwrap();
        assert!(!accounts.accounts()[1].live);
    }

    #[test]
    fn certificate_mode_parses_certificates() {
        let options = AlipayOptions {
            accounts: vec![test_support::certificate_account_options()],
            ..AlipayOptions::default()
        };
        let accounts = options.build_accounts().unwrap();
        let account = accounts.resolve(None).unwrap();
        assert!(account.certificate_mode);
        assert_eq!(account.app_cert_sn.as_deref().map(str::len), Some(32));
        assert!(account
            .alipay_root_cert_sn
            .as_deref()
            .is_some_and(|sn| sn.contains('_')));
    }

    #[test]
    fn debug_never_prints_secrets() {
        let options = test_support::options();
        let text = format!("{options:?}");
        assert!(!text.contains("PRIVATE KEY"));
        assert!(!text
            .contains(test_support::pem_body(test_support::merchant_private_key_pem()).as_str()));
        assert!(text.contains("<redacted>"));

        let certificates = test_support::certificates();
        let certificate_options = AlipayOptions {
            accounts: vec![test_support::certificate_account_options()],
            ..AlipayOptions::default()
        };
        let text = format!("{certificate_options:?}");
        assert!(!text.contains("CERTIFICATE"));
        assert!(!text.contains(&certificates.app_cert_pem));
    }

    #[test]
    fn missing_or_plaintext_http_gateway_is_a_configuration_error() {
        assert!(AlipayOptions::empty().validate().is_err());
        let options = test_support::options().with_gateway("http://gateway.example.com/gateway.do");
        assert!(options.validate().is_err());
        assert!(options.allow_private_networks(true).validate().is_ok());
    }

    #[test]
    fn rejects_missing_mode_and_invalid_keys() {
        let mut account = test_support::account_options();
        account.mode = None;
        let options = AlipayOptions {
            accounts: vec![account],
            ..AlipayOptions::default()
        };
        let error = options.validate().unwrap_err().to_string();
        assert!(error.contains("mode is required"), "{error}");

        let mut account = test_support::account_options();
        account.private_key_pem = "not a key".to_string();
        let options = AlipayOptions {
            accounts: vec![account],
            ..AlipayOptions::default()
        };
        assert!(options.validate().is_err());
    }

    #[test]
    fn rejects_empty_app_id_and_bad_urls() {
        let mut account = test_support::account_options();
        account.app_id = String::new();
        let options = AlipayOptions {
            accounts: vec![account],
            ..AlipayOptions::default()
        };
        assert!(options.validate().is_err());

        let account = test_support::account_options().with_notify_url("not-a-url");
        let options = AlipayOptions {
            accounts: vec![account],
            ..AlipayOptions::default()
        };
        assert!(options.validate().is_err());
    }

    #[test]
    fn default_account_must_exist() {
        let options = test_support::options().with_default_account("missing");
        assert!(options.validate().is_err());
    }

    #[test]
    fn options_deserialize_from_configuration_shape() {
        let value = serde_json::json!({
            "accounts": [{
                "id": "default",
                "app_id": test_support::APP_ID,
                "private_key_pem": test_support::merchant_private_key_pem(),
                "mode": {
                    "mode": "public_key",
                    "alipay_public_key_pem": test_support::alipay_public_key_pem()
                },
                "notify_url": "https://example.com/notify"
            }],
            "timeout_seconds": 10
        });
        let options: AlipayOptions = serde_json::from_value(value).expect("options deserialize");
        assert_eq!(options.timeout_seconds, 10);
        assert!(options.validate().is_ok());
    }

    #[test]
    fn empty_configuration_defaults_and_fails_validation() {
        let options: AlipayOptions =
            serde_json::from_value(serde_json::json!({})).expect("empty options");
        assert!(options.accounts.is_empty());
        assert_eq!(options.timeout_seconds, 30);
        assert!(options.validate().is_err());
    }
}
