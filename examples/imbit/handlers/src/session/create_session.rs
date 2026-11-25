//! `POST /api/session` — exchange a bearer token for browser session cookies.
//!
//! The endpoint is a *browser adapter*, not a second login: the caller has
//! already authenticated with a bearer token, and the handler only turns that
//! identity into the two cookies a browser can carry automatically:
//!
//! * `HttpOnly` session cookie — opaque id resolved by `ISessionRepository` on
//!   every request and on every WebSocket upgrade;
//! * readable CSRF cookie — the browser app mirrors it in `X-CSRF-Token`.
//!
//! The response is self-controlled (`IRequest<Response>`) because it writes JSON
//! **and** two independent `Set-Cookie` headers, which a DTO cannot express.

use std::sync::Arc;

use webx::*;

use imbit_contracts::session::{CreateSessionRequest, SessionResponse};

#[derive(Inject)]
pub struct CreateSessionHandler {
    #[inject]
    scheme: Arc<SessionCookieScheme>,
    #[inject]
    csrf: Arc<CsrfConfig>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateSessionRequest, Response> for CreateSessionHandler {
    async fn handle(&mut self, request: CreateSessionRequest) -> ActionResult<Response> {
        let user = &request.user;

        // Capture only the claims the session needs; the session (not the JWT)
        // becomes the authority for name/email/tenant/roles until it is deleted.
        // `issue_session` assigns the id and the timestamps (issued/expiry), so
        // the placeholder clock values below are never persisted.
        let mut record = SessionRecord::new("", &user.id, 0, 0).with_name(user.display_name());
        if !user.email.is_empty() {
            record = record.with_email(&user.email);
        }
        if let Some(tenant) = user.tenant() {
            record = record.with_tenant(tenant);
        }
        record = record
            .with_roles(user.roles.iter().cloned())
            .with_permissions(user.permissions.iter().cloned());

        let session = self.scheme.issue_session(record).await?;
        let csrf_token = self.csrf.issue_token();

        // The cookies are the payload; the JSON body is only a confirmation.
        let body = serde_json::to_vec(&SessionResponse::from_user(user))
            .map_err(|error| Error::internal(format!("session response encode failed: {error}")))?;

        Ok(Box::new(
            ContentResult::bytes(body, "application/json")
                .header("set-cookie", session.set_cookie)
                .header("set-cookie", self.csrf.set_cookie(&csrf_token)),
        ))
    }
}
