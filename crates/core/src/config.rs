//! Built-in configuration support: appsettings.json + AppOptions pattern.
//!
//! The framework automatically loads `appsettings.json` (merged with
//! `appsettings.Development.json` in dev mode) and binds it to the
//! built-in `AppOptions` struct.  Users customize options via
//! `HostBuilder::configure(|app| app.useOptions(|o| { ... }))`.

use crate::mode::AppMode;
use serde::Deserialize;
use std::path::Path;

// ---------------------------------------------------------------------------
// IAppOptions trait (for user-defined option types)
// ---------------------------------------------------------------------------

/// Application options — binds to a section of appsettings.json.
///
/// Users define their own structs implementing this trait,
/// then call `AppConfig::bind()` to bind values.
pub trait IAppOptions: for<'de> Deserialize<'de> + Default + Send + Sync + 'static {}

impl<T> IAppOptions for T where T: for<'de> Deserialize<'de> + Default + Send + Sync + 'static {}

// ---------------------------------------------------------------------------
// Built-in option types (matching standard appsettings.json layout)
// ---------------------------------------------------------------------------

/// Top-level application section.
#[derive(Debug, Clone, Deserialize)]
pub struct AppSection {
    /// Application display name.
    #[serde(default, rename = "Name")]
    pub name: String,
    /// Listen addresses as full URLs (e.g., "http://0.0.0.0:5000").
    /// ASP.NET Core compatible. Default: ["http://0.0.0.0:5000"].
    #[serde(default = "default_urls", rename = "Urls")]
    pub urls: Vec<String>,
    /// Maximum request body size in bytes. Default: 10 MB.
    #[serde(default = "default_max_body_size", rename = "MaxBodySize")]
    pub max_body_size: usize,
}

impl Default for AppSection {
    fn default() -> Self {
        Self {
            name: String::new(),
            urls: default_urls(),
            max_body_size: default_max_body_size(),
        }
    }
}

fn default_urls() -> Vec<String> {
    vec!["http://0.0.0.0:5000".to_string()]
}

fn default_max_body_size() -> usize {
    10 * 1024 * 1024 // 10 MB
}

/// JWT authentication section.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct JwtSection {
    /// HMAC secret for signing/verifying JWT tokens.
    #[serde(default, rename = "Secret")]
    pub secret: String,
}

/// CORS (Cross-Origin Resource Sharing) section.
#[derive(Debug, Clone, Deserialize)]
pub struct CorsSection {
    /// Allowed origins. Default: ["*"].
    #[serde(default = "default_origins")]
    pub origins: Vec<String>,
    /// Allowed methods. Default: GET, POST, PUT, DELETE, PATCH, OPTIONS.
    #[serde(default = "default_cors_methods")]
    pub methods: Vec<String>,
    /// Allowed headers. Default: Content-Type, Authorization.
    #[serde(default = "default_cors_headers")]
    pub headers: Vec<String>,
    /// Allow credentials. Default: false.
    #[serde(default)]
    pub allow_credentials: bool,
    /// Preflight cache max-age in seconds. Default: 86400.
    #[serde(default = "default_max_age")]
    pub max_age: u32,
}

impl Default for CorsSection {
    fn default() -> Self {
        Self {
            origins: default_origins(),
            methods: default_cors_methods(),
            headers: default_cors_headers(),
            allow_credentials: false,
            max_age: default_max_age(),
        }
    }
}

fn default_origins() -> Vec<String> {
    vec!["*".to_string()]
}

fn default_cors_methods() -> Vec<String> {
    vec![
        "GET".to_string(),
        "POST".to_string(),
        "PUT".to_string(),
        "DELETE".to_string(),
        "PATCH".to_string(),
        "OPTIONS".to_string(),
    ]
}

fn default_cors_headers() -> Vec<String> {
    vec!["Content-Type".to_string(), "Authorization".to_string()]
}

fn default_max_age() -> u32 {
    86400
}

/// Per-IP rate limiting (token bucket).
#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitSection {
    /// Enable rate-limit middleware. Default: false (enable in Production appsettings).
    #[serde(default, rename = "Enabled")]
    pub enabled: bool,
    /// Sustained requests per second per client IP.
    #[serde(default = "default_rate_limit_rps", rename = "RequestsPerSecond")]
    pub requests_per_second: f64,
    /// Burst capacity before throttling.
    #[serde(default = "default_rate_limit_burst", rename = "BurstSize")]
    pub burst_size: u32,
    /// Maximum distinct client IPs tracked (LRU eviction when exceeded).
    #[serde(default = "default_rate_limit_max_ips", rename = "MaxTrackedIps")]
    pub max_tracked_ips: usize,
}

impl Default for RateLimitSection {
    fn default() -> Self {
        Self {
            enabled: false,
            requests_per_second: default_rate_limit_rps(),
            burst_size: default_rate_limit_burst(),
            max_tracked_ips: default_rate_limit_max_ips(),
        }
    }
}

fn default_rate_limit_rps() -> f64 {
    100.0
}

fn default_rate_limit_burst() -> u32 {
    200
}

fn default_rate_limit_max_ips() -> usize {
    10_000
}

/// HTTP request metrics (`GET /metrics` Prometheus text when enabled).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct MetricsSection {
    #[serde(default, rename = "Enabled")]
    pub enabled: bool,
}

/// TLS (Transport Layer Security) section.
///
/// TLS is activated automatically when the `App.Urls` array
/// contains one or more `https://` entries. The certificate
/// and key paths are read from this section.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TlsSection {
    /// Path to TLS certificate PEM file.
    #[serde(default, rename = "CertPath")]
    pub cert_path: String,
    /// Path to TLS private key PEM file.
    #[serde(default, rename = "KeyPath")]
    pub key_path: String,
}

/// Standard application options loaded from appsettings.json.
///
/// Bound automatically by the framework.  Access via `host.options()`
/// or customize via `app.useOptions(|o| { ... })`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AppOptions {
    /// Application settings.
    #[serde(default, rename = "App")]
    pub app: AppSection,
    /// JWT authentication settings.
    #[serde(default, rename = "Jwt")]
    pub jwt: JwtSection,
    /// CORS settings.
    #[serde(default, rename = "Cors")]
    pub cors: CorsSection,
    /// TLS settings.
    #[serde(default, rename = "Tls")]
    pub tls: TlsSection,
    /// Rate limiting settings.
    #[serde(default, rename = "RateLimit")]
    pub rate_limit: RateLimitSection,
    /// Prometheus-style metrics endpoint settings.
    #[serde(default, rename = "Metrics")]
    pub metrics: MetricsSection,
}

// ---------------------------------------------------------------------------
// Config loading helpers
// ---------------------------------------------------------------------------

/// Load the merged appsettings JSON (base + environment overlay + env overrides).
///
/// 按运行模式自动合并对应的 `appsettings.{Mode}.json` overlay：
/// - `Development` → `appsettings.Development.json`
/// - `Production`  → `appsettings.Production.json`
///
/// Environment variables prefixed with `APP__` override the corresponding JSON values.
/// For example, `APP__App__Address=0.0.0.0:8080` overrides `{"App": {"Address": "..."}}`.
pub fn load_appsettings(mode: AppMode) -> Option<serde_json::Value> {
    let mut base = read_json_file("appsettings.json")?;

    let overlay_name = mode.overlay_filename();
    if let Some(overlay) = read_json_file(overlay_name) {
        merge_json(&mut base, overlay);
    }

    // Well-known env vars (JWT_SECRET) — applied before APP__ so APP__Jwt__Secret wins.
    apply_well_known_env_overrides(&mut base);

    // Apply environment variable overrides (APP__Section__Key pattern)
    apply_env_overrides(&mut base);

    Some(base)
}

/// Apply conventional environment variables that map to config sections.
///
/// `JWT_SECRET` → `Jwt.Secret` (overridden by `APP__Jwt__Secret` if set).
fn apply_well_known_env_overrides(config: &mut serde_json::Value) {
    if let Ok(secret) = std::env::var("JWT_SECRET") {
        if !secret.is_empty() {
            set_json_value(config, &["Jwt", "Secret"], &secret);
        }
    }
}

/// Apply environment variable overrides following the `APP__Section__Key` pattern.
fn apply_env_overrides(config: &mut serde_json::Value) {
    for (key, value) in std::env::vars() {
        if let Some(path) = key.strip_prefix("APP__") {
            // Split by double underscore to get path segments
            let segments: Vec<&str> = path.split("__").collect();
            if segments.is_empty() {
                continue;
            }
            set_json_value(config, &segments, &value);
        }
    }
}

/// Set a value in a JSON object following the given path segments.
fn set_json_value(obj: &mut serde_json::Value, segments: &[&str], value: &str) {
    if segments.is_empty() {
        return;
    }

    let key = segments[0];

    if let serde_json::Value::Object(map) = obj {
        if segments.len() == 1 {
            // Leaf: set the value, attempting to parse as JSON first
            let parsed =
                serde_json::from_str(value).unwrap_or(serde_json::Value::String(value.to_string()));
            map.insert(key.to_string(), parsed);
        } else if let Some(child) = map.get_mut(key) {
            // Recurse into child
            set_json_value(child, &segments[1..], value);
        } else {
            // Create intermediate objects as needed
            let mut child = serde_json::json!({});
            set_json_value(&mut child, &segments[1..], value);
            map.insert(key.to_string(), child);
        }
    }
}

/// Bind a section of the config JSON to a deserializable type.
pub fn bind_config<T: for<'de> Deserialize<'de> + Default>(
    config: &serde_json::Value,
    section: &str,
) -> T {
    if section.is_empty() || section == "." {
        serde_json::from_value(config.clone()).unwrap_or_default()
    } else {
        config
            .get(section)
            .map(|v| serde_json::from_value(v.clone()).unwrap_or_default())
            .unwrap_or_default()
    }
}

/// Bind the entire config JSON to a type (for root-level deserialization).
pub fn bind_root<T: for<'de> Deserialize<'de> + Default>(config: &serde_json::Value) -> T {
    serde_json::from_value(config.clone()).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn read_json_file(path: impl AsRef<Path>) -> Option<serde_json::Value> {
    let path = path.as_ref();

    /// Try to read and parse a JSON file at the given path.
    fn try_read(path: &Path) -> Option<serde_json::Value> {
        let content = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    // 1. 应用基准目录（exe 同级 / cwd / 上溯统一由 app_base 处理）。
    //    部署与开发期均能定位到 appsettings.json 所在目录。
    let base = crate::paths::app_base();
    let candidate = base.join(path);
    if let Some(value) = try_read(&candidate) {
        return Some(value);
    }

    // 2. Try as-is (relative to current working directory) — 兜底
    if let Some(value) = try_read(path) {
        return Some(value);
    }

    None
}

fn merge_json(base: &mut serde_json::Value, overlay: serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            for (k, v) in b {
                merge_json(a.entry(k).or_insert(serde_json::Value::Null), v);
            }
        }
        (a, b) => *a = b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_env_jwt_and_app_override_precedence() {
        std::env::set_var("JWT_SECRET", "env-jwt-secret-that-is-long-enough-32");
        let mut config = serde_json::json!({ "Jwt": { "Secret": "from-file" } });
        apply_well_known_env_overrides(&mut config);
        assert_eq!(
            config["Jwt"]["Secret"],
            "env-jwt-secret-that-is-long-enough-32"
        );

        std::env::set_var(
            "APP__Jwt__Secret",
            "from-app-override-secret-32chars-minimum",
        );
        apply_env_overrides(&mut config);
        std::env::remove_var("JWT_SECRET");
        std::env::remove_var("APP__Jwt__Secret");
        assert_eq!(
            config["Jwt"]["Secret"],
            "from-app-override-secret-32chars-minimum"
        );
    }
}
