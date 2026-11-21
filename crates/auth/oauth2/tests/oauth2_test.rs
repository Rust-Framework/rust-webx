use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use webx_oauth2::{
    OAuth2Client, OAuth2Config, OAuth2Provider, TokenFields, TokenRequest, UserInfoRequest,
};

fn config(redirect: &str) -> OAuth2Config {
    OAuth2Config::new("client-id", Some("client-secret".into()), redirect)
}

#[test]
fn presets_have_expected_endpoints_and_scopes() {
    let github = OAuth2Provider::github();
    assert_eq!(
        github.authorization_endpoint,
        "https://github.com/login/oauth/authorize"
    );
    assert_eq!(github.subject_pointer, "/id");
    assert!(github.scopes.contains(&"user:email".to_string()));

    let google = OAuth2Provider::google();
    assert_eq!(google.token_endpoint, "https://oauth2.googleapis.com/token");
    assert!(google.scopes.iter().any(|scope| scope == "openid"));

    let microsoft = OAuth2Provider::microsoft("contoso.onmicrosoft.com");
    assert!(microsoft
        .authorization_endpoint
        .contains("/contoso.onmicrosoft.com/oauth2/v2.0/authorize"));

    let gitlab = OAuth2Provider::gitlab("https://gitlab.example.com");
    assert_eq!(
        gitlab.userinfo_endpoint,
        "https://gitlab.example.com/api/v4/user"
    );

    let gitee = OAuth2Provider::gitee();
    assert_eq!(gitee.token_endpoint, "https://gitee.com/oauth/token");

    let facebook = OAuth2Provider::facebook();
    assert_eq!(facebook.userinfo_endpoint, "https://graph.facebook.com/me");

    let dingtalk = OAuth2Provider::dingtalk();
    assert!(!dingtalk.pkce);
    assert_eq!(dingtalk.token_fields.access_token, "/accessToken");

    let feishu = OAuth2Provider::feishu();
    assert_eq!(feishu.authorize_client_id_key, "app_id");
    assert_eq!(feishu.subject_pointer, "/data/open_id");

    let wechat = OAuth2Provider::wechat();
    assert_eq!(wechat.authorize_client_id_key, "appid");
    assert!(wechat.subject_fallbacks.contains(&"/openid".to_string()));

    let weibo = OAuth2Provider::weibo();
    assert_eq!(weibo.userinfo_subject_key.as_deref(), Some("uid"));

    let qq = OAuth2Provider::qq();
    assert!(qq.subject_endpoint.is_some());
    assert_eq!(qq.userinfo_auth, webx_oauth2::UserInfoAuth::Query);

    let douyin = OAuth2Provider::douyin();
    assert_eq!(
        douyin.userinfo_request,
        webx_oauth2::UserInfoRequest::JsonPost
    );
    assert!(!douyin.userinfo_body.is_empty());
    assert_eq!(
        OAuth2Provider::preset("github").expect("preset").name,
        "github"
    );
    assert!(OAuth2Provider::preset("unknown").is_none());
}

#[test]
fn authorization_url_contains_state_and_pkce() {
    let provider = OAuth2Provider::github();
    let client = OAuth2Client::new(provider, config("http://localhost:8765/cb")).expect("client");
    let auth = client.authorization_url("state-1").expect("authorization");
    assert_eq!(auth.state, "state-1");
    assert_eq!(auth.pkce_verifier.as_ref().map(String::len), Some(43));
    assert!(auth.url.contains("client_id=client-id"));
    assert!(auth.url.contains("state=state-1"));
    assert!(auth.url.contains("code_challenge_method=S256"));
    assert!(auth.url.contains("scope=read%3Auser+user%3Aemail"));
}

#[test]
fn http_endpoints_require_https_unless_explicitly_allowed() {
    let provider = OAuth2Provider::custom(
        "mock",
        "http://127.0.0.1:1/authorize",
        "http://127.0.0.1:1/token",
        "http://127.0.0.1:1/user",
    );
    let error = OAuth2Client::new(provider, config("http://localhost/cb")).err();
    assert!(error.is_some());
}

#[tokio::test]
async fn qq_and_douyin_provider_shapes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let base = format!("http://127.0.0.1:{}", addr.port());
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buffer = vec![0u8; 16 * 1024];
            let read = stream.read(&mut buffer).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            let path = path.split('?').next().unwrap_or("/");
            let (status, body) = match path {
                "/qq/token" => ("200 OK", r#"{"access_token":"qq-token"}"#),
                "/qq/me" => ("200 OK", r#"{"openid":"qq-openid"}"#),
                "/qq/user" => (
                    "200 OK",
                    r#"{"openid":"qq-openid","nickname":"QQ User","figureurl_qq_2":"https://img/qq.png"}"#,
                ),
                "/dy/token" => (
                    "200 OK",
                    r#"{"data":{"access_token":"dy-token","open_id":"dy-openid","expires_in":3600}}"#,
                ),
                "/dy/user" => (
                    "200 OK",
                    r#"{"data":{"open_id":"dy-openid","nickname":"Douyin User","avatar":"https://img/dy.png"}}"#,
                ),
                _ => ("404 Not Found", "{}"),
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });

    let mut config = config("http://localhost:8765/cb");
    config.pkce = false;
    config.allow_private_networks = true;

    let qq = OAuth2Provider::custom(
        "qq-test",
        format!("{base}/qq/authorize"),
        format!("{base}/qq/token"),
        format!("{base}/qq/user"),
    )
    .with_token_request(TokenRequest::QueryGet)
    .with_subject_endpoint(format!("{base}/qq/me"), "/openid")
    .with_userinfo_auth(webx_oauth2::UserInfoAuth::Query)
    .with_userinfo_query([("oauth_consumer_key", "{client_id}")])
    .with_mapping(
        "/openid",
        Some("/nickname"),
        None::<&str>,
        Some("/figureurl_qq_2"),
    );
    let qq_client = OAuth2Client::new(qq, config.clone()).expect("qq client");
    let qq_token = qq_client
        .exchange_code("code", None)
        .await
        .expect("qq token");
    assert_eq!(qq_token.subject.as_deref(), Some("qq-openid"));
    let qq_user = qq_client.userinfo(&qq_token).await.expect("qq user");
    assert_eq!(qq_user.subject, "qq-openid");
    assert_eq!(qq_user.name, "QQ User");

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
    let douyin = OAuth2Provider::custom(
        "douyin-test",
        format!("{base}/dy/authorize"),
        format!("{base}/dy/token"),
        format!("{base}/dy/user"),
    )
    .with_token_request(TokenRequest::JsonPost)
    .with_token_fields(fields)
    .with_userinfo_request(UserInfoRequest::JsonPost)
    .with_userinfo_body([("access_token", "{access_token}"), ("open_id", "{subject}")])
    .with_mapping(
        "/data/open_id",
        Some("/data/nickname"),
        None::<&str>,
        Some("/data/avatar"),
    );
    let douyin_client = OAuth2Client::new(douyin, config).expect("douyin client");
    let douyin_token = douyin_client
        .exchange_code("code", None)
        .await
        .expect("douyin token");
    assert_eq!(douyin_token.subject.as_deref(), Some("dy-openid"));
    let douyin_user = douyin_client
        .userinfo(&douyin_token)
        .await
        .expect("douyin user");
    assert_eq!(douyin_user.subject, "dy-openid");
    assert_eq!(douyin_user.name, "Douyin User");

    server.abort();
    let _ = tokio::time::timeout(Duration::from_secs(1), server).await;
}

#[tokio::test]
async fn exchange_and_userinfo_with_email_fallback() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let base = format!("http://127.0.0.1:{}", addr.port());
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buffer = vec![0u8; 16 * 1024];
            let read = stream.read(&mut buffer).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            let path = path.split('?').next().unwrap_or("/");
            let (status, body) = if path.starts_with("/token") {
                (
                    "200 OK",
                    r#"{"access_token":"token-1","token_type":"Bearer","expires_in":3600}"#,
                )
            } else if path.starts_with("/user") {
                (
                    "200 OK",
                    r#"{"id":42,"name":"Ada","avatar_url":"https://img/a.png"}"#,
                )
            } else if path.starts_with("/emails") {
                ("200 OK", r#"[{"email":"ada@example.com","primary":true}]"#)
            } else {
                ("404 Not Found", "{}")
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });

    let provider = OAuth2Provider::custom(
        "mock",
        format!("{base}/authorize"),
        format!("{base}/token"),
        format!("{base}/user"),
    )
    .with_scopes(["profile", "email"])
    .with_mapping("/id", Some("/name"), Some("/email"), Some("/avatar_url"))
    .with_email_endpoint(format!("{base}/emails"), "/email");
    let mut config = config("http://localhost:8765/cb");
    config.pkce = false; // the mock does not validate PKCE
    config.allow_private_networks = true;
    let client = OAuth2Client::new(provider, config).expect("client");
    let token = client
        .exchange_code("code-1", None)
        .await
        .expect("exchange");
    assert_eq!(token.access_token, "token-1");
    assert_eq!(token.expires_in, Some(3600));
    let user = client.userinfo(&token).await.expect("userinfo");
    assert_eq!(user.subject, "42");
    assert_eq!(user.name, "Ada");
    assert_eq!(user.email.as_deref(), Some("ada@example.com"));
    assert_eq!(user.avatar_url.as_deref(), Some("https://img/a.png"));
    let identity = user.into_identity();
    assert_eq!(identity.id, "42");
    assert_eq!(identity.email, "ada@example.com");

    server.abort();
    let _ = tokio::time::timeout(Duration::from_secs(1), server).await;
}
