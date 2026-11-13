//! Authentication/security extension points.
//!
//! Ports shared by application code and the HTTP host:
//!
//! * [`ITokenIssuer`] — mint a token from [`TokenClaims`] (login).
//! * [`ISessionStore`] — revoke / query revoked tokens (logout, forced sign-out);
//!   the authentication middleware consults it after verifying a token.
//! * [`IPasswordHasher`] — hash/verify passwords as PHC strings (Argon2id by
//!   default).
//!
//! # How the framework uses these ports
//!
//! The ports exist so business crates write "call the port" code, never crypto
//! or session-plumbing code:
//!
//! * **Login** — a handler injects `Arc<dyn ITokenIssuer>`, calls
//!   [`ITokenIssuer::issue`], and returns `IssuedToken::token`. The password is
//!   checked with `Arc<dyn IPasswordHasher>` (`hash` on registration, `verify` on
//!   login, `needs_rehash` to upgrade old hashes).
//! * **Authenticated requests** — the authentication middleware verifies the
//!   token, then (when an `ISessionStore` is registered) asks
//!   [`ISessionStore::is_revoked`] about the token's `jti`; revoked → 401.
//! * **Logout** — the handler calls [`ISessionStore::revoke`] with the `jti` it
//!   stored (or received) at login; no wait for `exp` is needed.
//!
//! The traits are dependency-free so they can be implemented anywhere; the
//! implementations that need third-party crates live in `rust-webx-host`
//! (`JwtTokenIssuer`, `Argon2PasswordHasher`), while the ones that need nothing
//! live here ([`InMemorySessionStore`]).

mod password;
mod session;
mod tokens;

pub use password::IPasswordHasher;
pub use session::{ISessionStore, InMemorySessionStore};
pub use tokens::{ITokenIssuer, IssuedToken, TokenClaims};
