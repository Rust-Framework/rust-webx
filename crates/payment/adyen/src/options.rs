//! Adyen adapter configuration (multi-account aware, secrets redacted).
//!
//! Adyen has two Checkout API base URLs: the test environment
//! (`https://checkout-test.adyen.com`) and, for live accounts, a per-merchant
//! URL built from the `liveEndpointUrlPrefix` shown in the Customer Area:
//! `https://{prefix}-checkout-live.adyenpayments.com/checkout`. The API version
//! is a path segment (`v71` here) and is pinned by this adapter.
//!
//! Configuration can be bound from a top-level `"Adyen"` section:
//!
//! ```json
//! {
//!   "Adyen": {
//!     "accounts": [
//!       {
//!         "id": "default",
//!         "api_key": "",
//!         "merchant_account": "",
//!         "hmac_key": ""
//!       }
//!     ]
//!   }
//! }
//! ```

use std::fmt;
use std::sync::OnceLock;
use std::time::Duration;

use serde::Deserialize;
use webx_client::HttpClientOptions;
use webx_pay::{AccountId, AccountSet, PaymentAccount, PaymentError, ProviderId};

use crate::hmac::decode_hmac_key;
use crate::provider::AdyenAccount;

/// Provider id owned by this adapter.
pub(crate) const PROVIDER: ProviderId = ProviderId::Adyen;

/// Default test Checkout API base URL.
pub const TEST_API_BASE: &str = "https://checkout-test.adyen.com";

/// Default Checkout API version path segment.
pub const DEFAULT_API_VERSION: &str = "v71";

fn default_test_api_base() -> String {
    TEST_API_BASE.to_string()
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

fn default_account_id() -> String {
    "default".to_string()
}

/// One Adyen merchant account.
///
/// `api_key` is the Checkout API key (`X-API-Key`), `merchant_account` is the
/// Adyen merchant account code sent in every request, and `hmac_key` is the
/// hexadecimal HMAC key generated for the standard webhook endpoint.
#[derive(Clone, Deserialize)]
pub struct AdyenAccountOptions {
    /// Account id used in requests and the webhook account path segment.
    #[serde(default = "default_account_id")]
    pub id: String,
    /// Checkout API key (`X-API-Key`).
    pub api_key: String,
    /// Adyen merchant account code.
    pub merchant_account: String,
    /// Hexadecimal HMAC key of the standard webhook endpoint.
    pub hmac_key: String,
    /// Optional client-side key (Drop-in/Components), published to frontends
    /// through [`IPaymentProvider::payment_options`](webx_pay::IPaymentProvider::payment_options).
    ///
    /// This is the only Adyen credential that leaves the server; it is never
    /// used in server-to-server calls.
    #[serde(default)]
    pub client_key: Option<String>,
    /// Whether this account talks to the live environment.
    #[serde(default)]
    pub live: bool,
    /// Live endpoint URL prefix from the Customer Area (`Developers` > `API URLs`).
    ///
    /// Required for live accounts unless `api_base` or `AdyenOptions::live_api_base`
    /// is configured.
    #[serde(default)]
    pub live_url_prefix: Option<String>,
    /// Explicit Checkout API base URL override (proxy/gateway/tests).
    #[serde(default)]
    pub api_base: Option<String>,
    /// Per-account response timeout override in seconds.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

impl AdyenAccountOptions {
    /// Account options for one merchant account.
    pub fn new(
        api_key: impl Into<String>,
        merchant_account: impl Into<String>,
        hmac_key: impl Into<String>,
    ) -> Self {
        Self {
            id: default_account_id(),
            api_key: api_key.into(),
            merchant_account: merchant_account.into(),
            hmac_key: hmac_key.into(),
            client_key: None,
            live: false,
            live_url_prefix: None,
            api_base: None,
            timeout_seconds: None,
        }
    }

    /// Rename the account (multi-account mode).
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Provide the client-side key published through the frontend payment catalog.
    pub fn with_client_key(mut self, client_key: impl Into<String>) -> Self {
        self.client_key = Some(client_key.into());
        self
    }

    /// Mark the account as live (uses the live Checkout URL).
    pub fn with_live(mut self, live: bool) -> Self {
        self.live = live;
        self
    }

    /// Set the live endpoint URL prefix.
    pub fn with_live_url_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.live_url_prefix = Some(prefix.into());
        self
    }

    /// Force an explicit Checkout API base URL for this account.
    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = Some(base.into());
        self
    }

    /// Override the response timeout for this account.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout_seconds = Some(timeout.as_secs().max(1));
        self
    }
}

impl fmt::Debug for AdyenAccountOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdyenAccountOptions")
            .field("id", &self.id)
            .field("api_key", &"<redacted>")
            .field("merchant_account", &self.merchant_account)
            .field("hmac_key", &"<redacted>")
            .field(
                "client_key",
                &self.client_key.as_ref().map(|_| "<redacted>"),
            )
            .field("live", &self.live)
            .field("live_url_prefix", &self.live_url_prefix)
            .field("api_base", &self.api_base)
            .field("timeout_seconds", &self.timeout_seconds)
            .finish()
    }
}

/// Adyen adapter options.
///
/// The single-account constructor is the common case; [`AdyenOptions::with_account`]
/// adds more accounts (service-provider / multi-tenant mode). Accounts are
/// routed by [`AccountId`](webx_pay::AccountId), and webhook deliveries are
/// matched to an account by `merchantAccountCode`.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct AdyenOptions {
    /// Merchant accounts; at least one is required.
    pub accounts: Vec<AdyenAccountOptions>,
    /// Account used when a request does not select one (defaults to the first).
    #[serde(default)]
    pub default_account: Option<String>,
    /// Checkout API base URL used by test accounts.
    pub test_api_base: String,
    /// Checkout API base URL used by live accounts.
    ///
    /// When absent, live accounts derive it from their `live_url_prefix`.
    #[serde(default)]
    pub live_api_base: Option<String>,
    /// API version path segment (`v71`).
    pub api_version: String,
    /// Response-header timeout in seconds.
    pub timeout_seconds: u64,
    /// TCP/TLS connect timeout in seconds.
    pub connect_timeout_seconds: u64,
    /// Allow loopback/intranet endpoints (tests only; production stays `false`).
    pub allow_private_networks: bool,
}

impl Default for AdyenOptions {
    fn default() -> Self {
        Self {
            accounts: Vec::new(),
            default_account: None,
            test_api_base: default_test_api_base(),
            live_api_base: None,
            api_version: default_api_version(),
            timeout_seconds: default_timeout(),
            connect_timeout_seconds: default_connect_timeout(),
            allow_private_networks: false,
        }
    }
}

impl AdyenOptions {
    /// Single-account options from an API key, merchant account and HMAC key.
    pub fn new(
        api_key: impl Into<String>,
        merchant_account: impl Into<String>,
        hmac_key: impl Into<String>,
    ) -> Self {
        Self {
            accounts: vec![AdyenAccountOptions::new(
                api_key,
                merchant_account,
                hmac_key,
            )],
            ..Self::default()
        }
    }

    /// Options with no account (bind from configuration, then populate
    /// `accounts`).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Add a merchant account.
    pub fn with_account(mut self, account: AdyenAccountOptions) -> Self {
        self.accounts.push(account);
        self
    }

    /// Select the default account.
    pub fn with_default_account(mut self, id: impl Into<String>) -> Self {
        self.default_account = Some(id.into());
        self
    }

    /// Override the test Checkout API base URL.
    pub fn with_test_api_base(mut self, base: impl Into<String>) -> Self {
        self.test_api_base = base.into();
        self
    }

    /// Override the live Checkout API base URL (otherwise derived per account).
    pub fn with_live_api_base(mut self, base: impl Into<String>) -> Self {
        self.live_api_base = Some(base.into());
        self
    }

    /// Override the pinned Checkout API version (tests/migration only).
    pub fn with_api_version(mut self, version: impl Into<String>) -> Self {
        self.api_version = version.into();
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

    /// Static validation: required fields, hexadecimal HMAC keys, HTTPS
    /// endpoints and a usable live URL for every live account.
    pub fn validate(&self) -> Result<(), PaymentError> {
        self.build_accounts().map(|_| ())
    }

    /// Request path for a Checkout endpoint (`"/payments"` -> `"/v71/payments"`).
    pub(crate) fn api_path(&self, endpoint: &str) -> String {
        format!("/{}{}", self.api_version.trim(), endpoint)
    }

    /// Parse the configured accounts into credentials and validate fields that
    /// do not need a network call.
    pub(crate) fn build_accounts(&self) -> Result<AccountSet<AdyenAccount>, PaymentError> {
        if self.accounts.is_empty() {
            return Err(PaymentError::config(
                PROVIDER,
                "adyen requires at least one account",
            ));
        }
        self.validate_api_version()?;
        self.validate_base_url("test_api_base", &self.test_api_base)?;
        if let Some(base) = &self.live_api_base {
            self.validate_base_url("live_api_base", base)?;
        }
        let mut converted = Vec::with_capacity(self.accounts.len());
        for account in &self.accounts {
            let credentials = self.build_account(account)?;
            let info = PaymentAccount {
                id: credentials.id.clone(),
                provider: PROVIDER,
                live: credentials.live,
                label: None,
            };
            converted.push((credentials.id.clone(), credentials, info));
        }
        let default = match &self.default_account {
            Some(id) => {
                AccountId::new(id.clone()).map_err(|error| PaymentError::config(PROVIDER, error))?
            }
            None => converted[0].0.clone(),
        };
        AccountSet::new(PROVIDER, default, converted)
    }

    fn build_account(&self, options: &AdyenAccountOptions) -> Result<AdyenAccount, PaymentError> {
        let id = AccountId::new(options.id.clone())
            .map_err(|error| PaymentError::config(PROVIDER, error))?;
        validate_required(PROVIDER, &id, "api_key", &options.api_key)?;
        validate_required(PROVIDER, &id, "merchant_account", &options.merchant_account)?;
        let hmac_key = decode_hmac_key(&options.hmac_key)?;
        let base_url = self.account_base_url(options)?;
        self.validate_base_url("account api_base", &base_url)?;
        let timeout_seconds = options
            .timeout_seconds
            .unwrap_or(self.timeout_seconds)
            .max(1);
        Ok(AdyenAccount {
            id,
            api_key: options.api_key.trim().to_string(),
            merchant_account: options.merchant_account.trim().to_string(),
            hmac_key,
            client_key: options
                .client_key
                .clone()
                .filter(|key| !key.trim().is_empty()),
            live: options.live,
            base_url: clean_base(&base_url),
            http: self.http_options(&base_url, timeout_seconds),
            client: OnceLock::new(),
        })
    }

    fn account_base_url(&self, options: &AdyenAccountOptions) -> Result<String, PaymentError> {
        if let Some(base) = &options.api_base {
            return Ok(base.clone());
        }
        if !options.live {
            return Ok(self.test_api_base.clone());
        }
        if let Some(base) = &self.live_api_base {
            return Ok(base.clone());
        }
        let prefix = options
            .live_url_prefix
            .as_deref()
            .map(str::trim)
            .filter(|prefix| !prefix.is_empty())
            .ok_or_else(|| {
                PaymentError::config(
                    PROVIDER,
                    format!(
                        "live adyen account `{}` requires live_url_prefix (or live_api_base/api_base)",
                        options.id
                    ),
                )
            })?;
        if !prefix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(PaymentError::config(
                PROVIDER,
                format!(
                    "live adyen account `{}` live_url_prefix must be alphanumeric/hyphen",
                    options.id
                ),
            ));
        }
        Ok(format!(
            "https://{prefix}-checkout-live.adyenpayments.com/checkout"
        ))
    }

    fn validate_api_version(&self) -> Result<(), PaymentError> {
        let version = self.api_version.trim();
        if version.is_empty()
            || !version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.')
        {
            return Err(PaymentError::config(
                PROVIDER,
                "api_version must be a path segment such as `v71`",
            ));
        }
        Ok(())
    }

    fn validate_base_url(&self, label: &str, url: &str) -> Result<(), PaymentError> {
        let url = url.trim();
        if url.is_empty() || url.chars().any(char::is_control) {
            return Err(PaymentError::config(
                PROVIDER,
                format!("{label} must be a non-empty URL without control characters"),
            ));
        }
        let https = url.starts_with("https://");
        let http_allowed = self.allow_private_networks && url.starts_with("http://");
        if !https && !http_allowed {
            return Err(PaymentError::config(
                PROVIDER,
                format!(
                    "{label} must be an https URL (http is allowed only with allow_private_networks)"
                ),
            ));
        }
        Ok(())
    }

    fn http_options(&self, base_url: &str, timeout_seconds: u64) -> HttpClientOptions {
        HttpClientOptions {
            base_url: clean_base(base_url),
            connect_timeout: Duration::from_secs(self.connect_timeout_seconds.max(1)),
            request_timeout: Some(Duration::from_secs(timeout_seconds)),
            read_timeout: Some(Duration::from_secs(timeout_seconds)),
            allow_private_networks: self.allow_private_networks,
            user_agent: format!("rust-webx-pay-adyen/{}", env!("CARGO_PKG_VERSION")),
            ..HttpClientOptions::default()
        }
    }
}

impl fmt::Debug for AdyenOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdyenOptions")
            .field("accounts", &self.accounts)
            .field("default_account", &self.default_account)
            .field("test_api_base", &self.test_api_base)
            .field("live_api_base", &self.live_api_base)
            .field("api_version", &self.api_version)
            .field("timeout_seconds", &self.timeout_seconds)
            .field("connect_timeout_seconds", &self.connect_timeout_seconds)
            .field("allow_private_networks", &self.allow_private_networks)
            .finish()
    }
}

fn clean_base(base: &str) -> String {
    base.trim().trim_end_matches('/').to_string()
}

fn validate_required(
    provider: ProviderId,
    account: &AccountId,
    field: &str,
    value: &str,
) -> Result<(), PaymentError> {
    if value.trim().is_empty() {
        return Err(PaymentError::config(
            provider,
            format!("adyen account `{account}` {field} must not be empty"),
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(PaymentError::config(
            provider,
            format!("adyen account `{account}` {field} must not contain control characters"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HMAC_FIXTURE: &str = "00112233445566778899aabbccddeeff";

    #[test]
    fn single_account_defaults_to_the_default_id() {
        let options = AdyenOptions::new("fixture-api-key", "FixtureMerchant", HMAC_FIXTURE);
        let accounts = options.build_accounts().unwrap();
        assert_eq!(accounts.default_id().as_str(), "default");
        assert_eq!(accounts.accounts().len(), 1);
        assert!(!accounts.accounts()[0].live);
        assert_eq!(
            accounts.resolve(None).unwrap().base_url,
            "https://checkout-test.adyen.com"
        );
    }

    #[test]
    fn api_path_pins_the_version() {
        let options = AdyenOptions::new("fixture-api-key", "FixtureMerchant", HMAC_FIXTURE);
        assert_eq!(options.api_path("/payments"), "/v71/payments");
        let options = options.with_api_version("v72");
        assert_eq!(options.api_path("/payments"), "/v72/payments");
        assert!(options.with_api_version("v71/../x").validate().is_err());
    }

    #[test]
    fn live_accounts_derive_the_checkout_url_from_the_prefix() {
        let options = AdyenOptions::new("fixture-api-key", "FixtureMerchant", HMAC_FIXTURE)
            .with_account(
                AdyenAccountOptions::new("fixture-api-key-2", "FixtureMerchant2", HMAC_FIXTURE)
                    .with_id("live")
                    .with_live(true)
                    .with_live_url_prefix("fixture-prefix"),
            )
            .with_default_account("default");
        let accounts = options.build_accounts().unwrap();
        let live = accounts.resolve(Some(&AccountId::from("live"))).unwrap();
        assert!(live.live);
        assert_eq!(
            live.base_url,
            "https://fixture-prefix-checkout-live.adyenpayments.com/checkout"
        );
    }

    #[test]
    fn live_accounts_without_a_url_are_a_configuration_error() {
        let options = AdyenOptions::new("fixture-api-key", "FixtureMerchant", HMAC_FIXTURE)
            .with_account(
                AdyenAccountOptions::new("fixture-api-key", "FixtureMerchant", HMAC_FIXTURE)
                    .with_id("live")
                    .with_live(true),
            );
        let error = options.validate().unwrap_err();
        assert_eq!(error.http_status(), 500);
        assert!(error.to_string().contains("live_url_prefix"));
    }

    #[test]
    fn missing_http_base_and_bad_hmac_keys_are_configuration_errors() {
        let options = AdyenOptions::new("fixture-api-key", "FixtureMerchant", "not-hex");
        assert!(options.validate().is_err());

        let options = AdyenOptions::new("fixture-api-key", "FixtureMerchant", HMAC_FIXTURE)
            .with_test_api_base("http://api.example.com");
        assert!(options.validate().is_err());
        assert!(options.allow_private_networks(true).validate().is_ok());

        let options = AdyenOptions::new("", "FixtureMerchant", HMAC_FIXTURE);
        assert!(options.validate().is_err());
        let options = AdyenOptions::new("fixture-api-key", " ", HMAC_FIXTURE);
        assert!(options.validate().is_err());
    }

    #[test]
    fn duplicate_default_and_unknown_accounts_are_rejected() {
        let account =
            || AdyenAccountOptions::new("fixture-api-key", "FixtureMerchant", HMAC_FIXTURE);
        let options = AdyenOptions::new("fixture-api-key", "FixtureMerchant", HMAC_FIXTURE)
            .with_account(account().with_id("default"));
        assert!(options.validate().is_err());

        let options = AdyenOptions::new("fixture-api-key", "FixtureMerchant", HMAC_FIXTURE)
            .with_default_account("missing");
        assert!(options.validate().is_err());
    }

    #[test]
    fn binds_from_the_adyen_configuration_section() {
        let options: AdyenOptions = serde_json::from_value(serde_json::json!({
            "accounts": [
                {
                    "api_key": "fixture-api-key",
                    "merchant_account": "FixtureMerchant",
                    "hmac_key": HMAC_FIXTURE
                }
            ],
            "timeout_seconds": 10
        }))
        .unwrap();
        assert_eq!(options.api_version, "v71");
        assert_eq!(options.timeout_seconds, 10);
        assert_eq!(options.accounts.len(), 1);
        assert!(options.validate().is_ok());
    }

    #[test]
    fn live_api_base_overrides_the_derived_url() {
        let options = AdyenOptions::new("fixture-api-key", "FixtureMerchant", HMAC_FIXTURE)
            .with_account(
                AdyenAccountOptions::new("fixture-api-key-2", "FixtureMerchant2", HMAC_FIXTURE)
                    .with_id("live")
                    .with_live(true),
            )
            .with_default_account("default")
            .with_live_api_base("https://live-gateway.example.com/checkout");
        let accounts = options.build_accounts().unwrap();
        let live = accounts.resolve(Some(&AccountId::from("live"))).unwrap();
        assert_eq!(live.base_url, "https://live-gateway.example.com/checkout");
    }

    #[test]
    fn debug_never_prints_secrets() {
        let options = AdyenOptions::new(
            "fixture-api-key-value",
            "FixtureMerchant",
            "00112233445566778899aabbccddeeffaabbccddeeff0011",
        );
        let text = format!("{options:?}");
        assert!(!text.contains("fixture-api-key-value"));
        assert!(!text.contains("00112233445566778899aabbccddeeff"));
        assert!(text.contains("<redacted>"));
        assert!(text.contains("FixtureMerchant"));
    }
}
