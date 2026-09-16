//! Upload spool directory management and client filename sanitisation.

use std::path::PathBuf;
use std::sync::RwLock;

/// Prefix given to every temporary file the framework writes for an upload.
pub const SPOOL_FILE_PREFIX: &str = "webx-upload-";

/// Longest filename [`sanitize_file_name`] will produce, in bytes.
const MAX_FILE_NAME_LEN: usize = 200;

static SPOOL_ROOT: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Point spooling at `path`.
///
/// The host calls this from `Host::build()` with `Form:TempDir`. When unset,
/// spooled uploads go to `webx-uploads` inside the system temporary directory.
pub fn set_spool_root(path: impl Into<PathBuf>) {
    let mut guard = SPOOL_ROOT
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = Some(path.into());
}

/// The directory used for spooled uploads.
pub fn spool_root() -> PathBuf {
    let guard = SPOOL_ROOT
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(root) = guard.as_ref() {
        return root.clone();
    }
    std::env::temp_dir().join("webx-uploads")
}

/// Reduce a client-supplied filename to a safe basename.
///
/// The result never contains a path separator, never contains control
/// characters, and is never `.`/`..`, so it can be joined onto a directory
/// without escaping it. Windows-reserved characters are replaced with `_`.
///
/// Returns `"file"` when nothing usable is left.
pub fn sanitize_file_name(raw: &str) -> String {
    // Keep only the last path component, whichever separator was used.
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");

    let mut cleaned = String::with_capacity(base.len());
    for ch in base.chars() {
        if ch.is_control() {
            continue;
        }
        match ch {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => cleaned.push('_'),
            _ => cleaned.push(ch),
        }
    }

    // Windows rejects trailing dots and spaces; "." and ".." are traversal.
    let trimmed = cleaned.trim().trim_end_matches(['.', ' ']);
    let candidate = if trimmed.is_empty() { "file" } else { trimmed };

    truncate_preserving_extension(candidate, MAX_FILE_NAME_LEN)
}

fn truncate_preserving_extension(name: &str, max: usize) -> String {
    if name.len() <= max {
        return name.to_string();
    }
    // Preserve a short extension so `SaveAs` still picks a sensible type.
    match name.rfind('.') {
        Some(idx) if idx > 0 && name.len() - idx <= 16 => {
            let ext = &name[idx..];
            let stem = truncate_chars(&name[..idx], max.saturating_sub(ext.len()));
            format!("{stem}{ext}")
        }
        _ => truncate_chars(name, max),
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}
