//! DbContext helpers — persistence error mapping, save, and reload macros.
//!
//! `rust-ef` is Web-agnostic (2.6 removed its historical `webx` feature), so
//! translating `EFError` into `webx::Error` is an application-boundary
//! decision. This module owns that decision once; handler modules only call
//! `.map_ef()?`. The contract is documented in
//! `docs/design/compatibility.md` §4.

use rust_ef::db_context::DbContext;
use rust_ef::error::EFError;
use webx::{Error, Result};

/// Client-visible detail for a `400` caused by database-derived input problems.
///
/// The original `Query` / `TypeConversion` message may embed SQL and column
/// names, so it is not echoed.
const DETAIL_INVALID_REQUEST: &str = "the submitted data is invalid";

/// Client-visible detail for a unique-constraint race (`409`).
///
/// Deliberately generic: `UniqueViolation` carries `constraint`/`table`, which
/// are schema details that must stay server-side.
const DETAIL_UNIQUE_VIOLATION: &str = "a unique constraint was violated by a concurrent request";

/// Client-visible detail for a transient database outage (`503`).
const DETAIL_UNAVAILABLE: &str = "the database is temporarily unavailable; retry later";

/// Map a rust-ef failure to the framework error contract.
pub fn map_ef_error(error: EFError) -> Error {
    match error {
        // Business conclusions authored by the caller keep their own message.
        EFError::NotFound(message, _) => Error::NotFound(message),
        EFError::ModelValidation(message, _) => Error::Validation(message),
        EFError::ConcurrencyConflict(message, _) => Error::Conflict(message),

        // Database-derived payloads that must not be echoed.
        EFError::Query(..) | EFError::TypeConversion(..) => {
            Error::Validation(DETAIL_INVALID_REQUEST.to_string())
        }
        EFError::UniqueViolation { .. } => Error::Conflict(DETAIL_UNIQUE_VIOLATION.to_string()),

        // Transient infrastructure failures: retryable 503 with fixed text.
        EFError::Connection(..) | EFError::Transaction(..) => {
            Error::Status(503, DETAIL_UNAVAILABLE.to_string())
        }

        // `Database`/`Migration`/`Configuration`/`Provider`/`ChangeTracking`/
        // `Other`: sanitized 500 (`public_detail()` is `None`); the original
        // message and its source chain only reach the operator log.
        other => Error::Internal(operator_detail(&other)),
    }
}

/// Operator-facing detail for a `500`: the EF message plus its source chain.
fn operator_detail(error: &EFError) -> String {
    let mut detail = error.to_string();
    let mut cause = std::error::Error::source(error);
    while let Some(error) = cause {
        detail.push_str(": ");
        detail.push_str(&error.to_string());
        cause = error.source();
    }
    detail
}

/// Extension for `Result<T, EFError>` at the application boundary.
pub trait EfResultExt<T> {
    /// Convert a persistence failure into the framework error.
    fn map_ef(self) -> Result<T>;
}

impl<T> EfResultExt<T> for std::result::Result<T, EFError> {
    fn map_ef(self) -> Result<T> {
        self.map_err(map_ef_error)
    }
}

/// Persist pending changes, mapping ORM errors to framework errors.
pub async fn save_changes(ctx: &mut DbContext) -> Result<()> {
    ctx.save_changes().await.map_ef()?;
    Ok(())
}

/// Find one row by String primary key.
#[macro_export]
macro_rules! ef_find_by_id {
    ($ctx:expr, $entity:ty, $id:expr $(; $($tail:tt)+)?) => {{
        use $crate::db::EfResultExt;
        let q = ($id).to_string();
        linq!(
            $ctx.set::<$entity>(),
            |row: $entity| row.id == q
            $(; $($tail)+)?
        )
        .first_or_default()
        .await
        .map_ef()
    }};
}

/// Find one row by id or return the given error.
#[macro_export]
macro_rules! ef_require_by_id {
    ($ctx:expr, $entity:ty, $id:expr, $err:expr $(; $($tail:tt)+)?) => {{
        $crate::ef_find_by_id!($ctx, $entity, $id $(; $($tail)+)?)?
            .ok_or_else(|| $err)?
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_ef::error::DatabaseErrorInfo;

    #[test]
    fn business_errors_keep_their_message() {
        assert_eq!(
            map_ef_error(EFError::not_found("missing")).status_code(),
            404
        );
        assert_eq!(
            map_ef_error(EFError::model_validation("bad input")).public_detail(),
            Some("bad input")
        );
        assert_eq!(
            map_ef_error(EFError::concurrency_conflict("busy")).status_code(),
            409
        );
    }

    #[test]
    fn unique_violation_is_a_sanitized_409() {
        let error = map_ef_error(EFError::unique_violation(
            Some("ux_users_email".into()),
            Some("users".into()),
        ));
        assert_eq!(error.status_code(), 409);
        let detail = error.public_detail().expect("4xx keeps a client detail");
        assert_eq!(detail, DETAIL_UNIQUE_VIOLATION);
        assert!(!detail.contains("ux_users_email"));
        assert!(!detail.contains("users"));
    }

    #[test]
    fn query_errors_are_a_sanitized_400() {
        let error = map_ef_error(EFError::query("SELECT * FROM secret_table"));
        assert_eq!(error.status_code(), 400);
        assert_eq!(error.public_detail(), Some(DETAIL_INVALID_REQUEST));
    }

    #[test]
    fn connection_errors_are_a_retryable_503() {
        let error = map_ef_error(EFError::connection("postgres://user:pass@db/app"));
        assert_eq!(error.status_code(), 503);
        assert!(error.retryable());
        assert_eq!(error.public_detail(), Some(DETAIL_UNAVAILABLE));
    }

    #[test]
    fn server_errors_are_hidden_500() {
        let error = map_ef_error(EFError::database(
            DatabaseErrorInfo::new("relation \"secret_table\" does not exist")
                .with_sqlstate("42P01")
                .with_sql("SELECT * FROM secret_table"),
        ));
        assert_eq!(error.status_code(), 500);
        assert_eq!(error.public_detail(), None);
        // The message is kept for the operator log, never for the client.
        assert!(error.to_string().contains("secret_table"));
    }
}
