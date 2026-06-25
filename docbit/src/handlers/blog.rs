//! Blog handlers — filesystem-backed posts per user.

use std::sync::Arc;

use rust_webapp::*;

use crate::contracts::blog::*;

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<ListBlogCategoriesRequest, Vec<BlogCategoryCount>>)]
pub struct ListBlogCategoriesHandler {
    blog: Arc<dyn IBlogService>,
}

#[rust_dicore::inject_attr(
    singleton,
    as = dyn IRequestHandler<CreateBlogCategoryRequest, BlogCategoryDef>
)]
pub struct CreateBlogCategoryHandler {
    blog: Arc<dyn IBlogService>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<ListBlogPostsRequest, Vec<BlogPostSummary>>)]
pub struct ListBlogPostsHandler {
    blog: Arc<dyn IBlogService>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<ListMyBlogPostsRequest, Vec<BlogPostSummary>>)]
pub struct ListMyBlogPostsHandler {
    blog: Arc<dyn IBlogService>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<GetBlogPostRequest, BlogPostModel>)]
pub struct GetBlogPostHandler {
    blog: Arc<dyn IBlogService>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<CreateBlogPostRequest, BlogPostModel>)]
pub struct CreateBlogPostHandler {
    blog: Arc<dyn IBlogService>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<UpdateBlogPostRequest, BlogPostModel>)]
pub struct UpdateBlogPostHandler {
    blog: Arc<dyn IBlogService>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<DeleteBlogPostRequest, String>)]
pub struct DeleteBlogPostHandler {
    blog: Arc<dyn IBlogService>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListBlogCategoriesRequest, Vec<BlogCategoryCount>>
    for ListBlogCategoriesHandler
{
    async fn handle(&self, _req: ListBlogCategoriesRequest) -> Result<Vec<BlogCategoryCount>> {
        self.blog
            .list_categories()
            .map_err(|e| Error::Internal(e))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateBlogCategoryRequest, BlogCategoryDef> for CreateBlogCategoryHandler {
    async fn handle(&self, _: CreateBlogCategoryRequest) -> Result<BlogCategoryDef> {
        unreachable!("handle_with_claims is always called")
    }

    async fn handle_with_claims(
        &self,
        req: CreateBlogCategoryRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<BlogCategoryDef> {
        let _claims = claims.ok_or_else(|| Error::Http("Not authenticated".into()))?;
        self.blog
            .upsert_category(&req.id, &req.name)
            .map_err(map_blog_err)
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListBlogPostsRequest, Vec<BlogPostSummary>> for ListBlogPostsHandler {
    async fn handle(&self, _req: ListBlogPostsRequest) -> Result<Vec<BlogPostSummary>> {
        self.blog
            .list_all_posts()
            .map_err(|e| Error::Internal(e))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListMyBlogPostsRequest, Vec<BlogPostSummary>> for ListMyBlogPostsHandler {
    async fn handle(&self, _: ListMyBlogPostsRequest) -> Result<Vec<BlogPostSummary>> {
        unreachable!("handle_with_claims is always called")
    }

    async fn handle_with_claims(
        &self,
        _: ListMyBlogPostsRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<Vec<BlogPostSummary>> {
        let claims = claims.ok_or_else(|| Error::Http("Not authenticated".into()))?;
        self.blog
            .list_user_posts(claims.subject())
            .map_err(|e| Error::Internal(e))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetBlogPostRequest, BlogPostModel> for GetBlogPostHandler {
    async fn handle(&self, req: GetBlogPostRequest) -> Result<BlogPostModel> {
        self.blog
            .get_post(&req.slug)
            .map_err(|e| {
                if e.contains("not found") {
                    Error::NotFound(e)
                } else {
                    Error::Internal(e)
                }
            })
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateBlogPostRequest, BlogPostModel> for CreateBlogPostHandler {
    async fn handle(&self, _: CreateBlogPostRequest) -> Result<BlogPostModel> {
        unreachable!("handle_with_claims is always called")
    }

    async fn handle_with_claims(
        &self,
        req: CreateBlogPostRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<BlogPostModel> {
        let claims = claims.ok_or_else(|| Error::Http("Not authenticated".into()))?;
        let user_name = claims
            .get_username()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Author".to_string());
        let category = req.category.unwrap_or_else(|| "rust".to_string());
        self.blog
            .create_post(
                claims.subject(),
                &user_name,
                &req.slug,
                &req.title,
                &req.summary,
                &req.content,
                &req.tags,
                &category,
                &req.published_at,
            )
            .map_err(map_blog_err)
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateBlogPostRequest, BlogPostModel> for UpdateBlogPostHandler {
    async fn handle(&self, _: UpdateBlogPostRequest) -> Result<BlogPostModel> {
        unreachable!("handle_with_claims is always called")
    }

    async fn handle_with_claims(
        &self,
        req: UpdateBlogPostRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<BlogPostModel> {
        let claims = claims.ok_or_else(|| Error::Http("Not authenticated".into()))?;
        let role = if claims.has_role("admin") {
            "admin"
        } else {
            "user"
        };
        self.blog
            .update_post(
                claims.subject(),
                role,
                &req.slug,
                req.title.as_deref(),
                req.summary.as_deref(),
                req.content.as_deref(),
                req.tags.as_deref(),
                req.category.as_deref(),
                req.published_at.as_deref(),
            )
            .map_err(map_blog_err)
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteBlogPostRequest, String> for DeleteBlogPostHandler {
    async fn handle(&self, _: DeleteBlogPostRequest) -> Result<String> {
        unreachable!("handle_with_claims is always called")
    }

    async fn handle_with_claims(
        &self,
        req: DeleteBlogPostRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<String> {
        let claims = claims.ok_or_else(|| Error::Http("Not authenticated".into()))?;
        let role = if claims.has_role("admin") {
            "admin"
        } else {
            "user"
        };
        self.blog
            .delete_post(claims.subject(), role, &req.slug)
            .map_err(map_blog_err)?;
        Ok(format!("Deleted blog post {}", req.slug))
    }
}

fn map_blog_err(e: String) -> Error {
    if e == "Forbidden" {
        Error::Http("Forbidden".into())
    } else if e.contains("not found") {
        Error::NotFound(e)
    } else if e.contains("Slug") || e.contains("Invalid") {
        Error::Http(e)
    } else {
        Error::Internal(e)
    }
}
