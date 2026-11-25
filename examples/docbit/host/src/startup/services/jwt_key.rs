//! First-boot provisioning of the JWT signing key named by `Jwt.SecretFile`.
//!
//! The framework resolves `Jwt.SecretFile` while building the host (priority:
//! `APP__Jwt__Secret`/`JWT_SECRET` → `Jwt.Secret` → `Jwt.SecretFile`) and fails
//! fast when the configured file is missing, too short or world-readable. The
//! **application** only provisions the file; it never reads the key material —
//! the framework hands the resolved secret to the issuer and verifier.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use webx::*;

/// Random bytes in a generated key, hex-encoded (64 characters).
const SECRET_BYTES: usize = 32;

/// Create the configured `Jwt.SecretFile` when it does not exist yet.
///
/// Returns the path that was created (or already existed), or `None` when no
/// `Jwt.SecretFile` is configured — an env/`Jwt.Secret` deployment needs nothing.
/// An existing key is never overwritten: rotating it is an operator decision.
pub fn ensure_jwt_secret_file(mode: AppMode) -> Result<Option<PathBuf>> {
    let Some(path) = configured_secret_file(mode)? else {
        return Ok(None);
    };

    if path.exists() {
        return Ok(Some(path));
    }

    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::di(format!("Cannot create '{}': {}", parent.display(), e)))?;
    }

    write_secret_file(&path)?;
    tracing::warn!(
        "[docbit] Generated a new JWT signing key at '{}'. Back it up and keep it out of \
         source control; deleting it invalidates every issued token.",
        path.display()
    );
    Ok(Some(path))
}

/// Read `Jwt.SecretFile` from the merged configuration (file + env overrides).
fn configured_secret_file(mode: AppMode) -> Result<Option<PathBuf>> {
    let (appsettings, _sources) = webx::try_load_appsettings(mode)
        .map_err(|e| Error::di(format!("Invalid configuration: {}", e)))?;

    Ok(appsettings
        .as_ref()
        .and_then(|value| value.get("Jwt"))
        .and_then(|jwt| jwt.get("SecretFile"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from))
}

/// Write a fresh random secret and restrict the file to the service account.
fn write_secret_file(path: &Path) -> Result<()> {
    // Two v4 UUIDs: 244 random bits, hex-encoded.
    let secret = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    debug_assert_eq!(secret.len(), SECRET_BYTES * 2);

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| Error::di(format!("Cannot create '{}': {}", path.display(), e)))?;
    file.write_all(secret.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|e| Error::di(format!("Cannot write '{}': {}", path.display(), e)))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| Error::di(format!("Cannot chmod 600 '{}': {}", path.display(), e)))?;
    }

    Ok(())
}
