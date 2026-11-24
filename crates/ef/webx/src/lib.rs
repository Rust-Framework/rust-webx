//! The `rust-webx` boundary for `rust-ef`.
//!
//! `rust-ef` is Web-agnostic (2.6 removed its historical `webx` feature), so
//! translating [`EFError`] into [`webx_core::error::Error`] is an
//! application-boundary decision. This crate owns that decision once:
//! handler/list code calls `.map_ef()?` and never matches `EFError` itself.
//!
//! | [`EFError`] | HTTP | Client-visible detail |
//! |---|---|---|
//! | `NotFound` | 404 | the caller-authored message |
//! | `ModelValidation` | 400 | the caller-authored message |
//! | `ConcurrencyConflict` | 409 | the caller-authored message |
//! | `Query` / `TypeConversion` | 400 | fixed text (SQL/column names never echoed) |
//! | `UniqueViolation` | 409 | fixed text (constraint/table never echoed) |
//! | `Connection` / `Transaction` | 503 | fixed text, retryable |
//! | `Database` with a transient driver condition¹ | 503 | fixed text, retryable |
//! | anything else | 500 | none (`public_detail() = None`), operator detail in the log |
//!
//! ¹ `SQLSTATE` class `08` (connection exception) / `40` (serialization failure,
//! deadlock) / `57P0*` (shutdown, crash, cannot connect now), SQLite
//! `busy`/`locked` (extended result codes `5`/`6` in the low byte) and MySQL
//! `1205`/`1213`. Providers report these as [`EFError::Database`] rather than
//! [`EFError::Connection`], so without the classification a transient outage
//! would reach the client as a non-retryable 500.
//!
//! ```ignore
//! use webx_ef::EfResultExt;
//!
//! ctx.save_changes().await.map_ef()?;
//! ```
//!
//! `rust-webx-core` and the `rust-webx` umbrella never link this crate: it is an
//! application dependency, so the ORM stays out of the framework's dependency
//! graph.
//!
//! The mapper is stateless and allocation-free on the success path; the
//! sanitized texts are constants and the operator detail (EF message + source
//! chain) is only built for the 500 branch.
//!
//! # Durable idempotency store
//!
//! [`idempotency::EfIdempotencyStore`] is the rust-ef implementation of
//! [`webx_core::idempotency::IIdempotencyStore`]. A single process can keep the
//! `Idempotency-Key` reservations in memory; **multiple instances** must share
//! them, so register the table entity once and hand the application's
//! `Arc<DbContextOptions>` to the store:
//!
//! ```ignore
//! use std::sync::Arc;
//! use webx_ef::idempotency::{register_idempotency_model, EfIdempotencyStore};
//!
//! register_idempotency_model(&mut ctx); // before bootstrap/migrate
//! let store = Arc::new(EfIdempotencyStore::new(Arc::clone(&db_options)));
//! Host::builder().use_idempotency(store, ttl).build();
//! ```
//!
//! `use_idempotency` is optional: register the store as a DI instance and set
//! `Idempotency.Enabled = true` to have the host pick it up automatically. Every
//! reservation carries a fencing token (`attempt` column) so a reclaimed lease's
//! late completion/release cannot overwrite the new attempt; completed responses
//! also persist the replayable `Location`/`ETag`/`Last-Modified`/`Cache-Control`
//! headers (`headers` column). Both columns are nullable — see [`idempotency`]
//! for the 2.8 schema upgrade note.
//!
//! See [`idempotency`] for the cleanup worker (`IdempotencyHousekeeping`), the
//! TTL/lease semantics and the race handling.

pub mod idempotency;

pub use idempotency::{
    register_idempotency_model, EfIdempotencyStore, IdempotencyHousekeeping, IdempotencyRecord,
};

use rust_ef::error::{DatabaseErrorInfo, EFError};
use webx_core::error::Error;

/// Client-visible detail for a `400` caused by database-derived input problems.
///
/// The original `Query` / `TypeConversion` message may embed SQL and column
/// names, so it is not echoed.
pub const DETAIL_INVALID_REQUEST: &str = "the submitted data is invalid";

/// Client-visible detail for a unique-constraint race (`409`).
///
/// Deliberately generic: `UniqueViolation` carries `constraint`/`table`, which
/// are schema details that must stay server-side.
pub const DETAIL_UNIQUE_VIOLATION: &str =
    "a unique constraint was violated by a concurrent request";

/// Client-visible detail for a transient database outage (`503`).
pub const DETAIL_UNAVAILABLE: &str = "the database is temporarily unavailable; retry later";

/// Whether the provider reported a condition a caller can usefully retry.
///
/// Providers keep driver diagnostics in [`EFError::Database`] instead of
/// normalizing them into [`EFError::Connection`]/[`EFError::Transaction`], so
/// the retryability conclusion has to be read from the diagnostics:
///
/// * `SQLSTATE` class `08` — connection exception.
/// * `SQLSTATE` class `40` — transaction rollback (serialization failure,
///   deadlock).
/// * `SQLSTATE` `57P01`/`57P02`/`57P03` — shutdown, crash, cannot connect now.
/// * SQLite has no `SQLSTATE`; it reports an extended result code whose primary
///   code is the low byte (`SQLITE_BUSY = 5`, `SQLITE_LOCKED = 6`), so
///   `5`/`6`, `261`/`517` (busy recovery/snapshot) and `262` (locked
///   shared-cache) all qualify.
/// * MySQL vendor codes `1205` (lock wait timeout) and `1213` (deadlock);
///   `1213` also carries `SQLSTATE` `40001`, but `1205` does not.
///
/// Everything else stays a hidden 500: an unparseable statement, a missing
/// table or a constraint violation is a defect, not a retryable outage.
fn is_transient(info: &DatabaseErrorInfo) -> bool {
    if let Some(sqlstate) = info.sqlstate.as_deref() {
        if sqlstate.starts_with("08")
            || sqlstate.starts_with("40")
            || matches!(sqlstate, "57P01" | "57P02" | "57P03")
        {
            return true;
        }
    }
    match info.code.as_deref() {
        Some("1205" | "1213") => true,
        // SQLite is the only supported provider that reports no SQLSTATE; its
        // extended result code embeds the primary code in the low eight bits.
        Some(code) if info.sqlstate.is_none() => code
            .parse::<i32>()
            .map(|code| matches!(code & 0xFF, 5 | 6))
            .unwrap_or(false),
        _ => false,
    }
}

/// Map a rust-ef failure to the framework error contract.
///
/// Business conclusions authored by the caller (`NotFound`, `ModelValidation`,
/// `ConcurrencyConflict`) keep their message; infrastructure-derived payloads
/// are replaced by fixed text so SQL, column, constraint and connection details
/// never reach the client. A driver-reported transient condition
/// ([`EFError::Database`] with a retryable `SQLSTATE`/vendor code) becomes the
/// same retryable 503 as [`EFError::Connection`], so an application never has to
/// re-classify driver errors itself.
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

        // Driver-reported transient conditions (connection loss, deadlock,
        // SQLite busy/locked) are the same retryable conclusion, just wrapped
        // in structured diagnostics by the provider.
        EFError::Database(info, _) if is_transient(&info) => {
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

/// Extension for `Result<T, EFError>` at the persistence boundary.
///
/// Implemented for `rust_ef::error::Result<T>` so call sites read
/// `.map_ef()?`; the success path is unchanged and adds no allocation.
pub trait EfResultExt<T> {
    /// Convert a persistence failure into the framework error.
    fn map_ef(self) -> webx_core::error::Result<T>;
}

impl<T> EfResultExt<T> for std::result::Result<T, EFError> {
    fn map_ef(self) -> webx_core::error::Result<T> {
        self.map_err(map_ef_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_ef::error::DatabaseErrorInfo;

    #[test]
    fn business_errors_keep_their_message() {
        let not_found = map_ef_error(EFError::not_found("missing"));
        assert_eq!(not_found.status_code(), 404);
        assert_eq!(not_found.public_detail(), Some("missing"));

        let validation = map_ef_error(EFError::model_validation("bad input"));
        assert_eq!(validation.status_code(), 400);
        assert_eq!(validation.public_detail(), Some("bad input"));

        let conflict = map_ef_error(EFError::concurrency_conflict("busy"));
        assert_eq!(conflict.status_code(), 409);
        assert_eq!(conflict.public_detail(), Some("busy"));
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
    fn query_and_type_conversion_are_a_sanitized_400() {
        for error in [
            EFError::query("SELECT * FROM secret_table"),
            EFError::TypeConversion("boolean for column secret_flag".to_string(), None),
        ] {
            let mapped = map_ef_error(error);
            assert_eq!(mapped.status_code(), 400);
            assert_eq!(mapped.public_detail(), Some(DETAIL_INVALID_REQUEST));
        }
    }

    #[test]
    fn transient_driver_conditions_are_a_retryable_503() {
        let cases = [
            // PostgreSQL/MySQL connection exception, serialization failure, shutdown.
            DatabaseErrorInfo::new("server closed the connection unexpectedly")
                .with_sqlstate("08006"),
            DatabaseErrorInfo::new("deadlock detected").with_sqlstate("40P01"),
            DatabaseErrorInfo::new("the database system is shutting down").with_sqlstate("57P02"),
            // SQLite: no SQLSTATE; the extended result code carries the primary code.
            DatabaseErrorInfo::new("database is locked").with_code("5"),
            DatabaseErrorInfo::new("database table is locked").with_code("6"),
            DatabaseErrorInfo::new("database is locked").with_code("261"),
            DatabaseErrorInfo::new("database is locked").with_code("517"),
            DatabaseErrorInfo::new("database table is locked").with_code("262"),
            // MySQL: lock wait timeout / deadlock.
            DatabaseErrorInfo::new("Lock wait timeout exceeded")
                .with_sqlstate("HY000")
                .with_code("1205"),
            DatabaseErrorInfo::new("Deadlock found when trying to get lock").with_code("1213"),
        ];
        for info in cases {
            let mapped = map_ef_error(EFError::database(info));
            assert_eq!(mapped.status_code(), 503);
            assert!(mapped.retryable());
            assert_eq!(mapped.public_detail(), Some(DETAIL_UNAVAILABLE));
        }
    }

    #[test]
    fn non_transient_driver_conditions_stay_a_hidden_500() {
        for info in [
            DatabaseErrorInfo::new("relation \"secret\" does not exist").with_sqlstate("42P01"),
            DatabaseErrorInfo::new("insert or update violates foreign key constraint")
                .with_sqlstate("23503")
                .with_table("orders"),
            DatabaseErrorInfo::new("disk I/O error").with_code("10"),
            DatabaseErrorInfo::new("driver reported nothing structured"),
        ] {
            let mapped = map_ef_error(EFError::database(info));
            assert_eq!(mapped.status_code(), 500);
            assert_eq!(mapped.public_detail(), None);
            assert!(!mapped.retryable());
        }
    }

    #[test]
    fn connection_and_transaction_are_a_retryable_503() {
        for error in [
            EFError::connection("postgres://user:pass@db/app"),
            EFError::Transaction("rollback failed".to_string(), None),
        ] {
            let mapped = map_ef_error(error);
            assert_eq!(mapped.status_code(), 503);
            assert!(mapped.retryable());
            assert_eq!(mapped.public_detail(), Some(DETAIL_UNAVAILABLE));
        }
    }

    #[test]
    fn server_errors_are_hidden_500_with_operator_detail() {
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

    #[test]
    fn the_extension_trait_maps_errors_and_passes_values_through() {
        let ok: std::result::Result<u32, EFError> = Ok(7);
        assert_eq!(ok.map_ef().expect("ok"), 7);

        let failed: std::result::Result<u32, EFError> = Err(EFError::not_found("gone"));
        let error = failed.map_ef().expect_err("mapped");
        assert_eq!(error.status_code(), 404);
    }
}
