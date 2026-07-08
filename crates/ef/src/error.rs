//! Map rust-ef errors to rust-webx HTTP-oriented errors.

use rust_ef::error::EFError;
use rust_webx_core::Error;

/// Convert an `EFError` into a framework `Error` with appropriate HTTP semantics.
pub fn map_ef_error(err: EFError) -> Error {
    match err {
        EFError::NotFound(msg, _) => Error::NotFound(msg),
        EFError::ConcurrencyConflict(msg, _) => Error::Conflict(msg),
        EFError::ModelValidation(msg, _) => Error::Validation(msg),
        EFError::Query(msg, _) => Error::Http(msg),
        other => Error::Internal(other.to_string()),
    }
}

/// Extension for `Result<T, EFError>`.
pub trait EfResultExt<T> {
    fn map_ef(self) -> Result<T, Error>;
}

impl<T> EfResultExt<T> for Result<T, EFError> {
    fn map_ef(self) -> Result<T, Error> {
        self.map_err(map_ef_error)
    }
}
