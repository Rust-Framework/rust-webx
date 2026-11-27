//! Authentication/security extension points.
//!
//! Ports shared by application code and the HTTP host:
//!
//! * [`ITokenIssuer`] — mint a token from [`TokenClaims`] (login).
//! * [`ISessionStore`] — revoke / query revoked tokens (logout, forced sign-out);
//!   the authentication middleware consults it after verifying a token.
//! * [`ISessionRepository`] — server-side browser sessions for cookie auth:
//!   opaque id in, `SessionRecord` out; logout is immediate.
//! * [`IPasswordHasher`] — hash/verify passwords as PHC strings (Argon2id by
//!   default).
//! * [`IClientCredentials`] — obtain a **machine** token for a service-to-service
//!   call (OAuth 2.0 client credentials); the implementation owns the token
//!   cache, early refresh and single-flight, so call sites just ask for a token.
//!
//! Two dependency-free primitives for the security-adjacent code every
//! application writes at least once:
//!
//! * [`constant_time_eq`] — compare secrets without leaking where they differ.
//! * [`OneTimeSecret`] — a TTL-bound secret consumed exactly once (installation
//!   token, invite link, password reset), with a [`OneTimeSecretState`] for
//!   telling 401 from 410.
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

mod client_credentials;
mod once;
mod password;
mod refresh;
mod session;
mod session_repo;
mod tokens;

pub use client_credentials::{IClientCredentials, ServiceToken};
pub use once::{constant_time_eq, OneTimeSecret, OneTimeSecretState};
pub use password::IPasswordHasher;
pub use refresh::{
    IRefreshTokenStore, InMemoryRefreshTokenStore, RefreshRotation, RefreshTokenRecord,
    RefreshTokenStatus,
};
pub use session::{ISessionStore, InMemorySessionStore};
pub use session_repo::{ISessionRepository, InMemorySessionRepository, SessionRecord};
pub use tokens::{ITokenIssuer, IssuedToken, TokenClaims};
