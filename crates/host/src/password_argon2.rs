//! Argon2id [`IPasswordHasher`] implementation — the framework default.
//!
//! Enabled by the `password-argon2` feature (on by default). `HostBuilder`
//! registers one automatically in DI, so a handler only injects
//! `Arc<dyn IPasswordHasher>`; register a different implementation — or this one
//! with different [`Argon2Params`] — to replace/configure it.
//!
//! Both operations are CPU-bound and run on `tokio::task::spawn_blocking`
//! threads, so a login never stalls an async worker.
//!
//! ```ignore
//! // configure (e.g. raise the cost for a new deployment)
//! let hasher = Argon2PasswordHasher::with_params(Argon2Params::new(65536, 3, 1));
//! // or hash/verify through the port
//! let phc = hasher.hash(&password).await?;
//! if hasher.verify(&password, &stored).await? && hasher.needs_rehash(&stored) { /* upgrade */ }
//! ```

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use rand_core::OsRng;
use webx_core::error::{Error, Result};
use webx_core::security::IPasswordHasher;

/// Algorithm identifier embedded in the PHC string produced by this hasher.
const ARGON2ID_IDENT: &str = "argon2id";

/// Argon2 cost parameters.
///
/// The defaults mirror `argon2::Argon2::default()` (19 MiB memory, 2 iterations,
/// 1 lane — the OWASP baseline), so hashes created by hand-written
/// `Argon2::default()` code keep verifying after a migration to this port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Argon2Params {
    /// Memory cost in KiB (`m`).
    pub memory_kib: u32,
    /// Iterations / time cost (`t`).
    pub iterations: u32,
    /// Lanes / parallelism (`p`).
    pub parallelism: u32,
}

impl Argon2Params {
    /// Build a parameter set. Values are validated when hashing; invalid
    /// combinations surface as [`Error::PasswordHash`] instead of panicking.
    pub const fn new(memory_kib: u32, iterations: u32, parallelism: u32) -> Self {
        Self {
            memory_kib,
            iterations,
            parallelism,
        }
    }

    /// The `argon2` crate defaults (19 MiB / 2 / 1).
    pub const fn owasp_default() -> Self {
        Self::new(19 * 1024, 2, 1)
    }

    /// Convert to the `argon2` crate's own parameter type.
    fn to_argon2_params(self) -> Result<Params> {
        Params::new(
            self.memory_kib,
            self.iterations,
            self.parallelism,
            None,
        )
        .map_err(|error| {
            Error::PasswordHash(format!(
                "invalid Argon2 parameters (m={}, t={}, p={}): {error}",
                self.memory_kib, self.iterations, self.parallelism
            ))
        })
    }
}

impl Default for Argon2Params {
    fn default() -> Self {
        Self::owasp_default()
    }
}

/// Argon2id password hasher.
///
/// Uses Argon2id (v0x13), a fresh random salt per hash, and PHC output, so the
/// algorithm and cost parameters are stored with each hash.
#[derive(Debug, Clone, Copy, Default)]
pub struct Argon2PasswordHasher {
    params: Argon2Params,
}

impl Argon2PasswordHasher {
    /// The framework default (Argon2id with [`Argon2Params::owasp_default`]).
    pub fn new() -> Self {
        Self::default()
    }

    /// Use explicit cost parameters (also used for `needs_rehash` decisions).
    pub fn with_params(params: Argon2Params) -> Self {
        Self { params }
    }

    /// The parameters this hasher produces hashes with.
    pub fn params(&self) -> Argon2Params {
        self.params
    }
}

/// Parse and fully verify one PHC string against `password` — runs on a blocking
/// thread.
fn verify_blocking(password: &str, stored_hash: &str) -> Result<bool> {
    let parsed = PasswordHash::new(stored_hash).map_err(|error| {
        Error::PasswordHash(format!("stored password hash is malformed: {error}"))
    })?;

    // A PHC string without salt or output cannot represent a password hash; the
    // PHC parser accepts the shape, so verify the fields exists before treating a
    // parse as a usable stored hash.
    if parsed.salt.is_none() || parsed.hash.is_none() {
        return Err(Error::PasswordHash(
            "stored password hash is malformed: missing salt or hash output".to_string(),
        ));
    }
    // Reject unusable parameter fields (e.g. `m=notanumber`) explicitly: a broken
    // stored record must surface as an error, not as "wrong password".
    Params::try_from(&parsed).map_err(|error| {
        Error::PasswordHash(format!(
            "stored password hash parameters are invalid: {error}"
        ))
    })?;

    // `verify_password` reads the algorithm's parameters from the PHC string, so
    // the hasher's own configured cost does not prevent older hashes verifying.
    let algorithm = match parsed.algorithm.as_str() {
        ARGON2ID_IDENT => Algorithm::Argon2id,
        "argon2i" => Algorithm::Argon2i,
        "argon2d" => Algorithm::Argon2d,
        other => {
            return Err(Error::PasswordHash(format!(
                "stored password hash uses an unsupported algorithm '{other}'"
            )))
        }
    };
    let argon2 = Argon2::new(algorithm, Version::V0x13, Params::DEFAULT);

    match argon2.verify_password(password.as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        // A mismatch is an ordinary authentication failure, not corruption.
        Err(argon2::password_hash::Error::Password) => Ok(false),
        Err(error) => Err(Error::PasswordHash(format!(
            "password verification failed: {error}"
        ))),
    }
}

#[async_trait::async_trait]
impl IPasswordHasher for Argon2PasswordHasher {
    async fn hash(&self, password: &str) -> Result<String> {
        let params = self.params;
        let password = password.to_owned();
        // Dry-run the parameter validation before spawning: a bad configuration
        // must fail fast with a typed error, not inside a join handle.
        params.to_argon2_params()?;

        tokio::task::spawn_blocking(move || {
            let argon2 = Argon2::new(
                Algorithm::Argon2id,
                Version::V0x13,
                params.to_argon2_params()?,
            );
            let salt = SaltString::generate(&mut OsRng);
            argon2
                .hash_password(password.as_bytes(), &salt)
                .map(|hash| hash.to_string())
                .map_err(|error| Error::PasswordHash(format!("password hashing failed: {error}")))
        })
        .await
        .map_err(|error| {
            Error::PasswordHash(format!("password hashing task failed: {error}"))
        })?
    }

    async fn verify(&self, password: &str, stored_hash: &str) -> Result<bool> {
        let password = password.to_owned();
        let stored_hash = stored_hash.to_owned();
        tokio::task::spawn_blocking(move || verify_blocking(&password, &stored_hash))
            .await
            .map_err(|error| {
                Error::PasswordHash(format!("password verification task failed: {error}"))
            })?
    }

    fn needs_rehash(&self, stored_hash: &str) -> bool {
        let Ok(parsed) = PasswordHash::new(stored_hash) else {
            // Unparseable: cannot be trusted, upgrade it on the next login.
            return true;
        };
        if parsed.salt.is_none()
            || parsed.hash.is_none()
            || parsed.algorithm.as_str() != ARGON2ID_IDENT
            || parsed.version != Some(0x13)
        {
            // 0x13 is Argon2 version v=19 (`Version::V0x13`); the PHC field is a
            // plain decimal, not the enum.
            return true;
        }
        let Ok(stored) = Params::try_from(&parsed) else {
            return true;
        };
        stored.m_cost() != self.params.memory_kib
            || stored.t_cost() != self.params.iterations
            || stored.p_cost() != self.params.parallelism
    }
}
