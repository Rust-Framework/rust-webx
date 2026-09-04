//! Docbit HTTP end-to-end tests (SQLite, isolated temp directory per test).

use serial_test::serial;

use std::sync::Once;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static INIT: Once = Once::new();

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Isolated app directory with appsettings + sample docs/.
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
            "Bio": "Test bio for SEO",
            "Links": { "Github": "https://example.com", "Docs": "/docs" },
            "Footer": {
                "SiteUrl": "www.example.test",
                "SiteLabel": "Test",
                "Copyright": "© test",
                "Icp": ""
            }
        }
    });
    std::fs::write(
        dir.path().join("appsettings.json"),
        serde_json::to_string_pretty(&appsettings).unwrap(),
    )
    .unwrap();
    let docs = dir.path().join("docs").join("demo-work");
    std::fs::create_dir_all(docs.join("ch")).unwrap();
    std::fs::write(
        docs.join("INDEX.json"),
        r#"{
  "meta": {
    "title": "Demo Work",
    "subtitle": "Demo",
    "docTitle": "Demo Docs",
    "description": "Demo portfolio work",
    "category": "tool",
    "tags": ["demo"],
    "featured": true,
    "sortOrder": 1,
    "foreword": "FOREWORD.md",
    "pathRules": {
      "chapterIndex": "{chapterId}/INDEX.md",
      "sectionFile": "{chapterId}/{sectionId}.md"
    }
  },
  "parts": [{
    "title": "Part",
    "chapters": [{
      "id": "ch",
      "title": "Chapter",
      "sections": [
        { "id": "exists", "title": "Exists" },
        { "id": "missing", "title": "Missing" }
      ]
    }]
  }]
}"#,
    )
    .unwrap();
    std::fs::write(docs.join("FOREWORD.md"), "# Foreword\n\nHello docs.\n").unwrap();
    std::fs::write(docs.join("ch").join("INDEX.md"), "# Chapter\n").unwrap();
    std::fs::write(
        docs.join("ch").join("exists.md"),
        "# Exists Page\n\nBody.\n",
    )
    .unwrap();
    // ch/missing.md intentionally absent — index filter must omit it.
    std::fs::create_dir_all(dir.path().join("wwwroot")).unwrap();
    std::fs::write(
        dir.path().join("wwwroot").join("index.html"),
        r#"<!doctype html><html><head><title>Shell</title></head><body><main id="app"><div class="loading-state"></div></main></body></html>"#,
    )
    .unwrap();
    std::env::set_var("RUST_WEBX_APP_BASE", dir.path());
    dir
}

struct DocbitFixture {
    _dir: tempfile::TempDir,
    server: rust_webx::TestServer,
}

impl DocbitFixture {
    fn base(&self) -> String {
        self.server.base_url.clone()
    }

    async fn teardown(self) {
        self.server.teardown().await;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

async fn spawn_docbit() -> DocbitFixture {
    let dir = setup_app_dir();
    let port = rust_webx::free_port();
    let server = rust_webx::spawn(docbit_host::build_host(), port).await;
    DocbitFixture { _dir: dir, server }
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
    login.json::<serde_json::Value>().await.unwrap()["token"]
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
            "category_id": "00000000-0000-4000-8000-000000000003",
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
    assert_eq!(
        get.json::<serde_json::Value>().await.unwrap()["title"],
        "E2E Post"
    );

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

async fn register_user_token(client: &reqwest::Client, base: &str) -> (String, String) {
    let email = format!("user-{}@e2e.test", unique_suffix());
    let reg = client
        .post(format!("{}/api/auth/register", base))
        .json(&serde_json::json!({
            "name": "Regular User",
            "email": email,
            "password": "password123"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(reg.status().as_u16(), 200);
    let token = reg.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .expect("token")
        .to_string();
    (email, token)
}

#[tokio::test]
#[serial]
async fn e2e_non_admin_forbidden_on_admin_routes() {
    let fx = spawn_docbit().await;
    let client = reqwest::Client::new();
    let base = fx.base();
    let (_email, token) = register_user_token(&client, &base).await;

    let roles = client
        .get(format!("{}/api/roles", base))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .unwrap();
    assert_eq!(roles.status().as_u16(), 403);
    let ct = roles
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct.contains("application/problem+json"));

    let categories = client
        .post(format!("{}/api/categories", base))
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({
            "name": "Forbidden",
            "slug": "forbidden",
            "sort_order": 0
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(categories.status().as_u16(), 403);

    fx.teardown().await;
}

#[tokio::test]
#[serial]
async fn e2e_admin_category_crud() {
    let fx = spawn_docbit().await;
    let client = reqwest::Client::new();
    let base = fx.base();
    let token = admin_token(&client, &base).await;
    let slug = format!("e2e-cat-{}", unique_suffix());

    let create = client
        .post(format!("{}/api/categories", base))
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({
            "name": "E2E Category",
            "slug": slug,
            "sort_order": 99
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(create.status().as_u16(), 200);
    let created: serde_json::Value = create.json().await.unwrap();
    let id = created["id"].as_str().expect("category id");
    assert_eq!(created["slug"], slug);

    let update = client
        .put(format!("{}/api/categories/{}", base, id))
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({ "name": "E2E Category Updated" }))
        .send()
        .await
        .unwrap();
    assert_eq!(update.status().as_u16(), 200);
    assert_eq!(
        update.json::<serde_json::Value>().await.unwrap()["name"],
        "E2E Category Updated"
    );

    let list = client
        .get(format!("{}/api/categories", base))
        .send()
        .await
        .unwrap();
    assert_eq!(list.status().as_u16(), 200);
    let tree: Vec<serde_json::Value> = list.json().await.unwrap();

    fn tree_has_slug(nodes: &[serde_json::Value], slug: &str) -> bool {
        nodes.iter().any(|n| {
            n["slug"] == slug
                || n.get("children")
                    .and_then(|c| c.as_array())
                    .is_some_and(|ch| tree_has_slug(ch, slug))
        })
    }
    assert!(tree_has_slug(&tree, &slug));

    let delete = client
        .delete(format!("{}/api/categories/{}", base, id))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .unwrap();
    assert_eq!(delete.status().as_u16(), 200);

    fx.teardown().await;
}

#[tokio::test]
#[serial]
async fn e2e_docs_content_accepts_slash_path_and_filters_missing() {
    let fx = spawn_docbit().await;
    let base = fx.base();

    let index = reqwest::get(format!("{base}/api/docs/demo-work/index"))
        .await
        .unwrap();
    assert_eq!(index.status().as_u16(), 200);
    let body: serde_json::Value = index.json().await.unwrap();
    let json = body.to_string();
    assert!(json.contains("exists.md"), "{json}");
    assert!(
        !json.contains("missing.md"),
        "missing leaf must be filtered: {json}"
    );

    let content = reqwest::get(format!("{base}/api/docs/demo-work/content/ch/exists.md"))
        .await
        .unwrap();
    assert_eq!(content.status().as_u16(), 200);
    let doc: serde_json::Value = content.json().await.unwrap();
    assert!(doc["content"].as_str().unwrap().contains("Exists Page"));

    // Legacy colon encoding still works.
    let legacy = reqwest::get(format!("{base}/api/docs/demo-work/content/ch:exists.md"))
        .await
        .unwrap();
    assert_eq!(legacy.status().as_u16(), 200);

    fx.teardown().await;
}

#[tokio::test]
#[serial]
async fn e2e_seo_robots_sitemap_and_ssr_shell() {
    let fx = spawn_docbit().await;
    let base = fx.base();
    let client = reqwest::Client::new();

    let robots = client
        .get(format!("{base}/robots.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(robots.status().as_u16(), 200);
    let robots_body = robots.text().await.unwrap();
    assert!(robots_body.contains("Sitemap:"));
    assert!(robots_body.contains("Disallow: /api/"));

    let sitemap = client
        .get(format!("{base}/sitemap.xml"))
        .send()
        .await
        .unwrap();
    assert_eq!(sitemap.status().as_u16(), 200);
    let sm = sitemap.text().await.unwrap();
    assert!(sm.contains("<urlset"));
    assert!(sm.contains("/works/demo-work"));

    let home = client
        .get(format!("{base}/"))
        .header("Accept", "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(home.status().as_u16(), 200);
    let html = home.text().await.unwrap();
    assert!(
        html.contains(r#"meta name="description""#) || html.contains("meta name=\"description\"")
    );
    assert!(html.contains("og:title") || html.contains("property=\"og:title\""));
    assert!(html.contains("data-ssr=\"1\""));
    assert!(html.contains("Demo Work") || html.contains("精选作品") || html.contains("Test"));

    let work = client
        .get(format!("{base}/works/demo-work"))
        .header("Accept", "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(work.status().as_u16(), 200);
    let work_html = work.text().await.unwrap();
    assert!(work_html.contains("Demo Work"));
    assert!(work_html.contains("canonical"));

    fx.teardown().await;
}
