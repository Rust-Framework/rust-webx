//! WeChat Pay APIv3 adapter for rust-webx-pay.
//!
//! Implements the merchant ("直连商户") APIv3 protocol for Native/JSAPI/App/H5
//! payments, order query/close, refunds, platform-certificate or public-key
//! response verification and encrypted callback notifications.
//!
//! ```no_run
//! use webx::Host;
//! use webx_pay_wechatpay::WechatPayHostBuilderExt;
//!
//! // Runtime configuration from the "WeChatPay" section (hot-reloadable).
//! let host = Host::builder().add_wechat_pay().build();
//!
//! // Or a prebuilt static configuration:
//! // Host::builder()
//! //     .add_wechat_pay_with(WechatPayOptions::new(account))
//! //     .build();
//! ```
//!
//! # Protocol facts encoded here
//!
//! * Every request is signed with the merchant API certificate private key:
//!   `METHOD\nURL\nTIMESTAMP\nNONCE\nBODY\n` (five lines, trailing newline),
//!   RSA-SHA256/PKCS#1 v1.5, base64. The `Authorization` header uses the
//!   `WECHATPAY2-SHA256-RSA2048` scheme.
//! * Every response/notification signature is verified over
//!   `TIMESTAMP\nNONCE\nBODY\n` with the WeChat Pay platform key selected by the
//!   `Wechatpay-Serial` header: either a platform certificate downloaded from
//!   `GET /v3/certificates` (rotation-aware cache) or the configured WeChat Pay
//!   public key (`PUB_KEY_ID_...` mode).
//! * Callback and certificate resource payloads are `AEAD_AES_256_GCM` encrypted
//!   with the APIv3 key; the base64 ciphertext carries the 16-byte tag at the
//!   end (RFC 5116 / NIST GCM semantics).
//! * Money is always integer fen (`amount.total`), currency is fixed to CNY.
//!
//! # Frontend catalog
//!
//! [`IPaymentProvider::payment_options`](webx_pay::IPaymentProvider::payment_options)
//! reports one option per configured merchant account for each APIv3 channel:
//! `wechat_native` (`qr_code`), `wechat_jsapi` (`client_sdk`), `wechat_app`
//! (`app`) and `wechat_h5` (`redirect`). `client_config` carries only the public
//! `app_id`; the merchant private key, APIv3 key and certificate serial are
//! never serialized into the catalog. An operator label is copied when the
//! public account view has one.
//!
//! # Failure policy
//!
//! * A 2xx response without `Wechatpay-Signature` headers is rejected: the
//!   adapter never trusts unverified payment data.
//! * Error (non-2xx) responses are still parsed when a proxy stripped the
//!   signature headers, but they can only produce an error, never payment state.
//! * An unknown `Wechatpay-Serial` triggers one platform-certificate refresh
//!   (rate-limited to [`CERTIFICATE_REFRESH_INTERVAL`]) and then fails closed.
//! * The `GET /v3/certificates` download response itself is not signature
//!   verified: it is the bootstrap trust anchor and the official documentation
//!   exempts the platform-certificate download from response verification
//!   ("文件下载接口和首次下载平台证书除外"). TLS and the subsequent fail-closed
//!   verification of every business response are the trust boundary.

mod cert;
mod crypto;
mod mapping;
mod options;
mod provider;
mod sign;
mod time;
mod transport;
mod webhook;

pub use cert::CERTIFICATE_REFRESH_INTERVAL;
pub use crypto::{decrypt_aead_aes_256_gcm, AEAD_ALGORITHM, API_V3_KEY_LEN};
pub use mapping::{map_refund_status, map_trade_state};
pub use options::{WechatPayAccountOptions, WechatPayKeyMode, WechatPayOptions};
pub use provider::{
    WechatPayHostBuilderExt, WechatPayProvider, WechatPayProviderBuilder, WechatPayProviderFactory,
};
pub use sign::{
    app_client_signature_message, authorization_header, jsapi_client_signature_message,
    parse_private_key_pem, parse_public_key_pem, request_signature_message,
    response_signature_message, sign_message_base64, validate_timestamp, verify_message_base64,
    AUTHORIZATION_SCHEMA,
};
pub use webhook::{WechatPayWebhookVerifier, SIGNATURE_TOLERANCE_SECONDS};

/// Provider id owned by this adapter.
pub const PROVIDER: webx_pay::ProviderId = webx_pay::ProviderId::WechatPay;

/// WeChat Pay production API origin.
pub const DEFAULT_API_BASE: &str = "https://api.mch.weixin.qq.com";

/// Per-request JSON body cap for responses (certificate lists are well below).
pub(crate) const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

#[cfg(test)]
pub(crate) mod test_support;
