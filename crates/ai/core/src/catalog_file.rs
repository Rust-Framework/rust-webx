//! Hot-reload a model catalog from a JSON manifest.
//!
//! A [`ModelCatalog`] embeds live provider objects, so it cannot be serialized
//! wholesale. The manifest therefore describes only the data an operator changes
//! at runtime — models, capabilities and aliases — and references providers that
//! already exist in the running catalog:
//!
//! ```json
//! {
//!   "version": 7,
//!   "models": [
//!     { "provider": "openai", "model": "gpt-4o-mini", "context_window": 128000,
//!       "capabilities": { "streaming": true, "tools": true } }
//!   ],
//!   "aliases": [
//!     { "name": "fast", "strategy": "fallback",
//!       "targets": [ { "provider": "openai", "model": "gpt-4o-mini" } ] }
//!   ]
//! }
//! ```
//!
//! [`CatalogFileWatcher`] polls the file, builds a new immutable snapshot and
//! calls [`CatalogHandle::reload`]; a bad edit is reported and the old snapshot
//! stays active. `version` must increase when present; when omitted the watcher
//! uses `current + 1`, so every changed file is applied exactly once.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;

use crate::provider::{
    Alias, CatalogHandle, ModelCatalog, ModelKey, ModelSpec, ProviderCapabilities, RouteStrategy,
    RouteTarget,
};
use webx_core::hosted::PeriodicBackgroundService;

/// Manifest parse/validation failure.
#[derive(Debug, Error)]
pub enum CatalogFileError {
    /// The file could not be read.
    #[error("cannot read catalog file {path}: {detail}")]
    Io {
        /// Manifest path.
        path: String,
        /// Underlying error.
        detail: String,
    },
    /// The file is not valid JSON.
    #[error("invalid catalog JSON: {0}")]
    Json(String),
    /// The manifest references unknown data or would roll back.
    #[error("invalid catalog manifest: {0}")]
    Validation(String),
}

/// Capability flags as they appear in the manifest.
#[derive(Debug, Clone, Default, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct CapabilityFlags {
    /// Streamed responses.
    pub streaming: bool,
    /// Tool/function calling.
    pub tools: bool,
    /// Image input.
    pub vision: bool,
    /// JSON mode / structured output.
    pub json_mode: bool,
    /// Exposed reasoning content.
    pub reasoning: bool,
    /// Embeddings endpoint.
    pub embeddings: bool,
    /// Image generation.
    pub images: bool,
    /// Audio transcription/speech.
    pub audio: bool,
}

impl From<CapabilityFlags> for ProviderCapabilities {
    fn from(flags: CapabilityFlags) -> Self {
        ProviderCapabilities {
            streaming: flags.streaming,
            tools: flags.tools,
            vision: flags.vision,
            json_mode: flags.json_mode,
            reasoning: flags.reasoning,
            embeddings: flags.embeddings,
            images: flags.images,
            audio: flags.audio,
        }
    }
}

/// One model declaration.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestModel {
    /// Existing provider id.
    pub provider: String,
    /// Upstream model id.
    pub model: String,
    /// Capability flags (all false when omitted).
    #[serde(default)]
    pub capabilities: CapabilityFlags,
    /// Context window in tokens; `0` = unknown.
    #[serde(default)]
    pub context_window: u32,
    /// Whether the model may be selected.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// One alias candidate.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestTarget {
    /// Provider id.
    pub provider: String,
    /// Upstream model id.
    pub model: String,
    /// Routing weight (higher first for `weighted`).
    #[serde(default = "default_weight")]
    pub weight: u32,
}

fn default_weight() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

/// One public alias declaration.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestAlias {
    /// Public model name.
    pub name: String,
    /// `priority` (default), `fallback`, `weighted`, `least_latency`, `capability`.
    #[serde(default)]
    pub strategy: ManifestStrategy,
    /// Ordered candidates.
    #[serde(default)]
    pub targets: Vec<ManifestTarget>,
}

/// Strategy name in the manifest.
#[derive(Debug, Clone, Copy, Default, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManifestStrategy {
    /// Catalog order.
    #[default]
    Priority,
    /// Catalog order with explicit fallback semantics.
    Fallback,
    /// Weight order.
    Weighted,
    /// Observed-latency order.
    LeastLatency,
    /// Capability-filtered order.
    Capability,
}

impl From<ManifestStrategy> for RouteStrategy {
    fn from(strategy: ManifestStrategy) -> Self {
        match strategy {
            ManifestStrategy::Priority => RouteStrategy::Priority,
            ManifestStrategy::Fallback => RouteStrategy::Fallback,
            ManifestStrategy::Weighted => RouteStrategy::Weighted,
            ManifestStrategy::LeastLatency => RouteStrategy::LeastLatency,
            ManifestStrategy::Capability => RouteStrategy::Capability,
        }
    }
}

/// Serializable catalog manifest (providers stay in code).
#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CatalogManifest {
    /// Monotonic version. Omit to let the watcher use `current + 1`.
    pub version: Option<u64>,
    /// Models to publish (the manifest replaces the model set).
    pub models: Vec<ManifestModel>,
    /// Aliases to publish (the manifest replaces the alias set).
    pub aliases: Vec<ManifestAlias>,
}

impl CatalogManifest {
    /// Build the next snapshot from `base` (providers are reused by id).
    ///
    /// Validation is exhaustive before anything is published: unknown provider
    /// ids, duplicate model/alias names, empty aliases, unknown alias targets and
    /// a non-increasing explicit version all fail here.
    pub fn build(&self, base: &ModelCatalog) -> Result<Arc<ModelCatalog>, CatalogFileError> {
        let version = match self.version {
            Some(version) if version > base.version => version,
            Some(version) => {
                return Err(CatalogFileError::Validation(format!(
                    "version {version} must be greater than current version {}",
                    base.version
                )))
            }
            None => base.version.saturating_add(1),
        };

        let mut models = std::collections::HashMap::new();
        for entry in &self.models {
            if !base.providers.contains_key(&entry.provider) {
                return Err(CatalogFileError::Validation(format!(
                    "model {}/{} references unknown provider {}",
                    entry.provider, entry.model, entry.provider
                )));
            }
            let key = ModelKey::new(entry.provider.clone(), entry.model.clone());
            if models.contains_key(&key) {
                return Err(CatalogFileError::Validation(format!(
                    "duplicate model {}/{}",
                    entry.provider, entry.model
                )));
            }
            models.insert(
                key.clone(),
                ModelSpec {
                    key,
                    capabilities: entry.capabilities.clone().into(),
                    context_window: entry.context_window,
                    enabled: entry.enabled,
                },
            );
        }

        let mut aliases = std::collections::HashMap::new();
        for entry in &self.aliases {
            if entry.name.trim().is_empty() {
                return Err(CatalogFileError::Validation(
                    "alias name must not be empty".to_string(),
                ));
            }
            if aliases.contains_key(&entry.name) {
                return Err(CatalogFileError::Validation(format!(
                    "duplicate alias {}",
                    entry.name
                )));
            }
            if entry.targets.is_empty() {
                return Err(CatalogFileError::Validation(format!(
                    "alias {} has no targets",
                    entry.name
                )));
            }
            let mut targets = Vec::with_capacity(entry.targets.len());
            for target in &entry.targets {
                let key = ModelKey::new(target.provider.clone(), target.model.clone());
                if !models.contains_key(&key) {
                    return Err(CatalogFileError::Validation(format!(
                        "alias {} targets unknown model {}/{}",
                        entry.name, target.provider, target.model
                    )));
                }
                targets.push(RouteTarget::new(key).with_weight(target.weight));
            }
            aliases.insert(
                entry.name.clone(),
                Alias::new(entry.strategy.into(), targets),
            );
        }

        Ok(Arc::new(ModelCatalog {
            version,
            providers: base.providers.clone(),
            models,
            aliases,
        }))
    }
}

/// Poll a JSON manifest and publish new catalog snapshots.
pub struct CatalogFileWatcher {
    path: PathBuf,
    handle: CatalogHandle,
    interval: Duration,
    last_hash: Mutex<Option<u64>>,
}

impl Clone for CatalogFileWatcher {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            handle: self.handle.clone(),
            interval: self.interval,
            last_hash: Mutex::new(*self.last_hash.lock().unwrap_or_else(|p| p.into_inner())),
        }
    }
}

impl CatalogFileWatcher {
    /// Watch `path` for `handle`.
    pub fn new(path: impl Into<PathBuf>, handle: CatalogHandle, interval: Duration) -> Self {
        Self {
            path: path.into(),
            handle,
            interval,
            last_hash: Mutex::new(None),
        }
    }

    /// The watched path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read/validate/reload now.
    ///
    /// Returns `Ok(false)` when the file is absent or unchanged. A validation
    /// error leaves the current snapshot untouched; the next changed file
    /// retries.
    pub fn reload_now(&self) -> Result<bool, CatalogFileError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(CatalogFileError::Io {
                    path: self.path.display().to_string(),
                    detail: error.to_string(),
                })
            }
        };

        let hash = content_hash(&bytes);
        {
            let mut last = self.last_hash.lock().unwrap_or_else(|p| p.into_inner());
            if *last == Some(hash) {
                return Ok(false);
            }
            *last = Some(hash);
        }

        let manifest: CatalogManifest = serde_json::from_slice(&bytes)
            .map_err(|error| CatalogFileError::Json(error.to_string()))?;
        let next = manifest.build(&self.handle.current())?;
        let applied = self.handle.reload(next);
        if !applied {
            return Err(CatalogFileError::Validation(
                "catalog version did not increase".to_string(),
            ));
        }
        tracing::info!(
            path = %self.path.display(),
            version = self.handle.version(),
            "model catalog reloaded"
        );
        Ok(true)
    }
}

#[async_trait::async_trait]
impl PeriodicBackgroundService for CatalogFileWatcher {
    fn interval(&self) -> Duration {
        self.interval
    }

    async fn tick(&self) -> webx_core::error::Result<()> {
        self.reload_now().map(|_| ()).map_err(|error| {
            webx_core::error::Error::Validation(format!("catalog reload failed: {error}"))
        })
    }
}

fn content_hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{CanonicalRequest, CanonicalResponse};
    use crate::provider::{ChatProvider, ProviderError};

    struct MockProvider(&'static str);

    #[async_trait::async_trait]
    impl ChatProvider for MockProvider {
        fn id(&self) -> &str {
            self.0
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::chat_stream()
        }

        async fn complete(
            &self,
            _request: CanonicalRequest,
        ) -> Result<CanonicalResponse, ProviderError> {
            Err(ProviderError::Unsupported("mock".into()))
        }
    }

    fn base_catalog() -> Arc<ModelCatalog> {
        let mut catalog = ModelCatalog::new(3);
        catalog.add_provider(Arc::new(MockProvider("p")));
        catalog.add_model(ModelSpec::new(ModelKey::new("p", "base")));
        catalog.add_alias(
            "old",
            Alias::new(
                RouteStrategy::Fallback,
                vec![RouteTarget::new(ModelKey::new("p", "base"))],
            ),
        );
        Arc::new(catalog)
    }

    fn manifest() -> CatalogManifest {
        CatalogManifest {
            version: Some(9),
            models: vec![ManifestModel {
                provider: "p".into(),
                model: "fast".into(),
                capabilities: CapabilityFlags {
                    streaming: true,
                    tools: true,
                    ..Default::default()
                },
                context_window: 128_000,
                enabled: true,
            }],
            aliases: vec![ManifestAlias {
                name: "fast".into(),
                strategy: ManifestStrategy::Weighted,
                targets: vec![ManifestTarget {
                    provider: "p".into(),
                    model: "fast".into(),
                    weight: 5,
                }],
            }],
        }
    }

    #[test]
    fn builds_a_new_snapshot_reusing_providers() {
        let base = base_catalog();
        let catalog = manifest().build(&base).expect("manifest builds");
        assert_eq!(catalog.version, 9);
        assert_eq!(catalog.providers.len(), 1);
        assert_eq!(catalog.models.len(), 1);
        assert!(catalog.models.contains_key(&ModelKey::new("p", "fast")));
        assert_eq!(catalog.aliases["fast"].strategy, RouteStrategy::Weighted);
        assert_eq!(catalog.aliases["fast"].targets[0].weight, 5);
        assert_eq!(catalog.aliases.len(), 1, "the manifest replaces aliases");
    }

    #[test]
    fn rejects_unknown_providers_targets_and_version_rollback() {
        let base = base_catalog();

        let mut unknown_provider = manifest();
        unknown_provider.models[0].provider = "missing".into();
        assert!(matches!(
            unknown_provider.build(&base),
            Err(CatalogFileError::Validation(message)) if message.contains("unknown provider")
        ));

        let mut unknown_target = manifest();
        unknown_target.aliases[0].targets[0].model = "missing".into();
        assert!(matches!(
            unknown_target.build(&base),
            Err(CatalogFileError::Validation(message)) if message.contains("unknown model")
        ));

        let mut rollback = manifest();
        rollback.version = Some(2);
        assert!(matches!(
            rollback.build(&base),
            Err(CatalogFileError::Validation(message)) if message.contains("greater")
        ));

        let mut empty_alias = manifest();
        empty_alias.aliases[0].targets.clear();
        assert!(matches!(
            empty_alias.build(&base),
            Err(CatalogFileError::Validation(message)) if message.contains("no targets")
        ));
    }

    #[test]
    fn omitted_version_uses_current_plus_one() {
        let base = base_catalog();
        let mut next = manifest();
        next.version = None;
        let catalog = next.build(&base).expect("builds");
        assert_eq!(catalog.version, 4);
    }

    #[test]
    fn watcher_applies_changed_files_and_keeps_the_old_snapshot_on_errors() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("catalog.json");
        let handle = CatalogHandle::new(base_catalog());
        let watcher = CatalogFileWatcher::new(&path, handle.clone(), Duration::from_millis(50));

        std::fs::write(&path, serde_json::to_vec(&manifest()).expect("json")).expect("write");
        assert!(watcher.reload_now().expect("applies"));
        assert_eq!(handle.version(), 9);
        assert!(!watcher.reload_now().expect("unchanged"));

        std::fs::write(&path, b"{ not json").expect("write");
        assert!(matches!(
            watcher.reload_now(),
            Err(CatalogFileError::Json(_))
        ));
        assert_eq!(handle.version(), 9, "a bad edit must not drop the catalog");

        let mut next = manifest();
        next.version = Some(10);
        next.aliases[0].targets[0].weight = 9;
        std::fs::write(&path, serde_json::to_vec(&next).expect("json")).expect("write");
        assert!(watcher.reload_now().expect("applies fixed file"));
        assert_eq!(handle.version(), 10);
        assert_eq!(handle.current().aliases["fast"].targets[0].weight, 9);
    }

    #[tokio::test]
    async fn tick_applies_the_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("catalog.json");
        let handle = CatalogHandle::new(base_catalog());
        std::fs::write(&path, serde_json::to_vec(&manifest()).expect("json")).expect("write");
        let watcher = CatalogFileWatcher::new(&path, handle.clone(), Duration::from_millis(10));
        watcher.tick().await.expect("tick");
        assert_eq!(handle.version(), 9);
    }
}
