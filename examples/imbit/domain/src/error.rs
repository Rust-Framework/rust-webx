//! Typed domain errors and their single bridge to the framework contract.

use std::fmt;

use webx_core::Error;

/// A domain or repository failure, classified by what the caller must do about
/// it.
///
/// Aggregates and the in-process repositories return this instead of a `String`
/// so a handler never inspects message text to pick a status code: the [`From`]
/// bridge below is the only place that decides `400` / `403` / `404` / `409`,
/// and handlers reach it with a plain `?`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The referenced aggregate does not exist (`404`).
    NotFound(String),
    /// The caller may not act on the aggregate (`403`).
    Forbidden(String),
    /// The request breaks a domain rule (`400`).
    Validation(String),
    /// The current aggregate state forbids the operation (`409`).
    Conflict(String),
}

impl StoreError {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::Forbidden(message.into())
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation(message.into())
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    /// The client-visible text. Messages are written by the domain and never
    /// carry internals (no storage keys, no constraint names, no stack detail).
    pub fn message(&self) -> &str {
        match self {
            Self::NotFound(message)
            | Self::Forbidden(message)
            | Self::Validation(message)
            | Self::Conflict(message) => message,
        }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for StoreError {}

/// The one place that turns a domain classification into the framework error
/// contract.
///
/// `webx::Error` **is** `webx_core::Error` (the umbrella re-exports it), so once
/// this conversion exists a handler writes `domain_result?` and the framework
/// renders the matching problem+json / error frame.
impl From<StoreError> for Error {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::NotFound(message) => Error::not_found(message),
            StoreError::Forbidden(message) => Error::forbidden(message),
            StoreError::Validation(message) => Error::validation(message),
            StoreError::Conflict(message) => Error::conflict(message),
        }
    }
}
