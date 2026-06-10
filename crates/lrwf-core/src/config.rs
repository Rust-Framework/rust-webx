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
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AppSection {
    /// Application display name.
    #[serde(default, rename = "Name")]
    pub name: String,
    /// Listen address (e.g., "0.0.0.0:5000").
    #[serde(default = "default_address", rename = "Address")]
    pub address: String,
}

fn default_address() -> String {
    "0.0.0.0:5000".to_string()
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
    .into_iter()
    .map(String::from)
    .collect()
}

fn default_cors_headers() -> Vec<String> {
    vec!["Content-Type".to_string(), "Authorization".to_string()]
        .into_iter()
        .map(String::from)
        .collect()
}

fn default_max_age() -> u32 {
    86400
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
}

// ---------------------------------------------------------------------------
// Config loading helpers
// ---------------------------------------------------------------------------

/// Load the merged appsettings JSON (base + Development overlay).
pub fn load_appsettings(mode: AppMode) -> Option<serde_json::Value> {
    let mut base = read_json_file("appsettings.json")?;

    if mode == AppMode::Development {
        if let Some(dev) = read_json_file("appsettings.Development.json") {
            merge_json(&mut base, dev);
        }
    }

    Some(base)
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
    let content = std::fs::read_to_string(path.as_ref()).ok()?;
    serde_json::from_str(&content).ok()
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
