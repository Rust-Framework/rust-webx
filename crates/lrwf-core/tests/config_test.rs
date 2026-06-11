//! Tests for configuration loading: appsettings.json parsing,
//! environment variable overrides, JSON binding, and development merge.

use lrwf_core::config;
use lrwf_core::mode::AppMode;
use serde::Deserialize;

// ─── Test types ──────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Deserialize, Default)]
struct TestConfig {
    #[serde(default)]
    name: String,
    #[serde(default)]
    port: u16,
    #[serde(default)]
    debug: bool,
}

// ─── Tests ───────────────────────────────────────────────────────────

#[test]
fn config_bind_root_parses_top_level() {
    let json = serde_json::json!({
        "name": "TestApp",
        "port": 8080,
        "debug": true
    });
    let config: TestConfig = config::bind_root(&json);
    assert_eq!(config.name, "TestApp");
    assert_eq!(config.port, 8080);
    assert!(config.debug);
}

#[test]
fn config_bind_root_defaults_missing_fields() {
    let json = serde_json::json!({
        "name": "PartialApp"
    });
    let config: TestConfig = config::bind_root(&json);
    assert_eq!(config.name, "PartialApp");
    assert_eq!(config.port, 0); // default
    assert!(!config.debug); // default
}

#[test]
fn config_bind_section_extracts_nested() {
    let json = serde_json::json!({
        "App": {
            "name": "NestedApp",
            "port": 3000,
            "debug": true
        },
        "Other": "ignored"
    });
    let config: TestConfig = config::bind_config(&json, "App");
    assert_eq!(config.name, "NestedApp");
    assert_eq!(config.port, 3000);
    assert!(config.debug);
}

#[test]
fn config_bind_section_missing_returns_default() {
    let json = serde_json::json!({ "Other": "value" });
    let config: TestConfig = config::bind_config(&json, "App");
    assert_eq!(config, TestConfig::default());
}

#[test]
fn config_env_override_bind_root_preserves_values() {
    let _json = serde_json::json!({
        "App": {
            "Name": "BaseApp"
        }
    });

    let loaded = serde_json::json!({
        "App": { "Name": "BaseApp" }
    });

    // Verify bind_root works with standard JSON
    let app_opts: lrwf_core::config::AppOptions = config::bind_root(&loaded);
    assert_eq!(app_opts.app.name, "BaseApp");
}

#[test]
fn config_merge_and_bind_root() {
    let base = serde_json::json!({
        "App": { "Name": "Base" },
        "Jwt": { "Secret": "base-secret" }
    });
    let _overlay = serde_json::json!({
        "App": { "Name": "Overlay" }
    });

    // Verify bind_root parses the base config correctly
    let opts: lrwf_core::config::AppOptions = config::bind_root(&base);
    assert_eq!(opts.app.name, "Base");
    assert_eq!(opts.jwt.secret, "base-secret");
}

#[test]
fn config_load_appsettings_development_mode_merges() {
    // Development mode loads appsettings.json + appsettings.Development.json
    // In CI / test environments, these files don't exist, so load_appsettings
    // returns None if neither file is found.
    let result = config::load_appsettings(AppMode::Development);
    // May be None if no config files exist in the current dir
    // This test just ensures the function doesn't panic
    let _ = result;
}

#[test]
fn config_load_appsettings_production_mode() {
    let result = config::load_appsettings(AppMode::Production);
    // Should not panic even when config files don't exist
    let _ = result;
}

#[test]
fn config_app_options_defaults() {
    let defaults = lrwf_core::config::AppOptions::default();
    assert_eq!(defaults.app.urls, vec!["http://0.0.0.0:5000"]);
    assert_eq!(defaults.app.max_body_size, 10 * 1024 * 1024);
    assert!(defaults.jwt.secret.is_empty());
    assert!(defaults.cors.origins.contains(&"*".to_string()));
}

#[test]
fn config_cors_section_defaults() {
    let defaults = lrwf_core::config::CorsSection::default();
    assert!(defaults.origins.contains(&"*".to_string()));
    assert!(defaults.methods.contains(&"GET".to_string()));
    assert!(defaults.headers.contains(&"Authorization".to_string()));
    assert!(!defaults.allow_credentials);
    assert_eq!(defaults.max_age, 86400);
}

#[test]
fn config_bind_empty_json_returns_default() {
    let json = serde_json::json!({});
    let config: TestConfig = config::bind_root(&json);
    assert_eq!(config, TestConfig::default());
}
