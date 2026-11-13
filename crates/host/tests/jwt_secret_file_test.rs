//! XR-2: `Jwt.SecretFile` resolution — priority, fail-fast validation and the
//! Unix permission check.
//!
//! The first half calls [`apply_jwt_secret_file`] directly (unit-level: priority,
//! trimming, error messages); the second half builds a real [`Host`] and checks
//! the same resolution through `HostBuilder`, i.e. the path an application
//! actually takes at start-up.

use webx_core::config::AppOptions;
use webx_core::error::Error;
use webx_core::mode::AppMode;
use webx_host::auth_jwt::{apply_jwt_secret_file, MIN_JWT_SECRET_LEN};
use webx_host::server::Host;

const GOOD_SECRET: &str = "0123456789abcdef0123456789abcdef"; // exactly 32 bytes

fn options_with_file(path: &std::path::Path) -> AppOptions {
    let mut options = AppOptions::default();
    options.jwt.secret_file = Some(path.to_string_lossy().into_owned());
    options
}

fn temporary_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("a temporary directory is available")
}

fn write_file(dir: &tempfile::TempDir, name: &str, content: &[u8]) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, content).expect("the secret file is writable");
    path
}

/// Secret files must stay private; tests that create one on Unix set 0600
/// explicitly so they exercise the read path rather than the permission guard.
#[cfg(unix)]
fn set_mode(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .expect("the secret file mode is settable");
}

#[cfg(unix)]
fn write_private_file(dir: &tempfile::TempDir, name: &str, content: &[u8]) -> std::path::PathBuf {
    let path = write_file(dir, name, content);
    set_mode(&path, 0o600);
    path
}

#[cfg(not(unix))]
fn write_private_file(dir: &tempfile::TempDir, name: &str, content: &[u8]) -> std::path::PathBuf {
    // No POSIX modes: the ACL is trusted (documented limitation).
    write_file(dir, name, content)
}

#[test]
fn minimum_length_matches_the_documented_32_bytes() {
    assert_eq!(MIN_JWT_SECRET_LEN, 32);
}

#[test]
fn no_secret_and_no_file_is_a_noop() {
    let mut options = AppOptions::default();
    apply_jwt_secret_file(&mut options).expect("nothing configured is not an error");
    assert!(options.jwt.secret.is_empty());
}

#[test]
fn blank_secret_file_is_a_noop() {
    // An appsettings placeholder such as `"SecretFile": ""` must not trip
    // start-up: only a real path is opened.
    for blank in ["", "   ", "\t\n"] {
        let mut options = AppOptions::default();
        options.jwt.secret_file = Some(blank.to_string());
        apply_jwt_secret_file(&mut options).expect("a blank path is not configured");
        assert!(options.jwt.secret.is_empty());
    }
}

#[test]
fn secret_file_contents_are_used_when_secret_is_empty() {
    let dir = temporary_dir();
    let path = write_private_file(&dir, "jwt.key", GOOD_SECRET.as_bytes());

    let mut options = options_with_file(&path);
    apply_jwt_secret_file(&mut options).expect("a valid secret file is accepted");

    assert_eq!(options.jwt.secret, GOOD_SECRET);
    assert_eq!(
        options.jwt.secret_file.as_deref(),
        Some(path.to_string_lossy().as_ref()),
        "the resolved value lives in `Secret`; `SecretFile` stays as configured"
    );
}

#[test]
fn surrounding_whitespace_is_trimmed_from_the_file() {
    // `echo secret > jwt.key` leaves a newline; it must not become part of the key.
    let dir = temporary_dir();
    let content = format!("  {GOOD_SECRET}  \n");
    let path = write_private_file(&dir, "jwt.key", content.as_bytes());

    let mut options = options_with_file(&path);
    apply_jwt_secret_file(&mut options).expect("a padded secret file is accepted");
    assert_eq!(options.jwt.secret, GOOD_SECRET);
}

#[test]
fn explicit_secret_wins_and_the_file_is_never_read() {
    let dir = temporary_dir();
    // The path does not exist: resolving would fail, which proves the file is
    // not consulted when `Jwt.Secret` is already set.
    let missing = dir.path().join("does-not-exist.key");

    let mut options = options_with_file(&missing);
    options.jwt.secret = "explicit-secret-from-appsettings".to_string();

    apply_jwt_secret_file(&mut options).expect("an explicit secret wins");
    assert_eq!(options.jwt.secret, "explicit-secret-from-appsettings");
}

#[test]
fn missing_file_fails_fast() {
    let dir = temporary_dir();
    let missing = dir.path().join("missing.key");

    let mut options = options_with_file(&missing);
    let error = apply_jwt_secret_file(&mut options).expect_err("a missing secret file is fatal");

    assert!(matches!(error, Error::Di(_)), "got {error:?}");
    let message = error.to_string();
    assert!(
        message.contains("missing.key"),
        "the message names the path: {message}"
    );
    assert!(
        message.contains("Jwt.Secret") || message.contains("provision"),
        "the message says what to do: {message}"
    );
    assert!(
        options.jwt.secret.is_empty(),
        "a failed resolution must not partially configure a key"
    );
}

#[test]
fn directory_instead_of_file_fails_fast() {
    let dir = temporary_dir();
    let mut options = options_with_file(dir.path());
    let error = apply_jwt_secret_file(&mut options).expect_err("a directory is not a secret file");
    assert!(matches!(error, Error::Di(_)), "got {error:?}");
}

#[test]
fn empty_file_fails_fast() {
    let dir = temporary_dir();
    let path = write_private_file(&dir, "jwt.key", b"");

    let mut options = options_with_file(&path);
    let error = apply_jwt_secret_file(&mut options).expect_err("an empty secret file is fatal");
    assert!(matches!(error, Error::Di(_)), "got {error:?}");
    assert!(
        error.to_string().contains("empty"),
        "the message explains the failure: {error}"
    );
}

#[test]
fn whitespace_only_file_fails_fast() {
    let dir = temporary_dir();
    let path = write_private_file(&dir, "jwt.key", b"  \n\t \n");

    let mut options = options_with_file(&path);
    let error = apply_jwt_secret_file(&mut options).expect_err("blank content is not a secret");
    assert!(matches!(error, Error::Di(_)), "got {error:?}");
}

#[test]
fn too_short_file_fails_fast() {
    let dir = temporary_dir();
    let short = "0".repeat(MIN_JWT_SECRET_LEN - 1);
    let path = write_private_file(&dir, "jwt.key", short.as_bytes());

    let mut options = options_with_file(&path);
    let error = apply_jwt_secret_file(&mut options).expect_err("a weak key is fatal");
    assert!(matches!(error, Error::Di(_)), "got {error:?}");
    assert!(
        error.to_string().contains("only 31 bytes"),
        "the message reports the observed length: {error}"
    );
}

#[test]
fn exactly_minimum_length_is_accepted() {
    let dir = temporary_dir();
    let path = write_private_file(&dir, "jwt.key", GOOD_SECRET.as_bytes());

    let mut options = options_with_file(&path);
    apply_jwt_secret_file(&mut options).expect("32 bytes is the documented minimum");
    assert_eq!(options.jwt.secret.len(), MIN_JWT_SECRET_LEN);
}

#[cfg(unix)]
#[test]
fn group_or_other_accessible_file_is_rejected() {
    let dir = temporary_dir();
    let path = write_file(&dir, "jwt.key", GOOD_SECRET.as_bytes());

    // 0644 is the classic mistake: the key is world-readable.
    for mode in [0o644, 0o640, 0o604, 0o660, 0o606, 0o777] {
        set_mode(&path, mode);
        let mut options = options_with_file(&path);
        let error = apply_jwt_secret_file(&mut options).expect_err("group/other access is refused");
        assert!(
            matches!(error, Error::Di(_)),
            "mode {mode:04o}: got {error:?}"
        );
        assert!(
            error.to_string().contains("chmod 600"),
            "mode {mode:04o}: the message names the fix: {error}"
        );
        assert!(options.jwt.secret.is_empty());
    }
}

#[cfg(unix)]
#[test]
fn owner_only_modes_are_accepted() {
    let dir = temporary_dir();
    let path = write_file(&dir, "jwt.key", GOOD_SECRET.as_bytes());

    for mode in [0o600, 0o400] {
        set_mode(&path, mode);
        let mut options = options_with_file(&path);
        apply_jwt_secret_file(&mut options)
            .unwrap_or_else(|error| panic!("mode {mode:04o} must be accepted: {error}"));
        assert_eq!(options.jwt.secret, GOOD_SECRET);
    }
}

// ---------------------------------------------------------------------------
// Host-level: the resolution is wired into `HostBuilder::prepare()`
// ---------------------------------------------------------------------------

/// Configure a host exactly like an application using the file source would:
/// `Jwt.SecretFile` set, `Jwt.Secret` explicitly cleared so a development
/// `appsettings.json`/`JWT_SECRET` in the workspace cannot mask the file.
fn configure_secret_file(
    path: &std::path::Path,
) -> impl FnOnce(&mut webx_host::server::HostAppBuilder) + Send + 'static {
    let secret_file = path.to_string_lossy().into_owned();
    move |app| {
        app.useOptions(move |options| {
            options.jwt.secret.clear();
            options.jwt.secret_file = Some(secret_file);
        });
    }
}

#[test]
fn host_build_resolves_the_secret_from_the_file() {
    let dir = temporary_dir();
    let path = write_private_file(&dir, "jwt.key", GOOD_SECRET.as_bytes());

    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(configure_secret_file(&path))
        .add_authentication()
        .try_build()
        .expect("a valid secret file must let the host build");

    // Every later consumer — issuer, verifier, `jwt_secret()`, the Production
    // guard — reads `options.jwt.secret`, so this is the value they see.
    assert_eq!(host.options().jwt.secret, GOOD_SECRET);
}

#[test]
fn host_build_fails_fast_when_the_secret_file_is_missing() {
    let dir = temporary_dir();
    let missing = dir.path().join("absent.key");

    let error = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(configure_secret_file(&missing))
        .add_authentication()
        .try_build()
        .err()
        .expect("a missing secret file must abort start-up");

    assert!(matches!(error, Error::Di(_)), "got {error:?}");
    assert!(
        error.to_string().contains("could not be read"),
        "the operator gets an actionable message: {error}"
    );
}

#[test]
fn host_build_fails_fast_when_the_secret_file_is_too_short() {
    let dir = temporary_dir();
    let path = write_private_file(&dir, "jwt.key", b"too-short-secret");

    let error = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(configure_secret_file(&path))
        .add_authentication()
        .try_build()
        .err()
        .expect("a weak secret file must abort start-up");

    assert!(matches!(error, Error::Di(_)), "got {error:?}");
    assert!(
        error.to_string().contains("at least 32 bytes"),
        "the message states the requirement: {error}"
    );
}

#[test]
fn code_level_secret_override_wins_over_the_file() {
    let dir = temporary_dir();
    // The path does not exist: if the file were consulted the build would fail.
    let missing = dir.path().join("absent.key");

    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .configure(move |app| {
            let secret_file = missing.to_string_lossy().into_owned();
            app.useOptions(move |options| {
                options.jwt.secret = "explicit-secret-from-useoptions".into();
                options.jwt.secret_file = Some(secret_file);
            });
        })
        .add_authentication()
        .try_build()
        .expect("an explicit secret must skip the file");

    assert_eq!(host.options().jwt.secret, "explicit-secret-from-useoptions");
}

#[test]
fn production_guard_accepts_a_file_backed_secret() {
    let dir = temporary_dir();
    let path = write_private_file(&dir, "jwt.key", GOOD_SECRET.as_bytes());

    let host = Host::builder()
        .mode(AppMode::Production)
        .no_spa()
        .configure(move |app| {
            let secret_file = path.to_string_lossy().into_owned();
            app.useOptions(move |options| {
                options.jwt.secret.clear();
                options.jwt.secret_file = Some(secret_file);
                options.cors.origins = vec!["https://example.com".into()];
            });
        })
        .add_authentication()
        .try_build()
        .expect("the resolved file secret must satisfy the Production weak-secret guard");

    assert_eq!(host.options().jwt.secret, GOOD_SECRET);
}
