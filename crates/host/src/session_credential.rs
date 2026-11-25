//! Re-check of the credential that authorized a WebSocket upgrade (2.9.2).
//!
//! A WebSocket session captures its identity at handshake time. Without a
//! re-check, logout / "sign out everywhere" / a deleted cookie session leaves
//! the socket usable until it disconnects. The host builds a
//! [`SessionCredentialProvider`] from the registered authentication schemes and
//! session stores when `WebSocketLimits::session_revalidation` is enabled; it
//! returns a crate-internal checker whose `is_valid` the session polls. The
//! checker is deliberately **not** a public port: the JWT and cookie checks
//! below are the only families the host can re-check in this release, so a
//! public trait would add a concept applications cannot plug into.
//!
//! Two credential families are supported:
//!
//! * **JWT** — the `jti` claim is checked against
//!   [`ISessionStore::is_revoked`]; a store error is a transient error (retried
//!   by the session), a `true` verdict closes it.
//! * **Cookie session** — the opaque session id is looked up in the scheme's
//!   [`ISessionRepository`]; a missing/expired record closes the session.
//!
//! API keys are deliberately not re-checked: the key-validation port has no
//! revocation lookup in this release, so a session authorized by an API key is
//! bounded by the other lifetime limits instead. This is documented rather than
//! silently treated as always valid.
//!
//! [`ISessionStore::is_revoked`]: webx_core::security::ISessionStore::is_revoked
//! [`ISessionRepository`]: webx_core::security::ISessionRepository

use std::sync::Arc;

use async_trait::async_trait;

use webx_core::error::Result;
use webx_core::http::IHttpContext;
use webx_core::security::{ISessionRepository, ISessionStore};
use webx_core::time::IClock;

/// Crate-internal re-check of the credential that authorized an upgrade.
#[async_trait]
pub(crate) trait ConnectionCredential: Send + Sync {
    async fn is_valid(&self) -> Result<bool>;
}

/// Builds one [`ConnectionCredential`] from the upgrade request's headers and
/// claims. Owned by the WebSocket middleware and only built when session
/// revalidation is enabled (`None` otherwise, so the per-upgrade cost stays
/// zero for hosts that do not opt in).
pub(crate) struct SessionCredentialProvider {
    session_store: Option<Arc<dyn ISessionStore>>,
    session_repository: Option<Arc<dyn ISessionRepository>>,
    session_cookie_name: Option<String>,
    clock: Arc<dyn IClock>,
}

impl SessionCredentialProvider {
    pub(crate) fn new(
        session_store: Option<Arc<dyn ISessionStore>>,
        session_repository: Option<Arc<dyn ISessionRepository>>,
        session_cookie_name: Option<String>,
        clock: Arc<dyn IClock>,
    ) -> Self {
        Self {
            session_store,
            session_repository,
            session_cookie_name,
            clock,
        }
    }

    /// Whether at least one credential family can actually be checked.
    pub(crate) fn is_useful(&self) -> bool {
        (self.session_store.is_some() && self.session_cookie_name.is_some())
            || self.session_store.is_some()
            || self.session_repository.is_some()
    }

    /// Build the credential for one authenticated upgrade, if this host can
    /// check it.
    pub(crate) fn credential_for(
        &self,
        ctx: &dyn IHttpContext,
    ) -> Option<Arc<dyn ConnectionCredential>> {
        // A cookie scheme takes precedence: a request authenticated by the
        // session cookie must be re-validated against that repository even if
        // the claims happen to carry a jti.
        if let (Some(repository), Some(cookie_name)) =
            (&self.session_repository, &self.session_cookie_name)
        {
            if let Some(session_id) = ctx
                .request()
                .header("cookie")
                .and_then(|header| crate::auth_cookie::cookie_value(header, cookie_name))
            {
                return Some(Arc::new(CookieSessionCredential {
                    repository: Arc::clone(repository),
                    session_id: session_id.to_string(),
                    clock: Arc::clone(&self.clock),
                }));
            }
        }
        let jti = ctx.claims().and_then(|claims| claims.jti())?;
        let store = self.session_store.as_ref()?;
        Some(Arc::new(JwtSessionCredential {
            store: Arc::clone(store),
            jti: jti.to_string(),
        }))
    }
}

struct JwtSessionCredential {
    store: Arc<dyn ISessionStore>,
    jti: String,
}

#[async_trait]
impl ConnectionCredential for JwtSessionCredential {
    async fn is_valid(&self) -> Result<bool> {
        self.store
            .is_revoked(&self.jti)
            .await
            .map(|revoked| !revoked)
    }
}

struct CookieSessionCredential {
    repository: Arc<dyn ISessionRepository>,
    session_id: String,
    clock: Arc<dyn IClock>,
}

#[async_trait]
impl ConnectionCredential for CookieSessionCredential {
    async fn is_valid(&self) -> Result<bool> {
        let now = self.clock.now_unix();
        self.repository
            .get(&self.session_id)
            .await
            .map(|record| record.is_some_and(|record| !record.is_expired(now)))
    }
}
