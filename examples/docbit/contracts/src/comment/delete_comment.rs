//! `DELETE /api/comments/{id}` — author or admin, soft delete.

use serde::Deserialize;
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct DeleteCommentRequest {
    #[fromclaim]
    pub user: IdentityUser,
    pub id: String,
}

#[delete("/api/comments/{id}")]
#[authorize]
impl IRequest<String> for DeleteCommentRequest {}
