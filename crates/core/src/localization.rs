//! Localized endpoint text: resource references resolved against the host's
//! configured culture.
//!
//! The framework follows the .NET model: metadata stores a **resource key**
//! (optionally with a literal fallback), translations live in resource files,
//! and the active culture is a host-wide setting. Nothing in a route declaration
//! enumerates languages.
//!
//! ```ignore
//! #[get("/api/blog")]
//! #[display(key = "endpoint.blog.list.name", text = "博客模块-博客列表")]
//! impl IRequest<Vec<BlogModel>> for ListBlogRequest {}
//! ```
//!
//! Resources are registered at startup. The conventional layout needs one call:
//!
//! ```ignore
//! Host::builder().add_localization().build();  // reads locales/<culture>.json
//! ```
//!
//! For a single-file deployment, compile them in:
//!
//! ```ignore
//! Host::builder()
//!     .add_localization_with(
//!         LocalizationOptions::none()
//!             .resources("zh-CN", include_str!("../locales/zh-CN.json"))
//!             .resources("en", include_str!("../locales/en.json")),
//!     )
//!     .build();
//! ```
//!
//! Lookup follows .NET's resource fallback: exact culture, its parent cultures,
//! then the default culture, then the literal fallback, then the key itself.

use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A reference to localized text.
///
/// `key` is a resource key (`"endpoint.blog.list.name"`); `text` is a literal
/// default used when the key is absent from every resource. Either may be empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TextRef {
    /// Resource key; empty for literal-only text.
    pub key: &'static str,

    /// Literal fallback; empty when the key is the only source.
    pub text: &'static str,
}

impl TextRef {
    /// A key-only reference; a missing resource falls back to the key itself.
    pub const fn key(key: &'static str) -> Self {
        Self { key, text: "" }
    }

    /// Literal-only text; never looks at resources.
    pub const fn text(text: &'static str) -> Self {
        Self { key: "", text }
    }

    /// A key with a literal fallback.
    pub const fn new(key: &'static str, text: &'static str) -> Self {
        Self { key, text }
    }
}

/// The result of resolving a [`TextRef`].
///
/// Mirrors .NET's `LocalizedString`: `resource_not_found` is true when the key
/// was not found in any resource and `value` is the key itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalizedString {
    /// The resolved text.
    pub value: String,

    /// True when `value` is the unresolved key.
    pub resource_not_found: bool,
}

impl LocalizedString {
    /// A successfully resolved value (resource hit or literal).
    pub fn found(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            resource_not_found: false,
        }
    }

    /// A key that was not found in any resource.
    pub fn missing(key: &str) -> Self {
        Self {
            value: key.to_string(),
            resource_not_found: true,
        }
    }
}

/// Resolves resource keys for a culture.
///
/// The default implementation of [`Self::get`] and [`Self::resolve`] uses
/// [`Self::default_culture`], which the host sets from `Culture.Default`.
/// Pass an explicit culture only for cross-culture output (for example a
/// per-culture report); request handling should use the configured default.
pub trait IStringLocalizer: Send + Sync {
    /// Culture used when no explicit culture is supplied.
    fn default_culture(&self) -> &str;

    /// Resolve `key` for `culture`, falling back through parent cultures to the
    /// default culture, then returning the key with `resource_not_found`.
    fn get_in(&self, culture: &str, key: &str) -> LocalizedString;

    /// Resolve `key` for the default culture.
    fn get(&self, key: &str) -> LocalizedString {
        self.get_in(self.default_culture(), key)
    }

    /// Resolve a metadata text reference for `culture`.
    fn resolve_in(&self, culture: &str, text: TextRef) -> LocalizedString {
        if !text.key.is_empty() {
            let found = self.get_in(culture, text.key);
            if !found.resource_not_found {
                return found;
            }
            if !text.text.is_empty() {
                return LocalizedString::found(text.text);
            }
            return found;
        }
        LocalizedString::found(text.text)
    }

    /// Resolve a metadata text reference for the default culture.
    fn resolve(&self, text: TextRef) -> LocalizedString {
        self.resolve_in(self.default_culture(), text)
    }
}

/// Resource files registered with the host.
///
/// Two sources, merged in this order (later wins):
///
/// 1. a resource directory of `<culture>.json` files, relative to the
///    application base (`locales/` by default for `add_localization()`), read at
///    startup so operators can adjust translations without a rebuild;
/// 2. explicit `(culture, json)` entries compiled into the binary with
///    `include_str!` — the single-executable form.
#[derive(Debug, Clone)]
pub struct LocalizationOptions {
    resource_dir: Option<PathBuf>,
    resources: Vec<(&'static str, &'static str)>,
}

impl Default for LocalizationOptions {
    /// The conventional layout: `<culture>.json` files under `locales/`.
    fn default() -> Self {
        Self {
            resource_dir: Some(PathBuf::from("locales")),
            resources: Vec::new(),
        }
    }
}

impl LocalizationOptions {
    /// The conventional layout (`locales/<culture>.json`) with no compiled-in
    /// entries. This is what `HostBuilder::add_localization()` uses.
    pub fn new() -> Self {
        Self::default()
    }

    /// No resource directory and no resources.
    ///
    /// Use it for explicit-only resources
    /// (`LocalizationOptions::none().resources(...)`) or to disable localization.
    pub fn none() -> Self {
        Self {
            resource_dir: None,
            resources: Vec::new(),
        }
    }

    /// Read `<culture>.json` files from `dir`.
    ///
    /// Relative paths resolve against the application base, like
    /// `appsettings.json`. [`Self::new`] sets this to `"locales"`.
    pub fn resource_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.resource_dir = Some(dir.into());
        self
    }

    /// Register one culture's resources.
    ///
    /// Registering the same culture twice merges the objects; later entries win
    /// on duplicate keys. Explicit entries override files from the resource
    /// directory.
    pub fn resources(mut self, culture: &'static str, json: &'static str) -> Self {
        self.resources.push((culture, json));
        self
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
        self.resources.iter().copied()
    }
}

/// The framework's default [`IStringLocalizer`]: static JSON resource tables.
pub struct StringLocalizer {
    default_culture: String,
    resources: HashMap<String, HashMap<String, String>>,
}

impl StringLocalizer {
    /// Build the localizer from the configured resource directory and
    /// compiled-in resource entries.
    ///
    /// Returns an error naming the file or culture when a resource is not a
    /// JSON object of `string → string`.
    pub fn new(
        default_culture: impl Into<String>,
        options: &LocalizationOptions,
    ) -> Result<Self, String> {
        let mut resources: HashMap<String, HashMap<String, String>> = HashMap::new();

        if let Some(dir) = &options.resource_dir {
            for (culture, json) in read_resource_dir(dir)? {
                insert_resources(&mut resources, &culture, &json)?;
            }
        }

        for (culture, json) in options.iter() {
            insert_resources(&mut resources, culture, json)?;
        }

        Ok(Self {
            default_culture: default_culture.into(),
            resources,
        })
    }

    fn lookup(&self, culture: &str, key: &str) -> Option<&str> {
        self.resources
            .get(&culture.to_ascii_lowercase())?
            .get(key)
            .map(String::as_str)
    }
}

fn insert_resources(
    resources: &mut HashMap<String, HashMap<String, String>>,
    culture: &str,
    json: &str,
) -> Result<(), String> {
    let parsed: HashMap<String, String> =
        serde_json::from_str(json).map_err(|error| format!("culture '{culture}': {error}"))?;
    resources
        .entry(culture.to_ascii_lowercase())
        .or_default()
        .extend(parsed);
    Ok(())
}

/// Read `<culture>.json` from `dir`, relative to the application base.
///
/// A missing directory is not an error; it means no resources were deployed.
fn read_resource_dir(dir: &Path) -> Result<Vec<(String, String)>, String> {
    let base = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        crate::paths::app_base().join(dir)
    };

    let entries = match std::fs::read_dir(&base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", base.display())),
    };

    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", base.display()))?;
        let path = entry.path();
        if !path.is_file()
            || path.extension().and_then(|extension| extension.to_str()) != Some("json")
        {
            continue;
        }
        let Some(culture) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let json = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        files.push((culture.to_string(), json));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

impl IStringLocalizer for StringLocalizer {
    fn default_culture(&self) -> &str {
        &self.default_culture
    }

    fn get_in(&self, culture: &str, key: &str) -> LocalizedString {
        for candidate in culture_chain(culture) {
            if let Some(value) = self.lookup(candidate, key) {
                return LocalizedString::found(value);
            }
        }
        // A key missing from the requested culture falls back to the host's
        // default culture, like neutral resources in ResourceManager.
        for candidate in culture_chain(&self.default_culture) {
            if let Some(value) = self.lookup(candidate, key) {
                return LocalizedString::found(value);
            }
        }
        LocalizedString::missing(key)
    }
}

/// `zh-Hans-CN` → `["zh-Hans-CN", "zh-Hans", "zh"]`.
fn culture_chain(culture: &str) -> Vec<&str> {
    let mut chain = Vec::new();
    let mut current = culture;
    while !current.is_empty() {
        chain.push(current);
        match current.rfind(['-', '_']) {
            Some(index) => current = &current[..index],
            None => break,
        }
    }
    chain
}
