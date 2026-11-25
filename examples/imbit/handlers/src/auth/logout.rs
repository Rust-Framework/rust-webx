//! `POST /api/logout` — revoke the caller's own token.

use std::sync::Arc;

use webx::*;

use imbit_contracts::auth::LogoutRequest;

#[derive(Inject)]
pub struct LogoutHandler {
    #[inject]
    sessions: Arc<dyn ISessionStore>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<LogoutRequest, OperationResult> for LogoutHandler {
    async fn handle(&mut self, request: LogoutRequest) -> ActionResult<OperationResult> {
        // Only the presented token is revoked: its `jti` comes from the verified
        // JWT, so a caller cannot revoke somebody else's session.
        self.sessions.revoke(&request.jti).await?;

        Ok(OperationResult::success("token revoked"))
    }
}
