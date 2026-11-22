//! WeChat Pay adapter configuration (multi-account aware, secrets redacted).
//!
//! Two credential modes are supported, mirroring the two WeChat Pay verification
//! mechanisms:
//!
//! * [`WechatPayKeyMode::PlatformCertificates`] (legacy/current default): the
//!   platform certificates are downloaded from `GET /v3/certificates`, decrypted
//!   with the APIv3 key and cached by serial number (rotation aware).
//! * [`WechatPayKeyMode::PublicKey`]: the merchant downloaded the WeChat Pay
//!   public key from the merchant platform; responses are verified against it
//!   and the `Wechatpay-Serial` header must equal its `PUB_KEY_ID_...` id.

use std::fmt;
use std::time::Duration;

use serde::Deserialize;
use webx_client::HttpClientOptions;
use webx_pay::{AccountId, AccountSet, PaymentAccount};

use crate::provider::WechatPayAccount;
use crate::sign::{parse_private_key_pem, parse_public_key_pem};
use crate::{DEFAULT_API_BASE, PROVIDER};

fn default_api_base() -> String {
    DEFAULT_API_BASE.to_string()
}

fn default_timeout() -> u64 {
    30
}

fn default_connect_timeout() -> u64 {
    5
}

fn default_account_id() -> String {
    AccountId::DEFAULT.to_string()
}

/// How WeChat Pay signatures are verified for one account.
#[derive(Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WechatPayKeyMode {
    /// Download and rotate platform certificates from `GET /v3/certificates`
    /// (official platform-certificate mode).
    #[default]
    PlatformCertificates,
    /// Verify with the WeChat Pay public key downloaded from the merchant
    /// platform (`PUB_KEY_ID_...` mode).
    PublicKey {
        /// Public key id (`PUB_KEY_ID_...`), matched against
        /// `Wechatpay-Serial`.
        public_key_id: String,
        /// PEM encoded WeChat Pay public key (`BEGIN PUBLIC KEY`).
        public_key_pem: String,
    },
}

impl fmt::Debug for WechatPayKeyMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlatformCertificates => f.write_str("PlatformCertificates"),
            Self::PublicKey { public_key_id, .. } => f
                .debug_struct("PublicKey")
                .field("public_key_id", public_key_id)
                .field("public_key_pem", &"<redacted>")
                .finish(),
        }
    }
}

/// One WeChat Pay merchant account.
///
/// The merchant API certificate (`serial_no` + `private_key_pem`) signs
/// requests; the APIv3 key (`api_v3_key`, exactly 32 characters) decrypts
/// callback resources and platform certificates. Only `app_id` is public and
/// exposed by the frontend catalog; `serial_no`, `private_key_pem` and
/// `api_v3_key` are credentials and never leave the adapter.
#[derive(Clone, Deserialize)]
pub struct WechatPayAccountOptions {
    /// Account id used in requests and the webhook route segment.
    #[serde(default = "default_account_id")]
    pub id: String,
    /// Merchant id (`mchid`).
    pub mch_id: String,
    /// App/公众号/小程序 id bound to `mch_id` (public; exposed in the frontend
    /// payment catalog).
    pub app_id: String,
    /// Merchant API certificate serial number.
    pub serial_no: String,
    /// Merchant API certificate private key (PEM, PKCS#8 or PKCS#1).
    pub private_key_pem: String,
    /// APIv3 key, exactly 32 characters.
    pub api_v3_key: String,
    /// Default callback URL; a request may override it.
    #[serde(default)]
    pub notify_url: String,
    /// Response verification mode.
    #[serde(default)]
    pub key_mode: WechatPayKeyMode,
    /// Force the live/test classification instead of the default (`true`:
    /// WeChat Pay APIv3 has no sandbox host, merchant accounts are live).
    #[serde(default)]
    pub live: Option<bool>,
}

impl WechatPayAccountOptions {
    /// Account with the required merchant credentials.
    pub fn new(
        mch_id: impl Into<String>,
        app_id: impl Into<String>,
        serial_no: impl Into<String>,
        private_key_pem: impl Into<String>,
        api_v3_key: impl Into<String>,
    ) -> Self {
        Self {
            id: default_account_id(),
            mch_id: mch_id.into(),
            app_id: app_id.into(),
            serial_no: serial_no.into(),
            private_key_pem: private_key_pem.into(),
            api_v3_key: api_v3_key.into(),
            notify_url: String::new(),
            key_mode: WechatPayKeyMode::PlatformCertificates,
            live: None,
        }
    }

    /// Rename the account (multi-account mode).
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Default callback URL for this account.
    pub fn with_notify_url(mut self, notify_url: impl Into<String>) -> Self {
        self.notify_url = notify_url.into();
        self
    }

    /// Use the WeChat Pay public-key verification mode.
    pub fn with_public_key(
        mut self,
        public_key_id: impl Into<String>,
        public_key_pem: impl Into<String>,
    ) -> Self {
        self.key_mode = WechatPayKeyMode::PublicKey {
            public_key_id: public_key_id.into(),
            public_key_pem: public_key_pem.into(),
        };
        self
    }

    /// Override the live/test classification.
    pub fn with_live(mut self, live: bool) -> Self {
        self.live = Some(live);
        self
    }

    /// Static validation: required fields, PEM parsing, key lengths and URL
    /// schemes. Performs no network call.
    pub fn validate(&self) -> Result<(), String> {
        let account = AccountId::new(self.id.clone())
            .map_err(|error| format!("wechatpay account id is invalid: {error}"))?;
        if self.mch_id.trim().is_empty()
            || !self.mch_id.chars().all(|ch| ch.is_ascii_alphanumeric())
        {
            return Err(format!(
                "wechatpay account `{account}` mch_id must be an alphanumeric merchant id"
            ));
        }
        if self.app_id.trim().is_empty()
            || !self.app_id.chars().all(|ch| ch.is_ascii_alphanumeric())
        {
            return Err(format!(
                "wechatpay account `{account}` app_id must be an alphanumeric application id"
            ));
        }
        if self.serial_no.trim().is_empty()
            || !self
                .serial_no
                .trim()
                .chars()
                .all(|ch| ch.is_ascii_hexdigit())
        {
            return Err(format!(
                "wechatpay account `{account}` serial_no must be a hexadecimal merchant certificate serial"
            ));
        }
        if self.private_key_pem.trim().is_empty() {
            return Err(format!(
                "wechatpay account `{account}` private_key_pem is required"
            ));
        }
        parse_private_key_pem(&self.private_key_pem).map_err(|error| {
            format!("wechatpay account `{account}` private_key_pem is invalid: {error}")
        })?;
        if self.api_v3_key.len() != crate::crypto::API_V3_KEY_LEN {
            return Err(format!(
                "wechatpay account `{account}` api_v3_key must be exactly {} bytes",
                crate::crypto::API_V3_KEY_LEN
            ));
        }
        if !self.notify_url.is_empty() && !self.notify_url.starts_with("https://") {
            return Err(format!(
                "wechatpay account `{account}` notify_url must be an https URL"
            ));
        }
        if let WechatPayKeyMode::PublicKey {
            public_key_id,
            public_key_pem,
        } = &self.key_mode
        {
            if public_key_id.trim().is_empty() {
                return Err(format!(
                    "wechatpay account `{account}` public_key_id is required in public-key mode"
                ));
            }
            parse_public_key_pem(public_key_pem).map_err(|error| {
                format!("wechatpay account `{account}` public_key_pem is invalid: {error}")
            })?;
        }
        Ok(())
    }

    pub(crate) fn into_account(self) -> Result<WechatPayAccount, String> {
        self.validate()?;
        WechatPayAccount::from_options(&self)
    }
}

impl fmt::Debug for WechatPayAccountOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WechatPayAccountOptions")
            .field("id", &self.id)
            .field("mch_id", &self.mch_id)
            .field("app_id", &self.app_id)
            .field("serial_no", &self.serial_no)
            .field("private_key_pem", &"<redacted>")
            .field("api_v3_key", &"<redacted>")
            .field("notify_url", &self.notify_url)
            .field("key_mode", &self.key_mode)
            .field("live", &self.live)
            .finish()
    }
}

/// WeChat Pay adapter options.
///
/// Bind from configuration with `HostBuilder::add_wechat_pay()`, which binds the
/// `"WeChatPay"` section through an options monitor, or build in code with
/// [`WechatPayOptions::new`] and register it with `add_wechat_pay_with(options)`.
/// [`WechatPayOptions::with_account`] adds more merchant accounts (multi-account
/// mode); every request then selects an account through its [`PaymentScope`](webx_pay::PaymentScope),
/// and callbacks use the per-account route segment.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct WechatPayOptions {
    /// Merchant accounts; at least one is required.
    pub accounts: Vec<WechatPayAccountOptions>,
    /// Account used when a request does not select one (defaults to the first).
    #[serde(default)]
    pub default_account: Option<String>,
    /// API origin (override for tests/proxies).
    pub api_base: String,
    /// Response timeout in seconds.
    pub timeout_seconds: u64,
    /// TCP/TLS connect timeout in seconds.
    pub connect_timeout_seconds: u64,
    /// Allow loopback/intranet endpoints (tests only; production stays `false`).
    pub allow_private_networks: bool,
}

impl Default for WechatPayOptions {
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

impl WechatPayOptions {
    /// Single-account options.
    pub fn new(account: WechatPayAccountOptions) -> Self {
        Self {
            accounts: vec![account],
            ..Self::default()
        }
    }

    /// Options with no account (bind from configuration, then populate
    /// `accounts`).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Add a merchant account.
    pub fn with_account(mut self, account: WechatPayAccountOptions) -> Self {
        self.accounts.push(account);
        self
    }

    /// Select the default account.
    pub fn with_default_account(mut self, id: impl Into<String>) -> Self {
        self.default_account = Some(id.into());
        self
    }

    /// Override the API origin.
    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = base.into();
        self
    }

    /// Response timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout_seconds = timeout.as_secs().max(1);
        self
    }

    /// TCP/TLS connect timeout.
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout_seconds = timeout.as_secs().max(1);
        self
    }

    /// Allow loopback/intranet destinations (tests only).
    pub fn allow_private_networks(mut self, allow: bool) -> Self {
        self.allow_private_networks = allow;
        self
    }

    /// Static validation: accounts, key material, APIv3 key length, HTTPS base.
    ///
    /// This is the [`IPaymentProvider::validate`](webx_pay::IPaymentProvider)
    /// contract without the network: startup fails fast on a bad PEM, a
    /// mistyped APIv3 key or a duplicate account.
    pub fn validate(&self) -> Result<(), String> {
        if self.accounts.is_empty() {
            return Err("wechatpay requires at least one account".to_string());
        }
        if !(self.api_base.starts_with("https://")
            || (self.allow_private_networks && self.api_base.starts_with("http://")))
        {
            return Err(
                "wechatpay api_base must be an https URL (http is allowed only with allow_private_networks)"
                    .to_string(),
            );
        }
        if self.timeout_seconds == 0 || self.connect_timeout_seconds == 0 {
            return Err("wechatpay timeouts must be at least one second".to_string());
        }
        let mut seen = std::collections::BTreeSet::new();
        for account in &self.accounts {
            account.validate()?;
            if !seen.insert(account.id.clone()) {
                return Err(format!("duplicate wechatpay account id `{}`", account.id));
            }
        }
        if let Some(default) = &self.default_account {
            if !seen.contains(default) {
                return Err(format!(
                    "wechatpay default account `{default}` is not configured"
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn build_accounts(&self) -> Result<AccountSet<WechatPayAccount>, String> {
        self.validate()?;
        let mut converted = Vec::with_capacity(self.accounts.len());
        for account in &self.accounts {
            let credentials = account.clone().into_account()?;
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

    pub(crate) fn http_options(&self) -> HttpClientOptions {
        let base_url = self.api_base.trim_end_matches('/').to_string();
        HttpClientOptions {
            base_url,
            connect_timeout: Duration::from_secs(self.connect_timeout_seconds.max(1)),
            request_timeout: Some(Duration::from_secs(self.timeout_seconds.max(1))),
            read_timeout: Some(Duration::from_secs(self.timeout_seconds.max(1))),
            allow_private_networks: self.allow_private_networks,
            user_agent: format!("rust-webx-pay-wechatpay/{}", env!("CARGO_PKG_VERSION")),
            ..HttpClientOptions::default()
        }
    }
}

impl fmt::Debug for WechatPayOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WechatPayOptions")
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
    use crate::test_support;

    fn account() -> WechatPayAccountOptions {
        test_support::account_options()
    }

    #[test]
    fn single_account_uses_the_default_id_and_live_classification() {
        let options = WechatPayOptions::new(account());
        let accounts = options.build_accounts().unwrap();
        assert_eq!(accounts.default_id().as_str(), AccountId::DEFAULT);
        assert_eq!(accounts.accounts().len(), 1);
        assert!(accounts.accounts()[0].live);
        assert_eq!(accounts.accounts()[0].provider, PROVIDER);
    }

    #[test]
    fn live_classification_is_overridable() {
        let options = WechatPayOptions::new(account().with_live(false));
        let accounts = options.build_accounts().unwrap();
        assert!(!accounts.accounts()[0].live);
    }

    #[test]
    fn api_v3_key_must_be_exactly_32_bytes() {
        let mut bad = account();
        bad.api_v3_key = "too-short".to_string();
        assert!(bad.validate().is_err());
        let mut bad = account();
        bad.api_v3_key.push('x');
        assert!(bad.validate().is_err());
        assert!(account().validate().is_ok());
    }

    #[test]
    fn invalid_pem_key_and_public_key_are_configuration_errors() {
        let mut bad = account();
        bad.private_key_pem = "not a pem".to_string();
        assert!(bad.validate().is_err());
        let bad = account().with_public_key("PUB_KEY_ID_TEST", "not a pem");
        assert!(bad.validate().is_err());
        let bad = account().with_public_key("", test_support::key_pair().public_pem.clone());
        assert!(bad.validate().is_err());
        let good = account().with_public_key(
            "PUB_KEY_ID_TEST",
            test_support::key_pair().public_pem.clone(),
        );
        assert!(good.validate().is_ok());
    }

    #[test]
    fn merchant_ids_and_serials_are_checked() {
        let mut bad = account();
        bad.mch_id = String::new();
        assert!(bad.validate().is_err());
        let mut bad = account();
        bad.serial_no = "not-hex".to_string();
        assert!(bad.validate().is_err());
        let mut bad = account();
        bad.notify_url = "http://example.com/hook".to_string();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn empty_or_plaintext_http_base_is_a_configuration_error() {
        assert!(WechatPayOptions::empty().build_accounts().is_err());
        let options = WechatPayOptions::new(account()).with_api_base("http://api.example.com");
        assert!(options.build_accounts().is_err());
        assert!(options
            .allow_private_networks(true)
            .build_accounts()
            .is_ok());
    }

    #[test]
    fn unknown_default_account_and_duplicate_ids_are_errors() {
        let options = WechatPayOptions::new(account()).with_default_account("missing");
        assert!(options.build_accounts().is_err());
        let options = WechatPayOptions::new(account()).with_account(account());
        assert!(options.build_accounts().is_err());
        let second = account().with_id("second");
        let options = WechatPayOptions::new(account())
            .with_account(second)
            .with_default_account("second");
        let accounts = options.build_accounts().unwrap();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts.default_id().as_str(), "second");
    }

    #[test]
    fn public_key_mode_is_selected_through_account_options() {
        let public = test_support::key_pair().public_pem.clone();
        let account = account().with_public_key("PUB_KEY_ID_TEST", public);
        assert!(matches!(
            account.key_mode,
            WechatPayKeyMode::PublicKey { .. }
        ));
        let options = WechatPayOptions::new(account);
        let accounts = options.build_accounts().unwrap();
        assert_eq!(accounts.len(), 1);
    }

    #[test]
    fn debug_never_prints_secrets() {
        let options = WechatPayOptions::new(account());
        let text = format!("{options:?}");
        assert!(!text.contains(&test_support::key_pair().private_pem));
        assert!(!text.contains(&test_support::api_v3_key_text()));
        assert!(text.contains("<redacted>"));
        let account_text = format!("{:?}", account());
        assert!(!account_text.contains(&test_support::key_pair().private_pem));
        assert!(account_text.contains("<redacted>"));
    }

    #[test]
    fn http_options_normalize_the_base_url_and_timeouts() {
        let options = WechatPayOptions::new(account())
            .with_api_base("https://api.mch.weixin.qq.com/")
            .with_timeout(Duration::from_secs(7))
            .with_connect_timeout(Duration::from_secs(2));
        let http = options.http_options();
        assert_eq!(http.base_url, "https://api.mch.weixin.qq.com");
        assert_eq!(http.request_timeout, Some(Duration::from_secs(7)));
        assert_eq!(http.connect_timeout, Duration::from_secs(2));
        assert!(http.user_agent.starts_with("rust-webx-pay-wechatpay/"));
    }
}
