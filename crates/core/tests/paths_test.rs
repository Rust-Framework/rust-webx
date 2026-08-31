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

#[test]
fn framework_root_honors_rust_framework_root_env() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("rust-framework-root-test-{nanos}"));
    fs::create_dir_all(dir.join("rust-webx")).unwrap();
    fs::create_dir_all(dir.join("rust-ef")).unwrap();

    std::env::set_var("RUST_FRAMEWORK_ROOT", &dir);
    assert_eq!(rust_webx_core::paths::framework_root().unwrap(), dir);
    assert!(rust_webx_core::paths::looks_like_framework_root(&dir));

    std::env::remove_var("RUST_FRAMEWORK_ROOT");
    let _ = fs::remove_dir_all(&dir);
}
