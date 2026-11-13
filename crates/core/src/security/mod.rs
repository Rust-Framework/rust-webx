//! Authentication/security extension points.
//!
//! Ports shared by application code and the HTTP host:
//!
//! * [`ITokenIssuer`] — mint a token from [`TokenClaims`] (login).
//! * [`ISessionStore`] — revoke / query revoked tokens (logout, forced sign-out);
//!   the authentication middleware consults it after verifying a token.
//!
//! The traits are dependency-free so they can be implemented anywhere; the
//! implementations that need third-party crates live in `rust-webx-host`
//! (e.g. `JwtTokenIssuer`), while the ones that need nothing live here
//! ([`InMemorySessionStore`]).

mod session;
mod tokens;

pub use session::{ISessionStore, InMemorySessionStore};
pub use tokens::{ITokenIssuer, IssuedToken, TokenClaims};
