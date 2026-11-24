//! OAuth 2.0 authorization-code client (social login).

use std::fmt;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use url::Url;
use webx_client::{HttpClient, HttpClientOptions, IHttpClient, OutboundRequest};
use webx_core::auth::IdentityUser;
use webx_core::error::{Error, Result};

use crate::provider::{ClientAuth, OAuth2Provider, TokenFormat, TokenRequest, UserInfoAuth};

const MAX_BODY_BYTES: usize = 64 * 1024;

/// Client credentials and redirect settings.
#[derive(Clone)]
pub struct OAuth2Config {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_uri: String,
    /// Enable PKCE S256 when the provider supports it.
    pub pkce: bool,
    /// Allow private/loopback token/userinfo endpoints (tests/self-hosted).
    pub allow_private_networks: bool,
}

impl fmt::Debug for OAuth2Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuth2Config")
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<redacted>"),
            )
            .field("redirect_uri", &self.redirect_uri)
            .field("pkce", &self.pkce)
            .finish()
    }
}

impl OAuth2Config {
    pub fn new(
        client_id: impl Into<String>,
        client_secret: Option<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret,
            redirect_uri: redirect_uri.into(),
            pkce: true,
            allow_private_networks: false,
        }
    }

    pub fn without_pkce(mut self) -> Self {
        self.pkce = false;
        self
    }

    pub fn allow_private_networks(mut self, allow: bool) -> Self {
        self.allow_private_networks = allow;
        self
    }
}

/// Authorization URL plus the server-side state/PKCE values to persist.
#[derive(Debug, Clone)]
pub struct OAuth2Authorization {
    pub url: String,
    pub state: String,
    pub pkce_verifier: Option<String>,
}

/// Token endpoint result.
#[derive(Clone)]
pub struct OAuth2Token {
    pub access_token: String,
    pub token_type: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    /// Provider-specific subject carried by the token (WeChat `openid`, etc.).
    pub subject: Option<String>,
    pub raw: Value,
}

impl fmt::Debug for OAuth2Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuth2Token")
            .field("access_token", &"<redacted>")
            .field("token_type", &self.token_type)
            .field("expires_in", &self.expires_in)
            .field("subject", &self.subject)
            .finish()
    }
}

/// Normalized social identity.
#[derive(Debug, Clone)]
pub struct OAuth2User {
    pub subject: String,
    pub name: String,
    pub email: Option<String>,
    pub avatar_url: Option<String>,
    pub raw: Value,
}

impl OAuth2User {
    /// Map to the framework identity model. Roles/permissions are assigned by
    /// the application's account-linking policy, not by the social provider.
    pub fn into_identity(self) -> IdentityUser {
        IdentityUser {
            id: self.subject,
            name: self.name,
            email: self.email.unwrap_or_default(),
            tenant_id: String::new(),
            roles: Vec::new(),
            permissions: Vec::new(),
        }
    }
}

/// OAuth 2.0 client bound to one provider/config.
pub struct OAuth2Client {
    provider: OAuth2Provider,
    config: OAuth2Config,
    http: HttpClient,
}

impl fmt::Debug for OAuth2Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuth2Client")
            .field("provider", &self.provider.name)
            .field("config", &self.config)
            .finish()
    }
}

impl OAuth2Client {
    pub fn new(provider: OAuth2Provider, config: OAuth2Config) -> Result<Self> {
        if config.client_id.trim().is_empty() {
            return Err(Error::di("OAuth2 client_id must not be empty".to_string()));
        }
        if config.redirect_uri.trim().is_empty() {
            return Err(Error::di(
                "OAuth2 redirect_uri must not be empty".to_string(),
            ));
        }
        if provider.userinfo_request == crate::provider::UserInfoRequest::JsonPost
            && provider.userinfo_body.is_empty()
        {
            return Err(Error::di(
                "OAuth2 JsonPost userinfo requires a body template".to_string(),
            ));
        }
        for endpoint in [
            &provider.authorization_endpoint,
            &provider.token_endpoint,
            &provider.userinfo_endpoint,
        ] {
            let secure = endpoint.starts_with("https://")
                || (config.allow_private_networks
                    && (endpoint.starts_with("http://127.0.0.1")
                        || endpoint.starts_with("http://localhost")));
            if !secure {
                return Err(Error::di(format!(
                    "OAuth2 endpoint must be https:// (got '{endpoint}')"
                )));
            }
        }
        let mut options = HttpClientOptions::new(provider.token_endpoint.clone());
        options.connect_timeout = Duration::from_secs(3);
        options.request_timeout = Some(Duration::from_secs(20));
        options.allow_private_networks = config.allow_private_networks;
        let http = HttpClient::new("oauth2", options)
            .map_err(|error| Error::di(format!("OAuth2 HTTP client: {error}")))?;
        Ok(Self {
            provider,
            config,
            http,
        })
    }

    pub fn provider(&self) -> &OAuth2Provider {
        &self.provider
    }

    /// Build the authorization URL and generate state + PKCE if enabled.
    pub fn authorization_url(&self, state: impl Into<String>) -> Result<OAuth2Authorization> {
        let state = state.into();
        if state.trim().is_empty() {
            return Err(Error::validation(
                "OAuth2 state must not be empty".to_string(),
            ));
        }
        let pkce = (self.provider.pkce && self.config.pkce).then(Pkce::generate);
        let mut url = Url::parse(&self.provider.authorization_endpoint)
            .map_err(|error| Error::di(format!("invalid authorization endpoint: {error}")))?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair(
                &self.provider.authorize_client_id_key,
                &self.config.client_id,
            );
            query.append_pair("response_type", "code");
            query.append_pair("redirect_uri", &self.config.redirect_uri);
            if !self.provider.scopes.is_empty() {
                query.append_pair("scope", &self.provider.scopes.join(" "));
            }
            query.append_pair("state", &state);
            for (key, value) in &self.provider.authorize_extra {
                query.append_pair(key, value);
            }
            if let Some(pkce) = &pkce {
                query.append_pair("code_challenge", &pkce.challenge());
                query.append_pair("code_challenge_method", "S256");
            }
        }
        let mut url = url.to_string();
        if self.provider.name == "wechat" {
            url.push_str("#wechat_redirect");
        }
        Ok(OAuth2Authorization {
            url,
            state,
            pkce_verifier: pkce.map(|pkce| pkce.verifier().to_string()),
        })
    }

    /// Exchange an authorization code.
    pub async fn exchange_code(
        &self,
        code: &str,
        pkce_verifier: Option<&str>,
    ) -> Result<OAuth2Token> {
        if code.trim().is_empty() {
            return Err(Error::validation(
                "OAuth2 authorization code must not be empty".to_string(),
            ));
        }
        if self.provider.pkce && self.config.pkce && pkce_verifier.is_none() {
            return Err(Error::validation(
                "OAuth2 PKCE code_verifier is required".to_string(),
            ));
        }
        let fields = &self.provider.token_fields;
        let mut params: Vec<(String, String)> = vec![
            (fields.client_id_key.clone(), self.config.client_id.clone()),
            (fields.code_key.clone(), code.to_string()),
            (fields.grant_type_key.clone(), fields.grant_type.clone()),
        ];
        if let Some(secret) = &self.config.client_secret {
            params.push((fields.client_secret_key.clone(), secret.clone()));
        }
        if let Some(key) = &fields.redirect_uri_key {
            params.push((key.clone(), self.config.redirect_uri.clone()));
        }
        if let Some(verifier) = pkce_verifier {
            params.push(("code_verifier".to_string(), verifier.to_string()));
        }
        params.extend(self.provider.token_extra.iter().cloned());

        let mut headers = vec![("accept".to_string(), "application/json".to_string())];
        if self.provider.client_auth == ClientAuth::Basic {
            let Some(secret) = &self.config.client_secret else {
                return Err(Error::di(
                    "OAuth2 provider requires a client_secret for Basic auth".to_string(),
                ));
            };
            let raw = format!("{}:{}", self.config.client_id, secret);
            let encoded = base64::engine::general_purpose::STANDARD.encode(raw);
            headers.push(("authorization".to_string(), format!("Basic {encoded}")));
            params.retain(|(key, _)| {
                key != &fields.client_id_key && key != &fields.client_secret_key
            });
        }
        let auth_header = headers
            .iter()
            .find(|(key, _)| key == "authorization")
            .map(|(_, value)| value.clone());

        let (status, body) = match self.provider.token_request {
            TokenRequest::FormPost => {
                let form = form_encode(&params);
                let mut request = OutboundRequest::post(&self.provider.token_endpoint)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("accept", "application/json")
                    .body(form);
                if let Some(value) = &auth_header {
                    request = request.header("authorization", value);
                }
                self.send(request).await?
            }
            TokenRequest::JsonPost => {
                let map: Map<String, Value> = params
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                    .collect();
                let body = serde_json::to_vec(&Value::Object(map))
                    .map_err(|error| Error::internal(format!("OAuth2 JSON: {error}")))?;
                let mut request = OutboundRequest::post(&self.provider.token_endpoint)
                    .header("content-type", "application/json")
                    .header("accept", "application/json")
                    .body(body);
                if let Some(value) = &auth_header {
                    request = request.header("authorization", value);
                }
                self.send(request).await?
            }
            TokenRequest::QueryGet => {
                let mut url = Url::parse(&self.provider.token_endpoint)
                    .map_err(|error| Error::di(format!("invalid token endpoint: {error}")))?;
                url.query_pairs_mut().extend_pairs(&params);
                let mut request =
                    OutboundRequest::get(url.as_str()).header("accept", "application/json");
                if let Some(value) = &auth_header {
                    request = request.header("authorization", value);
                }
                self.send(request).await?
            }
        };
        if !(200..300).contains(&status) {
            return Err(if status >= 500 {
                Error::status(503, format!("OAuth2 token endpoint HTTP {status}"))
            } else {
                Error::unauthorized("OAuth2 token endpoint rejected the code".to_string())
            });
        }
        let value = match self.provider.token_format {
            TokenFormat::Json => serde_json::from_slice(&body).map_err(|error| {
                Error::status(502, format!("invalid OAuth2 token JSON: {error}"))
            })?,
            TokenFormat::Form => form_to_json(&body),
        };
        let access_token = pointer(&value, &fields.access_token)
            .and_then(scalar_to_string)
            .ok_or_else(|| Error::status(502, "OAuth2 token response has no access_token"))?;
        let mut token = OAuth2Token {
            access_token,
            token_type: fields
                .token_type
                .as_ref()
                .and_then(|pointer| pointer_at(&value, pointer))
                .and_then(scalar_to_string)
                .unwrap_or_else(|| "Bearer".to_string()),
            refresh_token: fields
                .refresh_token
                .as_ref()
                .and_then(|pointer| pointer_at(&value, pointer))
                .and_then(scalar_to_string),
            expires_in: fields
                .expires_in
                .as_ref()
                .and_then(|pointer| pointer_at(&value, pointer))
                .and_then(|value| value.as_u64()),
            subject: fields
                .subject
                .as_ref()
                .and_then(|pointer| pointer_at(&value, pointer))
                .and_then(scalar_to_string),
            raw: value,
        };
        if token.subject.is_none() {
            if let Some(endpoint) = &self.provider.subject_endpoint {
                token.subject = self.fetch_subject(endpoint, &token.access_token).await?;
            }
        }
        Ok(token)
    }

    async fn fetch_subject(&self, endpoint: &str, access_token: &str) -> Result<Option<String>> {
        let mut url = Url::parse(endpoint)
            .map_err(|error| Error::di(format!("invalid subject endpoint: {error}")))?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("access_token", access_token);
            for (key, value) in &self.provider.subject_endpoint_extra {
                query.append_pair(key, &self.expand(value, access_token, None));
            }
        }
        let (status, body) = self
            .send(OutboundRequest::get(url.as_str()).header("accept", "application/json"))
            .await?;
        if !(200..300).contains(&status) {
            return Ok(None);
        }
        let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        Ok(pointer_at(&value, &self.provider.subject_endpoint_pointer).and_then(scalar_to_string))
    }

    fn expand(&self, template: &str, access_token: &str, subject: Option<&str>) -> String {
        template
            .replace("{access_token}", access_token)
            .replace("{subject}", subject.unwrap_or_default())
            .replace("{client_id}", &self.config.client_id)
    }

    /// Fetch and normalize the provider's user profile.
    pub async fn userinfo(&self, token: &OAuth2Token) -> Result<OAuth2User> {
        let subject_hint = match token.subject.clone() {
            Some(subject) => Some(subject),
            None => match &self.provider.subject_endpoint {
                Some(endpoint) => self.fetch_subject(endpoint, &token.access_token).await?,
                None => None,
            },
        };
        let mut url = Url::parse(&self.provider.userinfo_endpoint)
            .map_err(|error| Error::di(format!("invalid userinfo endpoint: {error}")))?;
        match self.provider.userinfo_auth {
            UserInfoAuth::Bearer => {
                if !self.provider.userinfo_query.is_empty() {
                    let mut query = url.query_pairs_mut();
                    for (key, value) in &self.provider.userinfo_query {
                        query.append_pair(
                            key,
                            &self.expand(value, &token.access_token, subject_hint.as_deref()),
                        );
                    }
                }
            }
            UserInfoAuth::Query => {
                let mut query = url.query_pairs_mut();
                query.append_pair("access_token", &token.access_token);
                if let (Some(key), Some(subject)) =
                    (&self.provider.userinfo_subject_key, subject_hint.as_deref())
                {
                    query.append_pair(key, subject);
                }
                for (key, value) in &self.provider.userinfo_query {
                    query.append_pair(
                        key,
                        &self.expand(value, &token.access_token, subject_hint.as_deref()),
                    );
                }
            }
        }
        let mut request = match self.provider.userinfo_request {
            crate::provider::UserInfoRequest::Get => {
                OutboundRequest::get(url.as_str()).header("accept", "application/json")
            }
            crate::provider::UserInfoRequest::JsonPost => {
                let body: Map<String, Value> = self
                    .provider
                    .userinfo_body
                    .iter()
                    .map(|(key, value)| {
                        (
                            key.clone(),
                            Value::String(self.expand(
                                value,
                                &token.access_token,
                                subject_hint.as_deref(),
                            )),
                        )
                    })
                    .collect();
                let body = serde_json::to_vec(&Value::Object(body))
                    .map_err(|error| Error::internal(format!("OAuth2 userinfo JSON: {error}")))?;
                OutboundRequest::post(url.as_str())
                    .header("accept", "application/json")
                    .header("content-type", "application/json")
                    .body(body)
            }
        };
        if self.provider.userinfo_auth == UserInfoAuth::Bearer {
            request = request.header("authorization", format!("Bearer {}", token.access_token));
        }
        let (status, body) = self.send(request).await?;
        if !(200..300).contains(&status) {
            return Err(Error::status(
                503,
                format!("OAuth2 userinfo endpoint HTTP {status}"),
            ));
        }
        let value: Value = serde_json::from_slice(&body).map_err(|error| {
            Error::status(502, format!("invalid OAuth2 userinfo JSON: {error}"))
        })?;
        let subject = self
            .subject(&value)
            .or(subject_hint)
            .ok_or_else(|| Error::unauthorized("OAuth2 userinfo has no subject".to_string()))?;
        let email = self
            .string_at(&value, self.provider.email_pointer.as_deref())
            .filter(|email| !email.is_empty());
        let email = match (email, &self.provider.email_endpoint) {
            (None, Some(endpoint)) => self.fetch_email(endpoint, &token.access_token).await?,
            (email, _) => email,
        };
        Ok(OAuth2User {
            subject,
            name: self
                .string_at(&value, self.provider.name_pointer.as_deref())
                .unwrap_or_default(),
            email,
            avatar_url: self.string_at(&value, self.provider.avatar_pointer.as_deref()),
            raw: value,
        })
    }

    async fn fetch_email(&self, endpoint: &str, access_token: &str) -> Result<Option<String>> {
        let (status, body) = self
            .send(
                OutboundRequest::get(endpoint)
                    .header("accept", "application/json")
                    .header("authorization", format!("Bearer {access_token}")),
            )
            .await?;
        if !(200..300).contains(&status) {
            return Ok(None);
        }
        let values: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let Some(items) = values.as_array() else {
            return Ok(None);
        };
        let preferred = items
            .iter()
            .find(|item| item.get("primary").and_then(Value::as_bool) == Some(true))
            .or_else(|| items.first());
        Ok(preferred
            .and_then(|item| pointer_at(item, &self.provider.email_list_pointer))
            .and_then(scalar_to_string)
            .filter(|email| !email.is_empty()))
    }

    fn subject(&self, value: &Value) -> Option<String> {
        std::iter::once(&self.provider.subject_pointer)
            .chain(self.provider.subject_fallbacks.iter())
            .find_map(|pointer| pointer_at(value, pointer).and_then(scalar_to_string))
    }

    fn string_at(&self, value: &Value, pointer: Option<&str>) -> Option<String> {
        pointer
            .and_then(|pointer| pointer_at(value, pointer))
            .and_then(scalar_to_string)
    }

    async fn send(&self, request: OutboundRequest) -> Result<(u16, Vec<u8>)> {
        let response = self
            .http
            .execute(request)
            .await
            .map_err(|error| Error::status(503, format!("OAuth2 transport failed: {error}")))?;
        let status = response.status;
        let body = response
            .body
            .collect(MAX_BODY_BYTES)
            .await
            .map_err(|error| Error::status(503, format!("OAuth2 body failed: {error}")))?;
        Ok((status, body.to_vec()))
    }
}

struct Pkce {
    verifier: String,
}

impl Pkce {
    fn generate() -> Self {
        let mut raw = Vec::with_capacity(32);
        raw.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
        raw.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
        Self {
            verifier: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw),
        }
    }

    fn verifier(&self) -> &str {
        &self.verifier
    }

    fn challenge(&self) -> String {
        let digest = Sha256::digest(self.verifier.as_bytes());
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
    }
}

fn pointer<'a>(value: &'a Value, pointer: &str) -> Option<&'a Value> {
    value.pointer(pointer)
}

fn pointer_at<'a>(value: &'a Value, pointer: &str) -> Option<&'a Value> {
    value.pointer(pointer)
}

fn scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn form_encode(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                url::form_urlencoded::byte_serialize(key.as_bytes()).collect::<String>(),
                url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn form_to_json(body: &[u8]) -> Value {
    let mut map = Map::new();
    for (key, value) in url::form_urlencoded::parse(body) {
        map.insert(key.into_owned(), Value::String(value.into_owned()));
    }
    Value::Object(map)
}
