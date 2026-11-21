//! Alipay OAuth 2.0 (signed `openapi.alipay.com` gateway).
//!
//! Alipay is not a plain OAuth 2.0 token endpoint: `alipay.system.oauth.token`
//! and `alipay.user.info.share` are RSA2-signed gateway requests. This module
//! keeps that protocol isolated from [`OAuth2Client`](crate::OAuth2Client),
//! while exposing the same normalized [`OAuth2User`].
//!
//! Required app configuration:
//! - `app_id`
//! - application private key (PKCS#8 PEM recommended; PKCS#1 accepted)
//! - Alipay public key (used to verify every gateway response)
//! - registered redirect URI

use std::fmt;
use std::time::Duration;

use base64::Engine as _;
use rsa::pkcs1::{DecodeRsaPrivateKey, DecodeRsaPublicKey};
use rsa::pkcs1v15::{SigningKey, VerifyingKey};
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use rsa::sha2::Sha256;
use rsa::signature::{SignatureEncoding, Signer, Verifier};
use rsa::{PrivateKey, PublicKey};
use serde_json::{Map, Value};
use url::Url;
use webx_client::{HttpClient, HttpClientOptions, IHttpClient, OutboundRequest};
use webx_core::auth::IdentityUser;
use webx_core::error::{Error, Result};

use crate::client::OAuth2User;

const AUTHORIZE_ENDPOINT: &str = "https://openauth.alipay.com/oauth2/publicAppAuthorize.htm";
const GATEWAY: &str = "https://openapi.alipay.com/gateway.do";
const SANDBOX_GATEWAY: &str = "https://openapi-sandbox.dl.alipaydev.com/gateway.do";
const MAX_BODY_BYTES: usize = 64 * 1024;

/// Alipay application credentials and endpoints.
#[derive(Clone)]
pub struct AlipayConfig {
    pub app_id: String,
    pub redirect_uri: String,
    pub gateway: String,
    pub authorize_endpoint: String,
    pub private_key_pem: String,
    pub alipay_public_key_pem: String,
    pub allow_private_networks: bool,
}

impl fmt::Debug for AlipayConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AlipayConfig")
            .field("app_id", &self.app_id)
            .field("redirect_uri", &self.redirect_uri)
            .field("gateway", &self.gateway)
            .field("private_key_pem", &"<redacted>")
            .field("alipay_public_key_pem", &"<redacted>")
            .finish()
    }
}

impl AlipayConfig {
    pub fn new(
        app_id: impl Into<String>,
        private_key_pem: impl Into<String>,
        alipay_public_key_pem: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        Self {
            app_id: app_id.into(),
            redirect_uri: redirect_uri.into(),
            gateway: GATEWAY.to_string(),
            authorize_endpoint: AUTHORIZE_ENDPOINT.to_string(),
            private_key_pem: private_key_pem.into(),
            alipay_public_key_pem: alipay_public_key_pem.into(),
            allow_private_networks: false,
        }
    }

    /// Use the Alipay sandbox gateway (still requires sandbox keys).
    pub fn sandbox(mut self) -> Self {
        self.gateway = SANDBOX_GATEWAY.to_string();
        self
    }

    pub fn allow_private_networks(mut self, allow: bool) -> Self {
        self.allow_private_networks = allow;
        self
    }
}

/// Successful `alipay.system.oauth.token` result.
#[derive(Clone)]
pub struct AlipayToken {
    pub access_token: String,
    pub user_id: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub raw: Value,
}

impl fmt::Debug for AlipayToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AlipayToken")
            .field("access_token", &"<redacted>")
            .field("user_id", &self.user_id)
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

/// Signed Alipay OAuth client.
pub struct AlipayClient {
    config: AlipayConfig,
    http: HttpClient,
    signing: SigningKey<Sha256>,
    verifying: VerifyingKey<Sha256>,
}

impl fmt::Debug for AlipayClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AlipayClient")
            .field("config", &self.config)
            .finish()
    }
}

impl AlipayClient {
    pub fn new(config: AlipayConfig) -> Result<Self> {
        if config.app_id.trim().is_empty() {
            return Err(Error::Di("Alipay app_id must not be empty".to_string()));
        }
        if config.redirect_uri.trim().is_empty() {
            return Err(Error::Di(
                "Alipay redirect_uri must not be empty".to_string(),
            ));
        }
        for endpoint in [&config.gateway, &config.authorize_endpoint] {
            let secure = endpoint.starts_with("https://")
                || (config.allow_private_networks
                    && (endpoint.starts_with("http://127.0.0.1")
                        || endpoint.starts_with("http://localhost")));
            if !secure {
                return Err(Error::Di(format!(
                    "Alipay endpoint must be https:// (got '{endpoint}')"
                )));
            }
        }
        let private = parse_private_key(&config.private_key_pem)?;
        let public = parse_public_key(&config.alipay_public_key_pem)?;
        let mut options = HttpClientOptions::new(config.gateway.clone());
        options.connect_timeout = Duration::from_secs(3);
        options.request_timeout = Some(Duration::from_secs(20));
        options.allow_private_networks = config.allow_private_networks;
        let http = HttpClient::new("alipay", options)
            .map_err(|error| Error::Di(format!("Alipay HTTP client: {error}")))?;
        Ok(Self {
            config,
            http,
            signing: SigningKey::<Sha256>::new(private),
            verifying: VerifyingKey::<Sha256>::new(public),
        })
    }

    /// Build the user-authorization URL.
    pub fn authorization_url(&self, state: impl Into<String>) -> Result<String> {
        let state = state.into();
        if state.trim().is_empty() {
            return Err(Error::Validation(
                "Alipay state must not be empty".to_string(),
            ));
        }
        let mut url = Url::parse(&self.config.authorize_endpoint)
            .map_err(|error| Error::Di(format!("invalid Alipay authorize URL: {error}")))?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("app_id", &self.config.app_id);
            query.append_pair("scope", "auth_user");
            query.append_pair("redirect_uri", &self.config.redirect_uri);
            query.append_pair("state", &state);
        }
        Ok(url.to_string())
    }

    /// Exchange an authorization code with `alipay.system.oauth.token`.
    pub async fn exchange_code(&self, code: &str) -> Result<AlipayToken> {
        if code.trim().is_empty() {
            return Err(Error::Validation(
                "Alipay authorization code must not be empty".to_string(),
            ));
        }
        let value = self
            .call(
                "alipay.system.oauth.token",
                vec![
                    ("grant_type".to_string(), "authorization_code".to_string()),
                    ("code".to_string(), code.to_string()),
                ],
            )
            .await?;
        let response = response_object(&value, "alipay_system_oauth_token_response")?;
        let access_token = string_field(response, "access_token")
            .ok_or_else(|| Error::Status(502, "Alipay token has no access_token".to_string()))?;
        let user_id = string_field(response, "user_id")
            .ok_or_else(|| Error::Status(502, "Alipay token has no user_id".to_string()))?;
        Ok(AlipayToken {
            access_token,
            user_id,
            refresh_token: string_field(response, "refresh_token"),
            expires_in: response.get("expires_in").and_then(Value::as_u64),
            raw: value,
        })
    }

    /// Fetch `alipay.user.info.share` and normalize it.
    pub async fn userinfo(&self, token: &AlipayToken) -> Result<OAuth2User> {
        let value = self
            .call(
                "alipay.user.info.share",
                vec![("auth_token".to_string(), token.access_token.clone())],
            )
            .await?;
        let response = response_object(&value, "alipay_user_info_share_response")?;
        let user_id = string_field(response, "user_id").unwrap_or_else(|| token.user_id.clone());
        if user_id.trim().is_empty() {
            return Err(Error::Unauthorized(
                "Alipay userinfo has no user_id".to_string(),
            ));
        }
        Ok(OAuth2User {
            subject: user_id,
            name: string_field(response, "nick_name").unwrap_or_default(),
            email: None,
            avatar_url: string_field(response, "avatar"),
            raw: value,
        })
    }

    /// Convenience: profile mapped to the framework identity.
    pub async fn identity(&self, token: &AlipayToken) -> Result<IdentityUser> {
        Ok(self.userinfo(token).await?.into_identity())
    }

    async fn call(&self, method: &str, extra: Vec<(String, String)>) -> Result<Value> {
        let mut params = vec![
            ("app_id".to_string(), self.config.app_id.clone()),
            ("method".to_string(), method.to_string()),
            ("format".to_string(), "JSON".to_string()),
            ("charset".to_string(), "utf-8".to_string()),
            ("sign_type".to_string(), "RSA2".to_string()),
            ("timestamp".to_string(), alipay_timestamp()),
            ("version".to_string(), "1.0".to_string()),
        ];
        params.extend(extra);
        let sign = self.sign(&params)?;
        params.push(("sign".to_string(), sign));
        let form = params
            .iter()
            .map(|(key, value)| {
                format!(
                    "{}={}",
                    url::form_urlencoded::byte_serialize(key.as_bytes()).collect::<String>(),
                    url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>()
                )
            })
            .collect::<Vec<_>>()
            .join("&");
        let response = self
            .http
            .execute(
                OutboundRequest::post(&self.config.gateway)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("accept", "application/json")
                    .body(form),
            )
            .await
            .map_err(|error| Error::Status(503, format!("Alipay transport: {error}")))?;
        let status = response.status;
        let body = response
            .body
            .collect(MAX_BODY_BYTES)
            .await
            .map_err(|error| Error::Status(503, format!("Alipay body: {error}")))?;
        if !(200..300).contains(&status) {
            return Err(Error::Status(503, format!("Alipay gateway HTTP {status}")));
        }
        let text = String::from_utf8(body.to_vec())
            .map_err(|error| Error::Status(502, format!("Alipay response not UTF-8: {error}")))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|error| Error::Status(502, format!("Alipay response JSON: {error}")))?;
        self.verify_response(&text, &value)?;
        Ok(value)
    }

    fn sign(&self, params: &[(String, String)]) -> Result<String> {
        let mut items: Vec<&(String, String)> = params
            .iter()
            .filter(|(key, value)| key != "sign" && !value.is_empty())
            .collect();
        items.sort_by(|left, right| left.0.cmp(&right.0));
        let content = items
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&");
        let signature = self.signing.sign(content.as_bytes());
        Ok(base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()))
    }

    fn verify_response(&self, body: &str, value: &Value) -> Result<()> {
        let Some(signature) = value.get("sign").and_then(Value::as_str) else {
            return Err(Error::Status(
                502,
                "Alipay response is missing a signature".to_string(),
            ));
        };
        let Some(index) = body.rfind(",\"sign\"") else {
            return Err(Error::Status(
                502,
                "Alipay response sign is not the last field".to_string(),
            ));
        };
        let content = &body[..index];
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(signature)
            .map_err(|_| Error::Status(502, "Alipay response sign is not base64".to_string()))?;
        let signature = rsa::pkcs1v15::Signature::try_from(bytes.as_slice())
            .map_err(|_| Error::Status(502, "Alipay response sign is malformed".to_string()))?;
        self.verifying
            .verify(content.as_bytes(), &signature)
            .map_err(|_| Error::Status(502, "Alipay response signature mismatch".to_string()))
    }
}

fn response_object<'a>(value: &'a Value, key: &str) -> Result<&'a Map<String, Value>> {
    if let Some(error) = value.get("error_response").and_then(Value::as_object) {
        let code = string_field(error, "code").unwrap_or_else(|| "unknown".to_string());
        let message = string_field(error, "sub_msg")
            .or_else(|| string_field(error, "msg"))
            .unwrap_or_default();
        return Err(Error::Unauthorized(format!(
            "Alipay error {code}: {message}"
        )));
    }
    value
        .get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| Error::Status(502, format!("Alipay response has no {key}")))
}

fn string_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    match object.get(key) {
        Some(Value::String(value)) => Some(value.clone()),
        Some(Value::Number(value)) => Some(value.to_string()),
        _ => None,
    }
}

fn parse_private_key(pem: &str) -> Result<PrivateKey> {
    let pem = normalize_pem(pem, "PRIVATE KEY");
    PrivateKey::from_pkcs8_pem(&pem)
        .or_else(|_| PrivateKey::from_pkcs1_pem(&pem))
        .map_err(|error| Error::Di(format!("Alipay private key: {error}")))
}

fn parse_public_key(pem: &str) -> Result<PublicKey> {
    let pem = normalize_pem(pem, "PUBLIC KEY");
    PublicKey::from_public_key_pem(&pem)
        .or_else(|_| PublicKey::from_pkcs1_pem(&pem))
        .map_err(|error| Error::Di(format!("Alipay public key: {error}")))
}

/// Alipay consoles often show a raw base64 key without PEM armor.
fn normalize_pem(key: &str, label: &str) -> String {
    let key = key.trim();
    if key.contains("BEGIN ") {
        return key.to_string();
    }
    let body = key
        .as_bytes()
        .chunks(64)
        .map(|chunk| String::from_utf8_lossy(chunk).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    format!("-----BEGIN {label}-----\n{body}\n-----END {label}-----")
}

fn alipay_timestamp() -> String {
    // Alipay expects GMT+8, `yyyy-MM-dd HH:mm:ss`.
    let now = time::OffsetDateTime::now_utc() + time::Duration::hours(8);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::pkcs1v15::Signature;
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
    use rsa::signature::Verifier;
    use rsa::Generate;

    #[test]
    fn raw_base64_key_is_armored() {
        let armored = normalize_pem("abcd", "PRIVATE KEY");
        assert!(armored.starts_with("-----BEGIN PRIVATE KEY-----"));
        assert!(armored.ends_with("-----END PRIVATE KEY-----"));
    }

    #[test]
    fn timestamp_has_alipay_shape() {
        let timestamp = alipay_timestamp();
        assert_eq!(timestamp.len(), 19);
        assert_eq!(&timestamp[4..5], "-");
        assert_eq!(&timestamp[10..11], " ");
    }

    #[test]
    fn rsa2_sign_and_verify_roundtrip() {
        let private = PrivateKey::generate();
        let public = PublicKey::from(&private);
        let client = AlipayClient::new(AlipayConfig::new(
            "app",
            private
                .to_pkcs8_pem(LineEnding::LF)
                .expect("private pem")
                .to_string(),
            public
                .to_public_key_pem(LineEnding::LF)
                .expect("public pem"),
            "https://app.example.com/cb",
        ))
        .expect("client");
        let params = vec![
            ("app_id".to_string(), "app".to_string()),
            (
                "method".to_string(),
                "alipay.system.oauth.token".to_string(),
            ),
            ("charset".to_string(), "utf-8".to_string()),
        ];
        let signature = client.sign(&params).expect("sign");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(signature)
            .expect("base64");
        let signature = Signature::try_from(bytes.as_slice()).expect("signature");
        client
            .verifying
            .verify(
                b"app_id=app&charset=utf-8&method=alipay.system.oauth.token",
                &signature,
            )
            .expect("verify");
    }
}
