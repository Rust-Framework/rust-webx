use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn app_base_honors_rust_webx_app_base_env() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("rust-webx-app-base-test-{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("appsettings.json"), "{}").unwrap();

    std::env::set_var("RUST_WEBX_APP_BASE", &dir);
    assert_eq!(rust_webx_core::paths::app_base(), dir);

    std::env::remove_var("RUST_WEBX_APP_BASE");
    let _ = fs::remove_dir_all(&dir);
}
