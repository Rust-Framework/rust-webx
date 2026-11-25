//! `DELETE /api/blog/{slug}` — soft delete (author or admin).

use serde::Deserialize;
use webx::*;

#[derive(Default, Deserialize, WebxRequestMeta)]
pub struct DeleteBlogPostRequest {
    pub slug: String,
    #[fromclaim]
    pub user: IdentityUser,
}

#[delete("/api/blog/{slug}")]
#[authorize]
impl IRequest<String> for DeleteBlogPostRequest {}
