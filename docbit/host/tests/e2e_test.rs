//! Docbit HTTP end-to-end tests (SQLite, isolated temp directory per test).

mod support;

use serial_test::serial;
use support::build_host;

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
    std::env::set_var("WEBX_APP_BASE", dir.path());
    dir
}

struct DocbitFixture {
    _dir: tempfile::TempDir,
    server: webx::TestServer,
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
    let port = webx::free_port();
    let server = webx::spawn(build_host(), port).await;
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

// ---------------------------------------------------------------------------
// Static files compiled into the executable
// ---------------------------------------------------------------------------

/// Send a conditional GET, returning the status code.
async fn status_with_range(base: &str, path: &str, range: &str) -> (u16, Option<String>) {
    let resp = reqwest::Client::new()
        .get(format!("{base}{path}"))
        .header("range", range)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let content_range = resp
        .headers()
        .get("content-range")
        .map(|value| value.to_str().unwrap().to_string());
    (status, content_range)
}

/// A deployment of just the executable: no `wwwroot` at all, so everything is
/// served from the binary.
#[tokio::test]
#[serial]
async fn e2e_static_files_are_served_from_the_binary_without_wwwroot() {
    let dir = setup_app_dir();
    // The shared fixture writes a shell index.html; remove it so this test
    // exercises the lone-executable deployment.
    std::fs::remove_dir_all(dir.path().join("wwwroot")).unwrap();

    let port = webx::free_port();
    let server = webx::spawn(build_host(), port).await;
    let base = server.base_url.clone();
    let client = reqwest::Client::new();

    let index = client
        .get(format!("{base}/index.html"))
        .send()
        .await
        .unwrap();
    assert_eq!(index.status().as_u16(), 200);
    let etag = index
        .headers()
        .get("etag")
        .expect("embedded files carry a content-derived ETag")
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        etag.starts_with("\"sha256-"),
        "expected a content hash, got {etag}"
    );
    assert_eq!(
        index
            .headers()
            .get("accept-ranges")
            .map(|value| value.to_str().unwrap()),
        Some("bytes"),
        "embedded files are seekable, so ranges must work"
    );
    assert!(!index.text().await.unwrap().is_empty());

    // A sibling with no index.html fallback involved is served from the binary
    // too, not shadowed by the SPA shell.
    let script = client.get(format!("{base}/app.js")).send().await.unwrap();
    assert_eq!(script.status().as_u16(), 200);
    assert_eq!(
        script.headers().get("content-type").unwrap(),
        "text/javascript"
    );
    assert!(script
        .headers()
        .get("etag")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("\"sha256-"));

    // Conditional request revalidates against the same validator.
    let conditional = client
        .get(format!("{base}/index.html"))
        .header("if-none-match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(conditional.status().as_u16(), 304);

    // An unknown path falls back to the embedded index.html for SPA routing.
    let fallback = client
        .get(format!("{base}/some/client/route"))
        .send()
        .await
        .unwrap();
    assert_eq!(fallback.status().as_u16(), 200);
    assert_eq!(
        fallback.headers().get("etag").unwrap().to_str().unwrap(),
        etag,
        "the fallback must be the embedded index.html"
    );

    server.teardown().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    drop(dir);
}

#[tokio::test]
#[serial]
async fn e2e_embedded_static_file_supports_byte_ranges() {
    let fx = spawn_docbit().await;
    let base = fx.base();
    let client = reqwest::Client::new();

    // Read the length from the representation so the assertions do not depend on
    // the size of the checked-in stylesheet.
    let full = client.get(format!("{base}/app.css")).send().await.unwrap();
    assert_eq!(full.status().as_u16(), 200);
    let len = full
        .headers()
        .get("content-length")
        .expect("embedded files have an exact Content-Length")
        .to_str()
        .unwrap()
        .to_string();

    let (status, content_range) = status_with_range(&base, "/app.css", "bytes=0-49").await;
    assert_eq!(status, 206);
    assert_eq!(
        content_range.as_deref(),
        Some(format!("bytes 0-49/{len}").as_str()),
        "the total length must be the embedded file's length"
    );

    // A start offset at or past the end cannot be satisfied, and the response
    // still reports the full length so a client can restart the transfer.
    let (status, content_range) =
        status_with_range(&base, "/app.css", &format!("bytes={len}-")).await;
    assert_eq!(status, 416);
    assert_eq!(
        content_range.as_deref(),
        Some(format!("bytes */{len}").as_str())
    );

    fx.teardown().await;
}

/// A file dropped next to the binary wins over the embedded copy; everything
/// else keeps coming from the binary even though `wwwroot/` exists on disk.
#[tokio::test]
#[serial]
async fn e2e_wwwroot_overrides_embedded_file() {
    let dir = setup_app_dir();
    let wwwroot = dir.path().join("wwwroot");
    std::fs::create_dir_all(&wwwroot).unwrap();
    std::fs::write(wwwroot.join("app.css"), b"/* operator override */").unwrap();

    let port = webx::free_port();
    let server = webx::spawn(build_host(), port).await;
    let base = server.base_url.clone();
    let client = reqwest::Client::new();

    let overridden = client.get(format!("{base}/app.css")).send().await.unwrap();
    assert_eq!(overridden.status().as_u16(), 200);
    let etag = overridden
        .headers()
        .get("etag")
        .map(|value| value.to_str().unwrap().to_string())
        .unwrap_or_default();
    assert!(
        !etag.starts_with("\"sha256-"),
        "a disk override is validated by size-mtime, not by the embedded content hash"
    );
    assert_eq!(overridden.text().await.unwrap(), "/* operator override */");

    // A sibling that was not overridden still comes from the binary. The
    // fixture's wwwroot/index.html must not swallow it as an SPA fallback.
    let embedded = client.get(format!("{base}/app.js")).send().await.unwrap();
    assert_eq!(embedded.status().as_u16(), 200);
    assert_eq!(
        embedded.headers().get("content-type").unwrap(),
        "text/javascript"
    );
    assert!(embedded
        .headers()
        .get("etag")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("\"sha256-"));

    server.teardown().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    drop(dir);
}

/// Write a documentation bundle zip the way an operator would.
fn write_bundle_zip(path: &std::path::Path) {
    use std::io::Write as _;

    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();

    let index = r#"{
  "meta": {
    "title": "Uploaded Work",
    "docTitle": "Uploaded Docs",
    "subtitle": "Uploaded subtitle",
    "description": "Replaced by an upload",
    "category": "tool",
    "tags": ["uploaded"],
    "sortOrder": 9,
    "pathRules": {
      "chapterIndex": "{chapterId}/INDEX.md",
      "sectionFile": "{chapterId}/{sectionId}.md"
    }
  },
  "parts": [{
    "title": "Part",
    "chapters": [{
      "id": "up",
      "title": "Uploaded Chapter",
      "sections": [{ "id": "fresh", "title": "Fresh Section" }]
    }]
  }]
}"#;

    for (name, body) in [
        ("INDEX.json", index),
        ("up/INDEX.md", "# Uploaded Chapter\n"),
        (
            "up/fresh.md",
            "# Fresh Section\n\ncontent from the upload\n",
        ),
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

/// An admin upload replaces the work's documentation and the API reflects it
/// immediately — no restart, and the catalog re-sync runs inside the request.
#[tokio::test]
#[serial]
async fn e2e_admin_uploads_documentation_bundle() {
    let fx = spawn_docbit().await;
    let base = fx.base();
    let client = reqwest::Client::new();

    // A section that only exists after the upload.
    let before = client
        .get(format!("{base}/api/docs/demo-work/content/up/fresh.md"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        before.status().as_u16(),
        404,
        "the fixture must not already contain the uploaded chapter"
    );

    let bundle_dir = tempfile::tempdir().unwrap();
    let bundle = bundle_dir.path().join("bundle.zip");
    write_bundle_zip(&bundle);
    let bytes = std::fs::read(&bundle).unwrap();

    let token = admin_token(&client, &base).await;
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name("bundle.zip")
        .mime_str("application/zip")
        .unwrap();
    let form = reqwest::multipart::Form::new().part("archive", part);

    let upload = client
        .post(format!("{base}/api/works/demo-work/docs"))
        .header("Authorization", format!("Bearer {}", token))
        .multipart(form)
        .send()
        .await
        .unwrap();
    let status = upload.status().as_u16();
    let body = upload.text().await.unwrap();
    assert_eq!(status, 200, "upload failed: {body}");

    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["slug"], "demo-work");
    assert!(
        json["files"].as_u64().unwrap() >= 3,
        "expected the bundle's files, got {body}"
    );

    // The new content is served straight away…
    let content = client
        .get(format!("{base}/api/docs/demo-work/content/up/fresh.md"))
        .send()
        .await
        .unwrap();
    assert_eq!(content.status().as_u16(), 200);
    assert!(content
        .text()
        .await
        .unwrap()
        .contains("content from the upload"));

    // …the navigation is rebuilt from the uploaded INDEX.json…
    let index: serde_json::Value = client
        .get(format!("{base}/api/docs/demo-work/index"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(index["title"], "Uploaded Docs");

    // …and the catalog re-sync picked up the new metadata, with no restart.
    let work: serde_json::Value = client
        .get(format!("{base}/api/exhibitions/demo-work"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        work["title"], "Uploaded Work",
        "the exhibition row must be re-synced from the uploaded INDEX.json"
    );

    fx.teardown().await;
}

/// Anonymous uploads are refused outright, and an admin's traversing archive is
/// rejected without writing anything outside the docs tree.
#[tokio::test]
#[serial]
async fn e2e_docs_upload_rejects_anonymous_and_traversing_archives() {
    let fx = spawn_docbit().await;
    let base = fx.base();
    let client = reqwest::Client::new();

    let bundle_dir = tempfile::tempdir().unwrap();
    let bundle = bundle_dir.path().join("bundle.zip");
    write_bundle_zip(&bundle);
    let bytes = std::fs::read(&bundle).unwrap();

    let anonymous = client
        .post(format!("{base}/api/works/demo-work/docs"))
        .multipart(
            reqwest::multipart::Form::new().part(
                "archive",
                reqwest::multipart::Part::bytes(bytes)
                    .file_name("bundle.zip")
                    .mime_str("application/zip")
                    .unwrap(),
            ),
        )
        .send()
        .await
        .unwrap();
    assert!(
        anonymous.status().as_u16() == 401 || anonymous.status().as_u16() == 403,
        "anonymous upload must be refused, got {}",
        anonymous.status()
    );

    // A zip whose entry escapes the archive root.
    let evil = bundle_dir.path().join("evil.zip");
    {
        use std::io::Write as _;
        let file = std::fs::File::create(&evil).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("../escaped.md", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"# escaped").unwrap();
        zip.finish().unwrap();
    }
    let evil_bytes = std::fs::read(&evil).unwrap();

    let token = admin_token(&client, &base).await;
    let response = client
        .post(format!("{base}/api/works/demo-work/docs"))
        .header("Authorization", format!("Bearer {}", token))
        .multipart(
            reqwest::multipart::Form::new().part(
                "archive",
                reqwest::multipart::Part::bytes(evil_bytes)
                    .file_name("evil.zip")
                    .mime_str("application/zip")
                    .unwrap(),
            ),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status().as_u16(),
        400,
        "a traversing archive must be a validation error"
    );

    fx.teardown().await;
}
