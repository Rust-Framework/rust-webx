//! Resource lookup and fallback behavior.

use webx_core::{IStringLocalizer, LocalizationOptions, StringLocalizer, TextRef};

fn localizer() -> StringLocalizer {
    let options = LocalizationOptions::none()
        .resources("zh", r#"{"hello":"你好","only_zh":"仅中文","bye":"再见"}"#)
        .resources("en", r#"{"hello":"Hello","bye":"Bye"}"#);
    StringLocalizer::new("zh-CN", &options).unwrap()
}

#[test]
fn exact_parent_and_default_culture_fallback() {
    let localizer = localizer();

    // Parent culture: zh-Hans-CN → zh-Hans → zh.
    assert_eq!(localizer.get_in("zh-Hans-CN", "hello").value, "你好");
    // Exact culture: en-US → en.
    assert_eq!(localizer.get_in("en-US", "hello").value, "Hello");
    // Not translated in en → falls back to the default culture's resource.
    assert_eq!(localizer.get_in("en", "only_zh").value, "仅中文");
    // No explicit culture → default culture.
    assert_eq!(localizer.get("hello").value, "你好");
}

#[test]
fn missing_key_reports_resource_not_found() {
    let missing = localizer().get("endpoint.missing");
    assert_eq!(missing.value, "endpoint.missing");
    assert!(missing.resource_not_found);
}

#[test]
fn resource_directory_loads_one_json_file_per_culture() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("zh-CN.json"), r#"{"hello":"你好"}"#).unwrap();
    std::fs::write(dir.path().join("en.json"), r#"{"hello":"Hello"}"#).unwrap();
    std::fs::write(dir.path().join("ignore.txt"), "not a resource").unwrap();

    let options = LocalizationOptions::new().resource_dir(dir.path());
    let localizer = StringLocalizer::new("zh-CN", &options).unwrap();

    assert_eq!(localizer.get("hello").value, "你好");
    assert_eq!(localizer.get_in("en", "hello").value, "Hello");

    // Explicit compiled-in entries override the directory.
    let options = LocalizationOptions::none()
        .resource_dir(dir.path())
        .resources("en", r#"{"hello":"Hi"}"#);
    let localizer = StringLocalizer::new("en", &options).unwrap();
    assert_eq!(localizer.get("hello").value, "Hi");
}

#[test]
fn text_ref_prefers_resource_then_literal_then_key() {
    let localizer = localizer();

    let found = localizer.resolve(TextRef::new("hello", "literal"));
    assert_eq!(found.value, "你好");

    let literal = localizer.resolve(TextRef::new("endpoint.missing", "literal"));
    assert_eq!(literal.value, "literal");

    let key = localizer.resolve(TextRef::key("endpoint.missing"));
    assert_eq!(key.value, "endpoint.missing");
    assert!(key.resource_not_found);

    let plain = localizer.resolve(TextRef::text("plain text"));
    assert_eq!(plain.value, "plain text");
}
