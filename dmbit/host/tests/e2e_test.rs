//! Dmbit HTTP end-to-end tests (SQLite, isolated temp directory per test).

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

/// Isolated app directory with an appsettings the host can load.
fn setup_app_dir() -> tempfile::TempDir {
    INIT.call_once(|| {
        let _ = tracing_subscriber::fmt::try_init();
    });

    std::env::set_var("APP_ENV", "Development");
    let dir = tempfile::tempdir().unwrap();
    let appsettings = serde_json::json!({
        "App": { "Name": "dmbit-test", "Urls": ["http://127.0.0.1:0"] },
        "Jwt": { "Secret": "dmbit-e2e-test-jwt-secret-min-32-chars" },
        "Site": { "Title": "Test Site", "BrandName": "Test", "RoomName": "Room A" }
    });
    std::fs::write(
        dir.path().join("appsettings.json"),
        serde_json::to_string_pretty(&appsettings).unwrap(),
    )
    .unwrap();
    std::env::set_var("WEBX_APP_BASE", dir.path());
    dir
}

struct DmbitFixture {
    _dir: tempfile::TempDir,
    server: webx::TestServer,
}

impl DmbitFixture {
    fn base(&self) -> String {
        self.server.base_url.clone()
    }

    async fn teardown(self) {
        self.server.teardown().await;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

async fn spawn_dmbit() -> DmbitFixture {
    let dir = setup_app_dir();
    let port = webx::free_port();
    let server = webx::spawn(build_host(), port).await;
    DmbitFixture { _dir: dir, server }
}

async fn admin_token(client: &reqwest::Client, base: &str) -> String {
    let login = client
        .post(format!("{}/api/auth/login", base))
        .json(&serde_json::json!({
            "email": "admin@dmbit.local",
            "password": "admin123"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status().as_u16(), 200, "admin login failed");
    login.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .expect("token present")
        .to_string()
}

fn header(token: &str) -> String {
    format!("Bearer {token}")
}

#[tokio::test]
#[serial]
async fn e2e_admin_login_and_me() {
    let fx = spawn_dmbit().await;
    let base = fx.base();
    let client = reqwest::Client::new();

    let token = admin_token(&client, &base).await;
    let me = client
        .get(format!("{base}/api/auth/me"))
        .header("Authorization", header(&token))
        .send()
        .await
        .unwrap();
    assert_eq!(me.status().as_u16(), 200);
    let me: serde_json::Value = me.json().await.unwrap();
    assert_eq!(me["email"], "admin@dmbit.local");

    let anonymous = client
        .get(format!("{base}/api/auth/me"))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status().as_u16(), 401);

    fx.teardown().await;
}

#[tokio::test]
#[serial]
async fn e2e_product_spec_device_flow() {
    let fx = spawn_dmbit().await;
    let base = fx.base();
    let client = reqwest::Client::new();
    let token = admin_token(&client, &base).await;
    let suffix = unique_suffix();

    // Product (JSON body).
    let product: serde_json::Value = client
        .post(format!("{base}/api/products"))
        .header("Authorization", header(&token))
        .json(&serde_json::json!({
            "name": "E2E Product",
            "code": format!("P-{suffix}"),
            "category": "compute",
            "remark": "created by e2e"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let product_id = product["id"].as_str().expect("product id").to_string();

    // Spec (JSON body with a composite component list).
    let spec: serde_json::Value = client
        .post(format!("{base}/api/specs"))
        .header("Authorization", header(&token))
        .json(&serde_json::json!({
            "product_id": product_id,
            "code": format!("S-{suffix}"),
            "brand": "E2E",
            "parameters": "机箱:1U",
            "unit": "台",
            "planned_quantity": 5,
            "components": [
                { "kind": "disk", "model": "E2E-SSD", "capacity_gb": 960, "qty_per_unit": 2 }
            ]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let spec_id = spec["id"].as_str().expect("spec id").to_string();
    assert_eq!(spec["code"], format!("S-{suffix}"));

    // Generate devices: the spec id comes from the route, the body only has options.
    let generated: serde_json::Value = client
        .post(format!("{base}/api/specs/{spec_id}/devices/generate"))
        .header("Authorization", header(&token))
        .json(&serde_json::json!({ "count": 2, "asset_prefix": "E2E-" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(generated["created"], 2, "generate failed: {generated}");

    let devices: Vec<serde_json::Value> = client
        .get(format!("{base}/api/specs/{spec_id}/devices"))
        .header("Authorization", header(&token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(devices.len(), 2);
    let device_id = devices[0]["id"].as_str().expect("device id").to_string();
    assert!(devices[0]["asset_code"]
        .as_str()
        .unwrap_or_default()
        .starts_with("E2E-"));

    // Update by path id only: the body carries no id field.
    let updated: serde_json::Value = client
        .put(format!("{base}/api/devices/{device_id}"))
        .header("Authorization", header(&token))
        .json(&serde_json::json!({ "status": "运行中", "location": "A1" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["status"], "运行中");
    assert_eq!(updated["location"], "A1");

    fx.teardown().await;
}

#[tokio::test]
#[serial]
async fn e2e_inventory_csv_import() {
    let fx = spawn_dmbit().await;
    let base = fx.base();
    let client = reqwest::Client::new();
    let token = admin_token(&client, &base).await;
    let suffix = unique_suffix();
    let product_code = format!("TP-{suffix}");
    let spec_code = format!("TS-{suffix}");

    let headers = [
        "产品名称",
        "产品编码",
        "类别",
        "规格编码",
        "品牌",
        "机箱",
        "主板",
        "内存",
        "接口",
        "扩展",
        "电源",
        "光模块",
        "附加参数",
        "单位",
        "数量",
        "部件类型",
        "部件型号",
        "容量",
        "单台数量",
        "备注",
        "排序",
    ];
    let row = [
        "E2E 导入产品",
        product_code.as_str(),
        "compute",
        spec_code.as_str(),
        "E2E",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "台",
        "5",
        "加速卡",
        "A100",
        "",
        "1",
        "e2e import",
        "0",
    ];
    let csv = format!("{}\n{}\n", headers.join(","), row.join(","));

    let part = reqwest::multipart::Part::bytes(csv.into_bytes())
        .file_name("inventory.csv")
        .mime_str("text/csv")
        .unwrap();
    let form = reqwest::multipart::Form::new().part("file", part);
    let imported: serde_json::Value = client
        .post(format!("{base}/api/inventory/import"))
        .header("Authorization", header(&token))
        .multipart(form)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(
        imported["products_upserted"], 1,
        "import failed: {imported}"
    );
    assert_eq!(imported["components_written"], 1, "import: {imported}");

    let products: Vec<serde_json::Value> = client
        .get(format!("{base}/api/products"))
        .header("Authorization", header(&token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        products.iter().any(|p| p["code"] == product_code),
        "imported product missing"
    );

    fx.teardown().await;
}
