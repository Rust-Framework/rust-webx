//! Provider descriptors and presets.

/// How the token endpoint authenticates the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientAuth {
    /// `client_id` / `client_secret` in the request body.
    Post,
    /// HTTP Basic `client_id:client_secret`.
    Basic,
}

/// How the token request is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenRequest {
    FormPost,
    JsonPost,
    QueryGet,
}

/// Wire format of the token response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenFormat {
    Json,
    Form,
}

/// How the access token is sent to the userinfo endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserInfoAuth {
    Bearer,
    Query,
}

/// HTTP method/body used for the userinfo endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserInfoRequest {
    Get,
    JsonPost,
}

/// Provider-specific token request keys and response JSON pointers.
#[derive(Debug, Clone)]
pub struct TokenFields {
    pub client_id_key: String,
    pub client_secret_key: String,
    pub code_key: String,
    pub grant_type_key: String,
    pub grant_type: String,
    pub redirect_uri_key: Option<String>,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<String>,
    pub token_type: Option<String>,
    pub subject: Option<String>,
}

impl Default for TokenFields {
    fn default() -> Self {
        Self {
            client_id_key: "client_id".into(),
            client_secret_key: "client_secret".into(),
            code_key: "code".into(),
            grant_type_key: "grant_type".into(),
            grant_type: "authorization_code".into(),
            redirect_uri_key: Some("redirect_uri".into()),
            access_token: "/access_token".into(),
            refresh_token: Some("/refresh_token".into()),
            expires_in: Some("/expires_in".into()),
            token_type: Some("/token_type".into()),
            subject: None,
        }
    }
}

/// A concrete OAuth 2.0 provider.
#[derive(Debug, Clone)]
pub struct OAuth2Provider {
    pub name: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub userinfo_endpoint: String,
    pub scopes: Vec<String>,
    pub client_auth: ClientAuth,
    pub token_request: TokenRequest,
    pub token_format: TokenFormat,
    pub userinfo_auth: UserInfoAuth,
    pub userinfo_request: UserInfoRequest,
    /// JSON body template for `JsonPost` userinfo (`{access_token}` etc.).
    pub userinfo_body: Vec<(String, String)>,
    /// Query key for the token's subject on Query-auth userinfo (e.g. `openid`).
    pub userinfo_subject_key: Option<String>,
    /// Extra userinfo query pairs; `{subject}`, `{access_token}`, `{client_id}`.
    pub userinfo_query: Vec<(String, String)>,
    pub token_fields: TokenFields,
    /// Extra token request parameters (e.g. QQ `fmt=json`).
    pub token_extra: Vec<(String, String)>,
    /// Optional endpoint that resolves the subject from an access token
    /// (QQ `/oauth2.0/me`).
    pub subject_endpoint: Option<String>,
    pub subject_endpoint_extra: Vec<(String, String)>,
    /// JSON pointer for the subject resolved by `subject_endpoint`.
    pub subject_endpoint_pointer: String,
    pub authorize_client_id_key: String,
    pub authorize_extra: Vec<(String, String)>,
    /// JSON pointer for the stable subject in the userinfo document.
    pub subject_pointer: String,
    /// Fallbacks when `subject_pointer` is absent.
    pub subject_fallbacks: Vec<String>,
    pub name_pointer: Option<String>,
    pub email_pointer: Option<String>,
    pub avatar_pointer: Option<String>,
    /// Optional endpoint returning an array of emails (GitHub).
    pub email_endpoint: Option<String>,
    /// JSON pointer inside one email array item.
    pub email_list_pointer: String,
    /// Whether the provider supports PKCE S256.
    pub pkce: bool,
}

impl OAuth2Provider {
    /// Generic provider; use a preset or fill these fields from configuration.
    pub fn custom(
        name: impl Into<String>,
        authorization_endpoint: impl Into<String>,
        token_endpoint: impl Into<String>,
        userinfo_endpoint: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            authorization_endpoint: authorization_endpoint.into(),
            token_endpoint: token_endpoint.into(),
            userinfo_endpoint: userinfo_endpoint.into(),
            scopes: Vec::new(),
            client_auth: ClientAuth::Post,
            token_request: TokenRequest::FormPost,
            token_format: TokenFormat::Json,
            userinfo_auth: UserInfoAuth::Bearer,
            userinfo_request: UserInfoRequest::Get,
            userinfo_body: Vec::new(),
            userinfo_subject_key: None,
            userinfo_query: Vec::new(),
            subject_endpoint: None,
            subject_endpoint_extra: Vec::new(),
            subject_endpoint_pointer: "/openid".into(),
            token_extra: Vec::new(),
            token_fields: TokenFields::default(),
            authorize_client_id_key: "client_id".into(),
            authorize_extra: Vec::new(),
            subject_pointer: "/sub".into(),
            subject_fallbacks: Vec::new(),
            name_pointer: Some("/name".into()),
            email_pointer: Some("/email".into()),
            avatar_pointer: Some("/picture".into()),
            email_endpoint: None,
            email_list_pointer: "/email".into(),
            pkce: true,
        }
    }

    /// Replace the requested scopes.
    pub fn with_scopes<I, S>(mut self, scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.scopes = scopes.into_iter().map(Into::into).collect();
        self
    }

    /// Set userinfo JSON-pointer mapping.
    pub fn with_mapping(
        mut self,
        subject: impl Into<String>,
        name: Option<impl Into<String>>,
        email: Option<impl Into<String>>,
        avatar: Option<impl Into<String>>,
    ) -> Self {
        self.subject_pointer = subject.into();
        self.name_pointer = name.map(Into::into);
        self.email_pointer = email.map(Into::into);
        self.avatar_pointer = avatar.map(Into::into);
        self
    }

    /// GitHub-style `/user/emails` fallback.
    pub fn with_email_endpoint(
        mut self,
        endpoint: impl Into<String>,
        pointer: impl Into<String>,
    ) -> Self {
        self.email_endpoint = Some(endpoint.into());
        self.email_list_pointer = pointer.into();
        self
    }

    pub fn with_authorize_extra<I, K, V>(mut self, pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.authorize_extra = pairs
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        self
    }

    pub fn with_userinfo_query<I, K, V>(mut self, pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.userinfo_query = pairs
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        self
    }

    pub fn with_token_fields(mut self, fields: TokenFields) -> Self {
        self.token_fields = fields;
        self
    }

    pub fn with_token_request(mut self, request: TokenRequest) -> Self {
        self.token_request = request;
        self
    }

    pub fn with_client_auth(mut self, auth: ClientAuth) -> Self {
        self.client_auth = auth;
        self
    }

    pub fn with_userinfo_auth(mut self, auth: UserInfoAuth) -> Self {
        self.userinfo_auth = auth;
        self
    }

    pub fn with_userinfo_request(mut self, request: UserInfoRequest) -> Self {
        self.userinfo_request = request;
        self
    }

    pub fn with_userinfo_body<I, K, V>(mut self, pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.userinfo_body = pairs
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        self
    }

    pub fn with_subject_endpoint(
        mut self,
        endpoint: impl Into<String>,
        pointer: impl Into<String>,
    ) -> Self {
        self.subject_endpoint = Some(endpoint.into());
        self.subject_endpoint_pointer = pointer.into();
        self
    }

    pub fn with_subject_endpoint_extra<I, K, V>(mut self, pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.subject_endpoint_extra = pairs
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        self
    }

    pub fn with_token_extra<I, K, V>(mut self, pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.token_extra = pairs
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        self
    }

    pub fn with_authorize_client_id_key(mut self, key: impl Into<String>) -> Self {
        self.authorize_client_id_key = key.into();
        self
    }

    /// Resolve a preset by its stable name (for configuration-driven apps).
    ///
    /// `microsoft` uses the `common` tenant and `gitlab` uses gitlab.com; use
    /// the typed constructors when a tenant/self-hosted base is required.
    pub fn preset(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "github" => Some(Self::github()),
            "facebook" => Some(Self::facebook()),
            "google" => Some(Self::google()),
            "microsoft" => Some(Self::microsoft("common")),
            "gitlab" => Some(Self::gitlab("https://gitlab.com")),
            "gitee" => Some(Self::gitee()),
            "dingtalk" => Some(Self::dingtalk()),
            "feishu" => Some(Self::feishu()),
            "wechat" => Some(Self::wechat()),
            "weibo" => Some(Self::weibo()),
            "qq" => Some(Self::qq()),
            "douyin" => Some(Self::douyin()),
            _ => None,
        }
    }

    pub fn without_pkce(mut self) -> Self {
        self.pkce = false;
        self
    }

    /// GitHub OAuth App.
    pub fn github() -> Self {
        Self::custom(
            "github",
            "https://github.com/login/oauth/authorize",
            "https://github.com/login/oauth/access_token",
            "https://api.github.com/user",
        )
        .with_scopes(["read:user", "user:email"])
        .with_mapping("/id", Some("/name"), Some("/email"), Some("/avatar_url"))
        .with_email_endpoint("https://api.github.com/user/emails", "/email")
        .with_authorize_extra([("allow_signup", "false")])
    }

    /// Google OAuth 2.0 / OIDC userinfo.
    pub fn google() -> Self {
        Self::custom(
            "google",
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
            "https://openidconnect.googleapis.com/v1/userinfo",
        )
        .with_scopes(["openid", "email", "profile"])
        .with_mapping("/sub", Some("/name"), Some("/email"), Some("/picture"))
        .with_authorize_extra([("access_type", "online")])
    }

    /// Facebook Login (Graph API v20).
    pub fn facebook() -> Self {
        Self::custom(
            "facebook",
            "https://www.facebook.com/v20.0/dialog/oauth",
            "https://graph.facebook.com/v20.0/oauth/access_token",
            "https://graph.facebook.com/me",
        )
        .with_scopes(["public_profile", "email"])
        .with_token_request(TokenRequest::QueryGet)
        .with_userinfo_query([("fields", "id,name,email,picture")])
        .with_mapping(
            "/id",
            Some("/name"),
            Some("/email"),
            Some("/picture/data/url"),
        )
    }

    /// Microsoft identity platform for one tenant (`common` for multi-tenant).
    pub fn microsoft(tenant: impl AsRef<str>) -> Self {
        let tenant = tenant.as_ref().trim_matches('/');
        let base = format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0");
        Self::custom(
            "microsoft",
            format!("{base}/authorize"),
            format!("{base}/token"),
            "https://graph.microsoft.com/oidc/userinfo",
        )
        .with_scopes(["openid", "profile", "email"])
        .with_mapping("/sub", Some("/name"), Some("/email"), None::<&str>)
    }

    /// GitLab.com or a self-hosted GitLab base URL.
    pub fn gitlab(base_url: impl AsRef<str>) -> Self {
        let base = base_url.as_ref().trim_end_matches('/');
        Self::custom(
            "gitlab",
            format!("{base}/oauth/authorize"),
            format!("{base}/oauth/token"),
            format!("{base}/api/v4/user"),
        )
        .with_scopes(["read_user"])
        .with_mapping("/id", Some("/name"), Some("/email"), Some("/avatar_url"))
    }

    /// Gitee (码云).
    pub fn gitee() -> Self {
        Self::custom(
            "gitee",
            "https://gitee.com/oauth/authorize",
            "https://gitee.com/oauth/token",
            "https://gitee.com/api/v5/user",
        )
        .with_scopes(["user_info"])
        .with_mapping("/id", Some("/name"), Some("/email"), Some("/avatar_url"))
    }

    /// DingTalk (钉钉) OAuth 2.0.
    pub fn dingtalk() -> Self {
        let fields = TokenFields {
            client_id_key: "clientId".into(),
            client_secret_key: "clientSecret".into(),
            grant_type_key: "grantType".into(),
            redirect_uri_key: None,
            access_token: "/accessToken".into(),
            refresh_token: Some("/refreshToken".into()),
            expires_in: Some("/expireIn".into()),
            token_type: None,
            subject: Some("/openId".into()),
            ..TokenFields::default()
        };
        let mut provider = Self::custom(
            "dingtalk",
            "https://login.dingtalk.com/oauth2/auth",
            "https://api.dingtalk.com/v1.0/oauth2/userAccessToken",
            "https://api.dingtalk.com/v1.0/contact/users/me",
        )
        .with_scopes(["openid"])
        .with_token_request(TokenRequest::JsonPost)
        .with_token_fields(fields)
        .with_mapping(
            "/unionId",
            Some("/nick"),
            Some("/email"),
            Some("/avatarUrl"),
        )
        .with_authorize_extra([("prompt", "consent")])
        .without_pkce();
        provider.subject_fallbacks.push("/openId".into());
        provider
    }

    /// Feishu/Lark OAuth 2.0 (v2 token endpoint).
    pub fn feishu() -> Self {
        let fields = TokenFields {
            access_token: "/access_token".into(),
            refresh_token: Some("/refresh_token".into()),
            expires_in: Some("/expires_in".into()),
            token_type: Some("/token_type".into()),
            ..TokenFields::default()
        };
        Self::custom(
            "feishu",
            "https://open.feishu.cn/open-apis/authen/v1/authorize",
            "https://open.feishu.cn/open-apis/authen/v2/oauth/token",
            "https://open.feishu.cn/open-apis/authen/v1/user_info",
        )
        .with_scopes(["contact:user.base:readonly"])
        .with_authorize_client_id_key("app_id")
        .with_token_request(TokenRequest::JsonPost)
        .with_token_fields(fields)
        .with_mapping(
            "/data/open_id",
            Some("/data/name"),
            Some("/data/enterprise_email"),
            Some("/data/avatar_url"),
        )
        .without_pkce()
    }

    /// Weibo OAuth 2.0.
    pub fn weibo() -> Self {
        let fields = TokenFields {
            subject: Some("/uid".into()),
            ..TokenFields::default()
        };
        let mut provider = Self::custom(
            "weibo",
            "https://api.weibo.com/oauth2/authorize",
            "https://api.weibo.com/oauth2/access_token",
            "https://api.weibo.com/2/users/show.json",
        )
        .with_token_fields(fields)
        .with_userinfo_auth(UserInfoAuth::Query)
        .with_mapping(
            "/id",
            Some("/screen_name"),
            None::<&str>,
            Some("/profile_image_url"),
        )
        .without_pkce();
        provider.userinfo_subject_key = Some("uid".into());
        provider
    }

    /// QQ Connect OAuth 2.0.
    pub fn qq() -> Self {
        let mut provider = Self::custom(
            "qq",
            "https://graph.qq.com/oauth2.0/authorize",
            "https://graph.qq.com/oauth2.0/token",
            "https://graph.qq.com/user/get_user_info",
        )
        .with_scopes(["get_user_info"])
        .with_token_request(TokenRequest::QueryGet)
        .with_token_extra([("fmt", "json")])
        .with_subject_endpoint("https://graph.qq.com/oauth2.0/me", "/openid")
        .with_subject_endpoint_extra([("fmt", "json")])
        .with_userinfo_auth(UserInfoAuth::Query)
        .with_userinfo_query([("oauth_consumer_key", "{client_id}")])
        .with_mapping(
            "/openid",
            Some("/nickname"),
            None::<&str>,
            Some("/figureurl_qq_2"),
        )
        .without_pkce();
        provider.userinfo_subject_key = Some("openid".into());
        provider
    }

    /// Douyin (抖音) OAuth 2.0.
    pub fn douyin() -> Self {
        let fields = TokenFields {
            client_id_key: "client_key".into(),
            client_secret_key: "client_secret".into(),
            redirect_uri_key: None,
            access_token: "/data/access_token".into(),
            refresh_token: Some("/data/refresh_token".into()),
            expires_in: Some("/data/expires_in".into()),
            token_type: None,
            subject: Some("/data/open_id".into()),
            ..TokenFields::default()
        };
        Self::custom(
            "douyin",
            "https://open.douyin.com/platform/oauth/connect/",
            "https://open.douyin.com/oauth/access_token/",
            "https://open.douyin.com/oauth/userinfo/",
        )
        .with_scopes(["user_info"])
        .with_authorize_client_id_key("client_key")
        .with_token_request(TokenRequest::JsonPost)
        .with_token_fields(fields)
        .with_userinfo_request(UserInfoRequest::JsonPost)
        .with_userinfo_body([("access_token", "{access_token}"), ("open_id", "{subject}")])
        .with_mapping(
            "/data/open_id",
            Some("/data/nickname"),
            None::<&str>,
            Some("/data/avatar"),
        )
        .without_pkce()
    }

    /// WeChat Open Platform QR login (`snsapi_login`).
    pub fn wechat() -> Self {
        let fields = TokenFields {
            client_id_key: "appid".into(),
            client_secret_key: "secret".into(),
            redirect_uri_key: None,
            subject: Some("/openid".into()),
            ..TokenFields::default()
        };
        let mut provider = Self::custom(
            "wechat",
            "https://open.weixin.qq.com/connect/qrconnect",
            "https://api.weixin.qq.com/sns/oauth2/access_token",
            "https://api.weixin.qq.com/sns/userinfo",
        )
        .with_scopes(["snsapi_login"])
        .with_authorize_client_id_key("appid")
        .with_token_request(TokenRequest::QueryGet)
        .with_token_fields(fields)
        .with_userinfo_auth(UserInfoAuth::Query)
        .with_userinfo_query([("lang", "zh_CN")])
        .with_mapping(
            "/unionid",
            Some("/nickname"),
            None::<&str>,
            Some("/headimgurl"),
        )
        .with_authorize_extra([("style", "qrconnect")])
        .without_pkce();
        provider.userinfo_subject_key = Some("openid".into());
        provider.subject_fallbacks.push("/openid".into());
        provider
    }
}
