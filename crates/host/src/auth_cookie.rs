//! Cookie authentication scheme (browser / BFF).
//!
//! The browser stores an **opaque session id** in an `HttpOnly` cookie; the
//! server resolves it through [`ISessionRepository`]. This is the BFF pattern:
//! no JWT in a cookie (nothing for XSS to read, nothing that outlives the
//! server-side record), logout and "sign out everywhere" are immediate.
//!
//! The scheme deliberately only *authenticates*. Login/logout endpoints own the
//! `Set-Cookie`/`Clear-Site-Data` responses:
//!
//! ```ignore
//! let session = scheme
//!     .issue_session(SessionRecord::new("", user.id)
//!         .with_tenant(tenant)
//!         .with_roles(user.roles))
//!     .await?;
//! response.set_header("set-cookie", &session.set_cookie);
//! ```
//!
//! `Cookie` + CSRF: a cookie-authenticated endpoint must also be behind
//! [`CsrfMiddleware`](crate::csrf::CsrfMiddleware). The host refuses to build
//! when a `session-cookie` scheme is registered without `use_csrf`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use webx_core::auth::{IAuthenticationScheme, IClaims};
use webx_core::error::{Error, Result};
use webx_core::http::IHttpContext;
use webx_core::security::{ISessionRepository, SessionRecord};

/// Stable scheme name used by the host's startup guard.
pub const SESSION_COOKIE_SCHEME_NAME: &str = "session-cookie";

/// `SameSite` cookie attribute.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SameSite {
    /// Sent on same-site requests and top-level cross-site navigations. Default.
    #[default]
    Lax,
    /// Never sent on cross-site requests.
    Strict,
    /// Sent on cross-site requests; requires `Secure`.
    None,
}

impl SameSite {
    /// The wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lax => "Lax",
            Self::Strict => "Strict",
            Self::None => "None",
        }
    }
}

/// Cookie attributes for the session id.
#[derive(Debug, Clone)]
pub struct CookieOptions {
    /// Cookie name. Default `__Host-webx_session`.
    pub name: String,
    /// Cookie path. Default `/`.
    pub path: String,
    /// `Secure` attribute. Default `true` (localhost is a secure context).
    pub secure: bool,
    /// `HttpOnly` attribute. Default `true`; a browser session id must never be
    /// readable from JavaScript.
    pub http_only: bool,
    /// `SameSite` attribute. Default `Lax`.
    pub same_site: SameSite,
    /// Idle timeout: a session unused for longer is invalid. `ZERO` disables it.
    pub idle_timeout: Duration,
    /// Absolute lifetime; the session dies at this point regardless of activity.
    pub absolute_timeout: Duration,
    /// Refresh `last_seen` on every authenticated request (idle sliding).
    /// Default `false`: it costs one repository write per request.
    pub sliding: bool,
}

impl Default for CookieOptions {
    fn default() -> Self {
        Self {
            name: "__Host-webx_session".to_string(),
            path: "/".to_string(),
            secure: true,
            http_only: true,
            same_site: SameSite::Lax,
            idle_timeout: Duration::from_secs(30 * 60),
            absolute_timeout: Duration::from_secs(12 * 60 * 60),
            sliding: false,
        }
    }
}

impl CookieOptions {
    /// Validate the attributes. Called by [`SessionCookieScheme::new`], so a
    /// misconfigured cookie fails startup instead of shipping without
    /// `HttpOnly`/`Secure`.
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(Error::di(
                "session cookie name must not be empty".to_string(),
            ));
        }
        if self
            .name
            .chars()
            .any(|c| c.is_control() || matches!(c, ' ' | ';' | '=' | ','))
        {
            return Err(Error::di(format!(
                "session cookie name '{}' contains an invalid character",
                self.name
            )));
        }
        if !self.http_only {
            return Err(Error::di(
                "session cookies must be HttpOnly; refusing to expose the session id to JavaScript"
                    .to_string(),
            ));
        }
        if !self.path.starts_with('/') {
            return Err(Error::di(
                "session cookie path must start with '/'".to_string(),
            ));
        }
        if self.absolute_timeout.is_zero() {
            return Err(Error::di(
                "session absolute_timeout must be greater than zero".to_string(),
            ));
        }
        if self.same_site == SameSite::None && !self.secure {
            return Err(Error::di(
                "SameSite=None requires Secure; refusing to build an insecure cookie".to_string(),
            ));
        }
        if self.name.starts_with("__Host-") && (!self.secure || self.path != "/") {
            return Err(Error::di(
                "__Host- cookies require Secure and Path=/".to_string(),
            ));
        }
        Ok(())
    }

    /// `Set-Cookie` value for a new session id.
    pub fn set_cookie(&self, session_id: &str) -> String {
        let mut cookie = format!(
            "{}={}; Path={}; Max-Age={}",
            self.name,
            session_id,
            self.path,
            self.absolute_timeout.as_secs()
        );
        if self.http_only {
            cookie.push_str("; HttpOnly");
        }
        if self.secure {
            cookie.push_str("; Secure");
        }
        cookie.push_str("; SameSite=");
        cookie.push_str(self.same_site.as_str());
        cookie
    }

    /// `Set-Cookie` value that clears the session cookie.
    pub fn clear_cookie(&self) -> String {
        let mut cookie = format!("{}=; Path={}; Max-Age=0", self.name, self.path);
        if self.http_only {
            cookie.push_str("; HttpOnly");
        }
        if self.secure {
            cookie.push_str("; Secure");
        }
        cookie.push_str("; SameSite=");
        cookie.push_str(self.same_site.as_str());
        cookie
    }
}

/// A freshly issued browser session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedSession {
    /// Opaque session id (also embedded in `set_cookie`).
    pub session_id: String,
    /// Ready-to-send `Set-Cookie` value.
    pub set_cookie: String,
    /// Absolute expiry (Unix seconds).
    pub expires_at_unix_secs: u64,
}

/// Cookie-based [`IAuthenticationScheme`].
pub struct SessionCookieScheme {
    repository: Arc<dyn ISessionRepository>,
    options: CookieOptions,
}

impl SessionCookieScheme {
    /// Build a scheme over `repository`; validates [`CookieOptions`].
    pub fn new(repository: Arc<dyn ISessionRepository>, options: CookieOptions) -> Result<Self> {
        options.validate()?;
        Ok(Self {
            repository,
            options,
        })
    }

    /// The cookie attributes in effect.
    pub fn options(&self) -> &CookieOptions {
        &self.options
    }

    /// The repository in effect.
    pub fn repository(&self) -> &Arc<dyn ISessionRepository> {
        &self.repository
    }

    /// Issue a session for `record.subject` with default record fields.
    pub async fn issue_for(&self, subject: impl Into<String>) -> Result<IssuedSession> {
        let subject = subject.into();
        if subject.trim().is_empty() {
            return Err(Error::validation(
                "a session subject must not be empty".to_string(),
            ));
        }
        let now = now_unix_secs();
        let record = SessionRecord::new(String::new(), subject, now, now);
        self.issue_session(record).await
    }

    /// Issue a session from a caller-built record (name/email/tenant/roles/
    /// permissions). The scheme assigns the id and the absolute expiry.
    pub async fn issue_session(&self, mut record: SessionRecord) -> Result<IssuedSession> {
        if record.subject.trim().is_empty() {
            return Err(Error::validation(
                "a session subject must not be empty".to_string(),
            ));
        }
        let now = now_unix_secs();
        record.id = new_session_id();
        record.issued_at_unix_secs = now;
        record.last_seen_unix_secs = now;
        record.expires_at_unix_secs = now.saturating_add(self.options.absolute_timeout.as_secs());
        self.repository.create(record.clone()).await?;
        Ok(IssuedSession {
            set_cookie: self.options.set_cookie(&record.id),
            session_id: record.id,
            expires_at_unix_secs: record.expires_at_unix_secs,
        })
    }

    /// Delete one session (logout).
    pub async fn revoke_session(&self, session_id: &str) -> Result<()> {
        self.repository.delete(session_id).await
    }

    /// Delete every session of `subject` ("sign out everywhere").
    pub async fn revoke_all_for_subject(&self, subject: &str) -> Result<u64> {
        self.repository.delete_all_for_subject(subject).await
    }

    fn session_id<'a>(&self, ctx: &'a dyn IHttpContext) -> Option<&'a str> {
        let header = ctx.request().header("cookie")?;
        cookie_value(header, &self.options.name)
    }
}

#[async_trait::async_trait]
impl IAuthenticationScheme for SessionCookieScheme {
    fn name(&self) -> &'static str {
        SESSION_COOKIE_SCHEME_NAME
    }

    fn session_cookie_name(&self) -> Option<&str> {
        Some(self.options.name.as_str())
    }

    /// The repository this scheme authenticates against, so a live WebSocket
    /// session can be re-checked after the session is deleted
    /// (`WebSocketLimits::session_revalidation`).
    fn session_repository(&self) -> Option<Arc<dyn webx_core::security::ISessionRepository>> {
        Some(Arc::clone(&self.repository))
    }

    fn matches(&self, ctx: &dyn IHttpContext) -> bool {
        // A Bearer request belongs to the JWT/API-key schemes; never let a
        // stale session cookie shadow an explicit Authorization header.
        if ctx.request().header("authorization").is_some() {
            return false;
        }
        self.session_id(ctx).is_some()
    }

    async fn authenticate(&self, ctx: &mut dyn IHttpContext) -> Result<Option<Box<dyn IClaims>>> {
        let Some(session_id) = self.session_id(ctx) else {
            return Ok(None);
        };
        let now = now_unix_secs();
        let Some(record) = self.repository.get(session_id).await? else {
            return Ok(None);
        };
        if record.subject.trim().is_empty() {
            // A session without a subject is not an identity. Treat it as a
            // corrupt credential, delete it and continue anonymously so a
            // protected endpoint still answers 401.
            tracing::warn!("deleting a session with an empty subject");
            if let Err(error) = self.repository.delete(session_id).await {
                tracing::warn!(%error, "failed to delete a subjectless session");
            }
            return Ok(None);
        }
        if record.is_expired(now) || record.is_idle_expired(now, self.options.idle_timeout) {
            // Expired sessions are credentials that no longer exist: remove the
            // record so a persistent repository does not keep dead rows.
            if let Err(error) = self.repository.delete(session_id).await {
                tracing::warn!(%error, "failed to delete an expired session");
            }
            return Ok(None);
        }
        if self.options.sliding {
            if let Err(error) = self.repository.touch(session_id, now).await {
                // A failed activity write must not turn a valid session into an
                // authentication failure; log and continue with the old value.
                tracing::warn!(%error, "failed to touch a sliding session");
            }
        }
        Ok(Some(Box::new(SessionClaims::from_record(&record))))
    }
}

struct SessionClaims {
    subject: String,
    roles: Vec<String>,
    permissions: Vec<String>,
    raw: HashMap<String, String>,
}

impl SessionClaims {
    fn from_record(record: &SessionRecord) -> Self {
        let mut raw = HashMap::new();
        raw.insert("sub".to_string(), record.subject.clone());
        if let Some(name) = &record.name {
            raw.insert("name".to_string(), name.clone());
        }
        if let Some(email) = &record.email {
            raw.insert("email".to_string(), email.clone());
        }
        if let Some(tenant) = &record.tenant_id {
            raw.insert("tenant_id".to_string(), tenant.clone());
        }
        Self {
            subject: record.subject.clone(),
            roles: record.roles.clone(),
            permissions: record.permissions.clone(),
            raw,
        }
    }
}

impl IClaims for SessionClaims {
    fn subject(&self) -> &str {
        &self.subject
    }

    fn roles(&self) -> &[String] {
        &self.roles
    }

    fn permissions(&self) -> &[String] {
        &self.permissions
    }

    fn claims(&self) -> &HashMap<String, String> {
        &self.raw
    }

    fn clone_box(&self) -> Box<dyn IClaims> {
        Box::new(Self {
            subject: self.subject.clone(),
            roles: self.roles.clone(),
            permissions: self.permissions.clone(),
            raw: self.raw.clone(),
        })
    }
}

/// Extract a cookie value from a `Cookie` header.
///
/// Values are returned verbatim (trimmed); the session ids and CSRF tokens the
/// framework issues are hex, so no percent-decoding is needed.
pub(crate) fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    for part in header.split(';') {
        let part = part.trim();
        if let Some((key, value)) = part.split_once('=') {
            if key.trim() == name {
                return Some(value.trim());
            }
        }
    }
    None
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// 256-bit opaque session id (two UUIDv4s, hex). No claims, no signature, no
/// way for a client to extend it.
fn new_session_id() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_value_finds_named_cookie_and_ignores_others() {
        let header = "a=1; __Host-webx_session=abc123; b=2";
        assert_eq!(cookie_value(header, "__Host-webx_session"), Some("abc123"));
        assert_eq!(cookie_value(header, "missing"), None);
        assert_eq!(cookie_value("no-equals", "x"), None);
    }

    #[test]
    fn default_cookie_is_hardened() {
        let options = CookieOptions::default();
        options.validate().unwrap();
        let cookie = options.set_cookie("sid");
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("Secure"));
        assert!(cookie.contains("SameSite=Lax"));
        assert!(cookie.contains("Path=/"));
    }

    #[tokio::test]
    async fn empty_subject_session_is_rejected() {
        let repository = std::sync::Arc::new(webx_core::security::InMemorySessionRepository::new());
        let scheme =
            SessionCookieScheme::new(repository, CookieOptions::default()).expect("scheme");
        let error = scheme.issue_for("  ").await.expect_err("empty subject");
        assert_eq!(error.code(), "validation_failed");
    }

    #[test]
    fn http_only_is_mandatory() {
        let options = CookieOptions {
            http_only: false,
            ..CookieOptions::default()
        };
        assert!(options.validate().is_err());
    }

    #[test]
    fn same_site_none_requires_secure() {
        let options = CookieOptions {
            same_site: SameSite::None,
            secure: false,
            ..CookieOptions::default()
        };
        let error = options.validate().unwrap_err();
        assert!(error.code() == "dependency_injection", "got {error:?}");
    }

    #[test]
    fn host_prefixed_cookie_requires_secure_root_path() {
        let options = CookieOptions {
            secure: false,
            name: "__Host-session".to_string(),
            ..CookieOptions::default()
        };
        assert!(options.validate().is_err());
    }

    #[test]
    fn new_session_id_is_64_hex_chars_and_unique() {
        let a = new_session_id();
        let b = new_session_id();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
