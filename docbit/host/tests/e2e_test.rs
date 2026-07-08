//! Docbit HTTP end-to-end tests (SQLite, isolated temp directory per test).

use serial_test::serial;

use std::net::TcpListener;
use std::sync::Once;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static INIT: Once = Once::new();

fn find_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Isolated app directory with appsettings + empty docs/.
fn setup_app_dir() -> tempfile::TempDir {
    INIT.call_once(|| {
        let _ = tracing_subscriber::fmt::try_init();
    });

    std::env::set_var("APP_ENV", "Development");
    let dir = tempfile::tempdir().unwrap();
    let appsettings = serde_json::json!({
        "App": { "Name": "docbit-test", "Urls": ["http://127.0.0.1:0"] },
        "Jwt": { "Secret": "docbit-e2e-test-jwt-secret-min-32-chars" },
        "Cors": {
            "Origins": ["*"],
            "Methods": ["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"],
            "Headers": ["Content-Type", "Authorization"]
        },
        "Site": {
            "Title": "Test Site",
            "BrandName": "Test",
            "Tagline": "E2E",
            "Author": "Test",
            "Bio": "Test",
            "Links": { "Github": "https://example.com", "Docs": "/docs" }
        }
    });
    std::fs::write(
        dir.path().join("appsettings.json"),
        serde_json::to_string_pretty(&appsettings).unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    std::env::set_var("RUST_WEBX_APP_BASE", dir.path());
    dir
}

struct DocbitFixture {
    port: u16,
    _dir: tempfile::TempDir,
    handle: rust_webx::ServerHandle,
    server: tokio::task::JoinHandle<()>,
}

impl DocbitFixture {
    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    async fn teardown(self) {
        self.handle.shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.server).await;
        // Allow SQLite connection pool to release file handles before the next test.
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

async fn wait_ready(port: u16) {
    let url = format!("http://127.0.0.1:{}/health/live", port);
    for _ in 0..60 {
        if let Ok(resp) = reqwest::get(&url).await {
            if resp.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("docbit failed to become ready on port {port}");
}

async fn spawn_docbit() -> DocbitFixture {
    let _dir = setup_app_dir();
    let port = find_free_port();
    let addr = format!("127.0.0.1:{}", port);
    let host = docbit_host::build_host();
    let handle = host.server_handle();
    let server = tokio::spawn(async move {
        let _ = host.run_at(&addr).await;
    });
    wait_ready(port).await;
    DocbitFixture {
        port,
        _dir,
        handle,
        server,
    }
}

async fn admin_token(client: &reqwest::Client, base: &str) -> String {
    let login = client
        .post(format!("{}/api/auth/login", base))
        .json(&serde_json::json!({
            "email": "admin@docbit.local",
            "password": "admin123"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status().as_u16(), 200);
    login
        .json::<serde_json::Value>()
        .await
        .unwrap()["token"]
        .as_str()
        .expect("token present")
        .to_string()
}

#[tokio::test]
#[serial]
async fn e2e_health_live_returns_pass() {
    let fx = spawn_docbit().await;
    let resp = reqwest::get(format!("{}/health/live", fx.base()))
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "pass");
    fx.teardown().await;
}

#[tokio::test]
#[serial]
async fn e2e_site_api_is_public() {
    let fx = spawn_docbit().await;
    let resp = reqwest::get(format!("{}/api/site", fx.base()))
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["Title"], "Test Site");
    fx.teardown().await;
}

#[tokio::test]
#[serial]
async fn e2e_admin_login_and_auth_me() {
    let fx = spawn_docbit().await;
    let client = reqwest::Client::new();
    let base = fx.base();
    let token = admin_token(&client, &base).await;

    let me = client
        .get(format!("{}/api/auth/me", base))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .unwrap();
    assert_eq!(me.status().as_u16(), 200);
    let profile: serde_json::Value = me.json().await.unwrap();
    assert_eq!(profile["email"], "admin@docbit.local");

    fx.teardown().await;
}

#[tokio::test]
#[serial]
async fn e2e_auth_me_without_token_returns_401_problem_json() {
    let fx = spawn_docbit().await;
    let resp = reqwest::get(format!("{}/api/auth/me", fx.base()))
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct.contains("application/problem+json"));
    fx.teardown().await;
}

#[tokio::test]
#[serial]
async fn e2e_register_login_flow() {
    let fx = spawn_docbit().await;
    let client = reqwest::Client::new();
    let base = fx.base();
    let email = format!("user-{}@e2e.test", unique_suffix());

    let reg = client
        .post(format!("{}/api/auth/register", base))
        .json(&serde_json::json!({
            "name": "E2E User",
            "email": email,
            "password": "password123"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(reg.status().as_u16(), 200);
    let reg_body: serde_json::Value = reg.json().await.unwrap();
    assert!(reg_body["token"].is_string());

    let login = client
        .post(format!("{}/api/auth/login", base))
        .json(&serde_json::json!({
            "email": email,
            "password": "password123"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status().as_u16(), 200);

    fx.teardown().await;
}

#[tokio::test]
#[serial]
async fn e2e_blog_list_is_public() {
    let fx = spawn_docbit().await;
    let resp = reqwest::get(format!("{}/api/blog", fx.base()))
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.is_array());
    fx.teardown().await;
}

#[tokio::test]
#[serial]
async fn e2e_admin_blog_crud() {
    let fx = spawn_docbit().await;
    let client = reqwest::Client::new();
    let base = fx.base();
    let token = admin_token(&client, &base).await;
    let slug = format!("e2e-post-{}", unique_suffix());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let create = client
        .post(format!("{}/api/blog", base))
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({
            "slug": slug,
            "title": "E2E Post",
            "summary": "Summary",
            "content": "Body content",
            "tags": ["e2e", "test"],
            "category_id": 1,
            "published_at": now
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(create.status().as_u16(), 200, "create failed");
    let created: serde_json::Value = create.json().await.unwrap();
    assert_eq!(created["slug"], slug);
    assert_eq!(created["title"], "E2E Post");

    let get = client
        .get(format!("{}/api/blog/{}", base, slug))
        .send()
        .await
        .unwrap();
    assert_eq!(get.status().as_u16(), 200);
    assert_eq!(get.json::<serde_json::Value>().await.unwrap()["title"], "E2E Post");

    let update = client
        .put(format!("{}/api/blog/{}", base, slug))
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({ "title": "E2E Updated" }))
        .send()
        .await
        .unwrap();
    assert_eq!(update.status().as_u16(), 200, "update failed");
    assert_eq!(
        update.json::<serde_json::Value>().await.unwrap()["title"],
        "E2E Updated"
    );

    let list = client
        .get(format!("{}/api/blog", base))
        .send()
        .await
        .unwrap();
    let posts: Vec<serde_json::Value> = list.json().await.unwrap();
    assert!(posts.iter().any(|p| p["slug"] == slug));

    let delete = client
        .delete(format!("{}/api/blog/{}", base, slug))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .unwrap();
    assert_eq!(delete.status().as_u16(), 200);

    let gone = client
        .get(format!("{}/api/blog/{}", base, slug))
        .send()
        .await
        .unwrap();
    assert_eq!(gone.status().as_u16(), 404);

    fx.teardown().await;
}

#[tokio::test]
#[serial]
async fn e2e_admin_rbac_list_roles() {
    let fx = spawn_docbit().await;
    let client = reqwest::Client::new();
    let base = fx.base();

    let anon = client
        .get(format!("{}/api/roles", base))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status().as_u16(), 401);

    let token = admin_token(&client, &base).await;
    let roles = client
        .get(format!("{}/api/roles", base))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .unwrap();
    assert_eq!(roles.status().as_u16(), 200);
    let body: Vec<serde_json::Value> = roles.json().await.unwrap();
    let names: Vec<&str> = body.iter().filter_map(|r| r["name"].as_str()).collect();
    assert!(names.contains(&"admin"));
    assert!(names.contains(&"user"));

    fx.teardown().await;
}
