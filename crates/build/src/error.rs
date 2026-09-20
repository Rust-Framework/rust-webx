//! Failures a build script can hit while embedding static files.
//!
//! Errors are values here for the same reason they are at runtime: a missing
//! asset directory is a predictable condition, not a bug, so it is reported as
//! a typed error that `build.rs` can propagate with `?` instead of a panic.

use std::fmt::{self, Display, Formatter};
use std::path::{Component, Path, PathBuf};

/// What went wrong while compiling static files into the executable.
pub enum Error {
    /// The configured directory does not exist, or is a file rather than a
    /// directory. Almost always a wrong path or assets that were never built.
    NotADirectory {
        declared: PathBuf,
        resolved: PathBuf,
    },

    /// A filesystem operation on an asset or the generated table failed.
    Io {
        /// Verb for the message, e.g. `read` or `write`.
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },

    /// Called outside a build script, where cargo has not provided `OUT_DIR`.
    MissingOutDir,

    /// [`crate::Builder::build`] was called without [`crate::Builder::web_root`].
    MissingWebRoot,
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotADirectory { declared, resolved } => write!(
                f,
                "static asset directory `{}` does not exist (resolved to `{}`); \
                 create it, or pass the right path to builder().web_root(...)",
                collapsed(declared).display(),
                collapsed(resolved).display(),
            ),
            Error::Io {
                operation,
                path,
                source,
            } => write!(f, "cannot {operation} `{}`: {source}", path.display()),
            Error::MissingOutDir => write!(
                f,
                "webx::builder() must be called from a build script: OUT_DIR is not set"
            ),
            Error::MissingWebRoot => write!(
                f,
                "webx::builder().build() needs .web_root(\"...\") before .build()"
            ),
        }
    }
}

/// Drop `.` and `..` segments so a message shows the directory meant rather than
/// the way it was written (`wwwroot/host/../assets` reads as `wwwroot/assets`).
///
/// Collapsing only ever happens for absolute paths: there, dropping a `..` pair
/// cannot name a different location, and the result is still absolute. A
/// relative path is shown exactly as written, because resolving it correctly
/// needs the working directory.
fn collapsed(path: &Path) -> PathBuf {
    if !path.is_absolute() {
        return path.to_path_buf();
    }
    let mut parts: Vec<Component<'_>> = Vec::with_capacity(path.components().count());
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Never step above the prefix or root, so `C:\..` stays `C:\`.
                if matches!(parts.last(), Some(Component::Normal(_))) {
                    parts.pop();
                }
            }
            other => parts.push(other),
        }
    }
    parts.into_iter().collect()
}

/// Cargo reports a failed build script through `Debug`, so delegate to
/// [`Display`]: the operator sees the message above instead of a struct dump.
impl fmt::Debug for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            Error::NotADirectory { .. } | Error::MissingOutDir | Error::MissingWebRoot => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{collapsed, Error};
    use std::path::{Path, PathBuf};

    /// An absolute base, built from the working directory so the test does not
    /// assume the platform's root form.
    fn base() -> PathBuf {
        std::env::current_dir().unwrap().join("repo")
    }

    #[test]
    fn a_message_shows_the_directory_meant_not_the_way_it_was_written() {
        let written = base().join("host").join("..").join("assets");
        assert_eq!(collapsed(&written), base().join("assets"));

        let with_curdir = base().join(".").join("assets");
        assert_eq!(collapsed(&with_curdir), base().join("assets"));
    }

    #[test]
    fn a_relative_path_is_left_exactly_as_written() {
        for written in ["../wwwroot", "wwwroot", "./assets/../app.css"] {
            let path = Path::new(written);
            assert_eq!(collapsed(path), path, "{written} must not be rewritten");
        }
    }

    #[test]
    #[cfg(unix)]
    fn collapsing_never_steps_above_the_root() {
        assert_eq!(collapsed(Path::new("/..")), Path::new("/"));
        assert_eq!(collapsed(Path::new("/../../etc")), Path::new("/etc"));
    }

    #[test]
    #[cfg(windows)]
    fn collapsing_never_steps_above_the_prefix_or_root() {
        assert_eq!(collapsed(Path::new(r"C:\..")), Path::new(r"C:\"));
        assert_eq!(collapsed(Path::new(r"C:\../../x")), Path::new(r"C:\x"));
    }

    #[test]
    fn the_message_names_the_paths_and_how_to_fix_them() {
        let resolved = base().join("host").join("..").join("wwwroot");
        let error = Error::NotADirectory {
            declared: PathBuf::from("../wwwroot"),
            resolved: resolved.clone(),
        };
        let message = error.to_string();

        assert!(message.contains("`../wwwroot`"), "got {message}");
        assert!(message.contains("web_root"), "got {message}");

        // The resolved path is shown collapsed, not as it was assembled.
        let raw = resolved.display().to_string();
        let shown = collapsed(&resolved).display().to_string();
        assert_ne!(raw, shown, "this test needs a path worth collapsing");
        assert!(message.contains(&shown), "got {message}");
        assert!(!message.contains(&raw), "got {message}");
    }

    /// Cargo prints a failed build script through `Debug`, so both views are the
    /// same operator-facing sentence.
    #[test]
    fn debug_matches_display() {
        let error = Error::MissingOutDir;
        assert_eq!(format!("{error}"), format!("{error:?}"));
    }
}
