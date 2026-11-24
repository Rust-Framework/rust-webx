//! Secret storage: resolve credentials without putting them in config files.
//!
//! A provider key, webhook token or upstream password belongs in an environment
//! variable, a mounted file or a KMS-backed store —not in `appsettings.json`
//! and not in a `Debug` log line. This module defines the port and two small
//! implementations:
//!
//! * [`EnvSecretStore`] reads the environment (container secrets, local dev);
//! * [`FileSecretStore`] reads one file per secret under a directory (Docker /
//!   Kubernetes secret mounts), rejecting traversal, symlinks, empty files and
//!   group/world-readable files on Unix;
//! * [`ChainSecretStore`] tries several stores in order.
//!
//! [`SecretValue`] is not `Debug`-printable (the redacted form is used instead)
//! and zeroizes its buffer on drop. Production envelope encryption / KMS and key
//! rotation implement [`ISecretStore`] without changing callers.
//!
//! ```ignore
//! let secret = store.get("OPENAI_API_KEY").await?;
//! let provider = OpenAiProvider::new("openai", client, secret);
//! ```

use async_trait::async_trait;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use zeroize::Zeroize;

/// Secret lookup failure. Details stay server-side (they may contain paths).
///
/// # Error mapping
///
/// `?` works in any function returning [`crate::error::Error`] (a handler, a
/// hosted service): the [`From`] implementation below maps `NotFound` to a
/// hidden 500 and `Unavailable` to a retryable 503. The secret **name** and the
/// store's reason are logged for the operator and never placed in the returned
/// error, whose `Display`/`public_detail()` may end up in a response. The
/// secret **value** is never logged by this crate.
#[derive(Debug, Error)]
pub enum SecretError {
    /// The store does not have a secret with this name.
    #[error("secret `{0}` not found")]
    NotFound(String),
    /// The store exists but could not return the secret.
    #[error("secret `{name}` unavailable: {detail}")]
    Unavailable {
        /// Requested secret name.
        name: String,
        /// Operator-facing reason (never the secret itself).
        detail: String,
    },
}

impl SecretError {
    /// Whether the failure means "try the next store in a chain".
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound(_))
    }
}

/// Client-facing detail for a transient secret-store failure.
///
/// Fixed on purpose: neither the secret name nor the store's reason may appear
/// in a response. Retrying the request can succeed once the store recovers,
/// which is why the mapped status is 503.
const SECRET_UNAVAILABLE_DETAIL: &str = "secret resolution is temporarily unavailable; retry later";

/// Convert a secret failure into the framework error type.
///
/// * `NotFound` means the deployment is missing a secret (a key that was never
///   provisioned, an empty file): a server-side defect, so 500 with no public
///   detail. `NotFound` also drives chain fall-through, so only the last error
///   usually reaches a handler.
/// * `Unavailable` means the store could not answer right now: 503 with a
///   fixed, retryable detail.
///
/// The secret name and the store reason are logged here (operator diagnostics)
/// and intentionally left out of the value returned to the caller.
impl From<SecretError> for crate::error::Error {
    fn from(error: SecretError) -> Self {
        match error {
            SecretError::NotFound(name) => {
                tracing::error!(secret = %name, "secret lookup failed: not found");
                crate::error::Error::Internal("secret is not available".to_string())
            }
            SecretError::Unavailable { name, detail } => {
                tracing::error!(secret = %name, reason = %detail, "secret store is unavailable");
                crate::error::Error::Status(503, SECRET_UNAVAILABLE_DETAIL.to_string())
            }
        }
    }
}

/// A secret value that is redacted in `Debug` and zeroized on drop.
///
/// There is deliberately no `Clone`: cloning a secret multiplies the copies
/// that must be zeroized. Share it through an `Arc` when several callers need
/// the same value.
pub struct SecretValue(String);

impl SecretValue {
    /// Wrap a secret string.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the secret (use it at the call site; do not log it).
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether the secret is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue(<redacted>)")
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl From<String> for SecretValue {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for SecretValue {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// A reference to a secret by name (and optional version).
///
/// Store this in provider configuration instead of the plaintext; rotating the
/// secret means publishing a new version, not editing every config row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRef {
    name: String,
    version: Option<String>,
}

impl SecretRef {
    /// A reference to the latest version of `name`.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: None,
        }
    }

    /// Pin a specific version.
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// The secret name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The pinned version, if any.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }
}

/// Resolve a secret by name.
///
/// The port is async so a KMS/Vault implementation can do network I/O; the
/// built-in env/file stores complete without awaiting anything.
#[async_trait]
pub trait ISecretStore: Send + Sync {
    /// Fetch `name`; `NotFound` lets a chain fall through to the next store.
    async fn get(&self, name: &str) -> Result<SecretValue, SecretError>;
}

/// Read secrets from environment variables.
#[derive(Debug, Default, Clone, Copy)]
pub struct EnvSecretStore;

impl EnvSecretStore {
    /// A store over the process environment.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ISecretStore for EnvSecretStore {
    async fn get(&self, name: &str) -> Result<SecretValue, SecretError> {
        match std::env::var(name) {
            Ok(value) if !value.is_empty() => Ok(SecretValue::new(value)),
            Ok(_) => Err(SecretError::NotFound(name.to_string())),
            Err(std::env::VarError::NotPresent) => Err(SecretError::NotFound(name.to_string())),
            Err(std::env::VarError::NotUnicode(_)) => Err(SecretError::Unavailable {
                name: name.to_string(),
                detail: "environment value is not valid UTF-8".to_string(),
            }),
        }
    }
}

/// Read one file per secret under a root directory.
///
/// The name must be a plain file name (`[A-Za-z0-9._-]`, no `..` or separators).
/// A trailing `\r`/`\n` from `echo`/mounted files is stripped; the rest of the
/// content is used verbatim. On Unix the file must not be group/world readable
/// (`0600`), which mirrors the JWT secret-file rule.
#[derive(Debug, Clone)]
pub struct FileSecretStore {
    root: PathBuf,
}

/// Whether an open error means "path is a symlink". On Unix, `O_NOFOLLOW`
/// yields `ELOOP`; elsewhere there is no nofollow flag, so no error maps here.
#[cfg(unix)]
fn is_symlink_error(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::ELOOP)
}

#[cfg(not(unix))]
fn is_symlink_error(_error: &std::io::Error) -> bool {
    false
}

impl FileSecretStore {
    /// Read secrets beneath `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The directory this store reads from.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path_for(&self, name: &str) -> Result<PathBuf, SecretError> {
        let valid = !name.is_empty()
            && name != "."
            && name != ".."
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
        if !valid {
            return Err(SecretError::Unavailable {
                name: name.to_string(),
                detail: "secret name must be a plain file name".to_string(),
            });
        }
        Ok(self.root.join(name))
    }
}

#[async_trait]
impl ISecretStore for FileSecretStore {
    async fn get(&self, name: &str) -> Result<SecretValue, SecretError> {
        let path = self.path_for(name)?;
        let unavailable = |detail: String| SecretError::Unavailable {
            name: name.to_string(),
            detail,
        };

        // On platforms without O_NOFOLLOW there is no race-free way to reject
        // symlinks, but a `symlink_metadata` check is still strictly better than
        // following arbitrary links out of the secret directory (Windows
        // junctions/symlinks need no admin rights). Keep the original guard.
        // On Unix the check below is done race-free on the opened fd instead.
        #[cfg(not(unix))]
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(unavailable("secret file must not be a symlink".into()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(SecretError::NotFound(name.to_string()));
            }
            Err(error) => {
                return Err(unavailable(format!(
                    "cannot stat {}: {error}",
                    path.display()
                )));
            }
            Ok(_) => {}
        }

        // Open the file once and validate the *opened handle*, never a path
        // that could be swapped between a separate stat() and read() (TOCTOU).
        #[cfg(unix)]
        let open_result = {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)
        };
        #[cfg(not(unix))]
        let open_result = std::fs::File::open(&path);

        let file = match open_result {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(SecretError::NotFound(name.to_string()));
            }
            // ELOOP on Unix means the path is a symlink rejected by O_NOFOLLOW.
            Err(error) if is_symlink_error(&error) => {
                return Err(unavailable("secret file must not be a symlink".into()));
            }
            Err(error) => {
                return Err(unavailable(format!(
                    "cannot open {}: {error}",
                    path.display()
                )))
            }
        };

        // Permissions are checked on the already-open fd, so a race cannot swap
        // in a world-readable file after validation.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            match file.metadata() {
                Ok(meta) if meta.permissions().mode() & 0o077 != 0 => {
                    return Err(unavailable("secret file permissions must be 0600".into()));
                }
                Err(error) => {
                    return Err(unavailable(format!(
                        "cannot stat {}: {error}",
                        path.display()
                    )));
                }
                _ => {}
            }
        }

        use std::io::Read;
        let mut file = file;
        let mut raw = Vec::new();
        file.read_to_end(&mut raw)
            .map_err(|error| unavailable(format!("cannot read {}: {error}", path.display())))?;

        // Trim in place so no un-zeroized copy of the secret is left behind.
        while matches!(raw.last(), Some(b'\r') | Some(b'\n')) {
            raw.pop();
        }
        if raw.is_empty() {
            return Err(SecretError::NotFound(name.to_string()));
        }
        let value = String::from_utf8(raw)
            .map_err(|_| unavailable("secret file is not valid UTF-8".into()))?;
        Ok(SecretValue::new(value))
    }
}

/// Ciphertext prefix produced by the framework's AES-GCM protector
/// (`webx1:<key-id>:<base64url(nonce||ciphertext)>`).
pub const PROTECTED_SECRET_PREFIX: &str = "webx1";

/// Encrypt-before-store port for secrets that must live in a database row
/// (TOTP seeds, provider refresh tokens, one-time signing keys).
///
/// The [`ISecretStore`] port answers "where does the key live"; this port
/// answers "how is a value encrypted at rest". A local AES-256-GCM
/// implementation lives in `webx_host::secret_protector`; a KMS/Vault
/// implementation of the same trait changes no call site.
///
/// # Contract
///
/// * `protect` uses the **active** key and returns a self-describing ciphertext
///   (algorithm prefix + key id), so old rows stay decryptable after rotation.
/// * `unprotect` selects the key by the id embedded in the ciphertext and must
///   fail closed: a tampered ciphertext is `Unavailable`, never plaintext.
/// * Neither method ever logs or returns the plaintext in an error detail.
///
/// # Deployment semantics
///
/// * **Single process**: the key ring is loaded once at startup from
///   [`ISecretStore`]; plaintext never enters configuration files.
/// * **Multiple instances**: all nodes must share the same key ring (same
///   secret store), otherwise a row written by one node cannot be read by
///   another. Key rotation = deploy a ring whose active key is new while old
///   keys stay for reads; rows are re-encrypted on their next write.
/// * **Containers**: keys come from mounted/environment secrets at boot; a
///   restart is safe because nothing secret is cached on local disk.
/// * **Performance**: one AEAD operation per protected field (microseconds);
///   protect small values, not blobs. Do not call it while holding a database
///   transaction open on a remote KMS.
#[async_trait]
pub trait ISecretProtector: Send + Sync {
    /// Encrypt `plaintext` with the active key; returns a self-describing ciphertext.
    async fn protect(&self, plaintext: &str) -> Result<String, SecretError>;

    /// Decrypt a ciphertext produced by any retained key.
    async fn unprotect(&self, ciphertext: &str) -> Result<SecretValue, SecretError>;

    /// Active key id (diagnostics and rotation bookkeeping).
    fn active_key_id(&self) -> &str;
}

/// Try each store in order until one returns a value.
pub struct ChainSecretStore {
    stores: Vec<Arc<dyn ISecretStore>>,
}

impl ChainSecretStore {
    /// An empty chain.
    pub fn new() -> Self {
        Self { stores: Vec::new() }
    }

    /// Append a store (chain order = registration order).
    pub fn push(mut self, store: Arc<dyn ISecretStore>) -> Self {
        self.stores.push(store);
        self
    }

    /// Number of stores in the chain.
    pub fn len(&self) -> usize {
        self.stores.len()
    }

    /// Whether the chain has no stores.
    pub fn is_empty(&self) -> bool {
        self.stores.is_empty()
    }
}

impl Default for ChainSecretStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ISecretStore for ChainSecretStore {
    async fn get(&self, name: &str) -> Result<SecretValue, SecretError> {
        let mut first_error = None;
        for store in &self.stores {
            match store.get(name).await {
                Ok(value) => return Ok(value),
                Err(error) if error.is_not_found() => {
                    first_error.get_or_insert(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(first_error.unwrap_or_else(|| SecretError::NotFound(name.to_string())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ENV: AtomicU64 = AtomicU64::new(0);

    fn unique_env() -> String {
        let sequence = NEXT_ENV.fetch_add(1, Ordering::Relaxed);
        format!("WEBX_TEST_SECRET_{}_{}", std::process::id(), sequence)
    }

    #[tokio::test]
    async fn env_store_reads_a_value_and_reports_missing() {
        let name = unique_env();
        std::env::set_var(&name, "s3cret");
        let store = EnvSecretStore::new();
        assert_eq!(store.get(&name).await.expect("read").expose(), "s3cret");
        std::env::remove_var(&name);
        assert!(matches!(
            store.get(&name).await,
            Err(SecretError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn empty_env_value_is_not_a_secret() {
        let name = unique_env();
        std::env::set_var(&name, "");
        let store = EnvSecretStore::new();
        assert!(matches!(
            store.get(&name).await,
            Err(SecretError::NotFound(_))
        ));
        std::env::remove_var(&name);
    }

    #[test]
    fn secret_value_is_redacted_in_debug() {
        let value = SecretValue::new("do-not-print");
        assert_eq!(format!("{value:?}"), "SecretValue(<redacted>)");
        assert_eq!(value.expose(), "do-not-print");
    }

    #[test]
    fn secret_errors_map_to_sanitized_framework_errors() {
        let not_found: crate::error::Error =
            SecretError::NotFound("OPENAI_API_KEY".to_string()).into();
        assert_eq!(not_found.status_code(), 500);
        assert_eq!(not_found.code(), "internal_error");
        assert_eq!(not_found.public_detail(), None);
        assert!(!not_found.retryable(), "a missing secret is not transient");
        assert!(!not_found.to_string().contains("OPENAI_API_KEY"));

        let unavailable: crate::error::Error = SecretError::Unavailable {
            name: "TOTP_PROTECTION_KEY".to_string(),
            detail: "connection refused by vault at 10.0.0.7".to_string(),
        }
        .into();
        assert_eq!(unavailable.status_code(), 503);
        assert_eq!(unavailable.code(), "service_unavailable");
        assert!(unavailable.retryable(), "503 invites a retry");
        assert_eq!(unavailable.public_detail(), Some(SECRET_UNAVAILABLE_DETAIL));
        for operator_only in ["TOTP_PROTECTION_KEY", "vault", "10.0.0.7"] {
            assert!(!unavailable.to_string().contains(operator_only));
            assert!(!unavailable
                .public_detail()
                .expect("503 keeps a fixed detail")
                .contains(operator_only));
        }
    }

    #[tokio::test]
    async fn file_store_reads_trimmed_values_and_hides_errors() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = FileSecretStore::new(dir.path());
        std::fs::write(dir.path().join("api.key"), "abc123\n").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                dir.path().join("api.key"),
                std::fs::Permissions::from_mode(0o600),
            )
            .expect("chmod");
        }

        assert_eq!(store.get("api.key").await.expect("read").expose(), "abc123");
        assert!(matches!(
            store.get("missing").await,
            Err(SecretError::NotFound(_))
        ));
        assert!(matches!(
            store.get("../api.key").await,
            Err(SecretError::Unavailable { .. })
        ));
        assert!(matches!(
            store.get("a/b").await,
            Err(SecretError::Unavailable { .. })
        ));
    }

    #[tokio::test]
    async fn empty_file_is_not_a_secret() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("empty");
        std::fs::write(&path, "\n").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        }
        let store = FileSecretStore::new(dir.path());
        assert!(matches!(
            store.get("empty").await,
            Err(SecretError::NotFound(_))
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn file_store_rejects_world_readable_secrets() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("loose");
        std::fs::write(&path, "abc").expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let store = FileSecretStore::new(dir.path());
        assert!(matches!(
            store.get("loose").await,
            Err(SecretError::Unavailable { detail, .. }) if detail.contains("0600")
        ));
    }

    #[tokio::test]
    async fn file_store_rejects_symlinks_on_all_platforms() {
        let dir = tempfile::tempdir().expect("temp dir");
        let outside = dir.path().join("outside.txt");
        std::fs::write(&outside, "external-secret").expect("write outside file");

        let link = dir.path().join("linked");
        #[cfg(unix)]
        let created = std::os::unix::fs::symlink(&outside, &link);
        #[cfg(windows)]
        let created = std::os::windows::fs::symlink_file(&outside, &link);
        // Creating symlinks needs extra privileges on some hosts; if the OS
        // refuses, there is no link to test and we skip rather than false-fail.
        if created.is_err() {
            return;
        }

        let store = FileSecretStore::new(dir.path());
        assert!(
            matches!(
                store.get("linked").await,
                Err(SecretError::Unavailable { detail, .. }) if detail.contains("symlink")
            ),
            "a symlink must never be followed, on any platform"
        );
    }

    #[tokio::test]
    async fn chain_falls_through_not_found_and_stops_on_unavailable() {
        struct Fixed(Result<&'static str, SecretError>);
        #[async_trait]
        impl ISecretStore for Fixed {
            async fn get(&self, _name: &str) -> Result<SecretValue, SecretError> {
                match &self.0 {
                    Ok(value) => Ok(SecretValue::new(*value)),
                    Err(SecretError::NotFound(name)) => Err(SecretError::NotFound(name.clone())),
                    Err(SecretError::Unavailable { name, detail }) => {
                        Err(SecretError::Unavailable {
                            name: name.clone(),
                            detail: detail.clone(),
                        })
                    }
                }
            }
        }

        let chain = ChainSecretStore::new()
            .push(Arc::new(Fixed(Err(SecretError::NotFound("x".into())))))
            .push(Arc::new(Fixed(Ok("second"))));
        assert_eq!(
            chain.get("x").await.expect("second store").expose(),
            "second"
        );
        assert_eq!(chain.len(), 2);

        let chain = ChainSecretStore::new()
            .push(Arc::new(Fixed(Err(SecretError::Unavailable {
                name: "x".into(),
                detail: "store down".into(),
            }))))
            .push(Arc::new(Fixed(Ok("second"))));
        assert!(matches!(
            chain.get("x").await,
            Err(SecretError::Unavailable { .. })
        ));
    }
}
