//! Blog handlers — 直接使用 DbContext 的 CRUD（中介者模式，无 service 抽象）。
//!
//! Tags 序列化为 JSON 字符串存库；列表查询 include `category`+`author`
//! 以填充 `category_name`/`author_name`。

use std::collections::HashMap;
use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webapp::*;
use tokio::sync::Mutex;

use docbit_contracts::blog::*;
use docbit_domain::entities::{Blog, Category};
use docbit_domain::{ApplyTo, ToEntity, ToModel};

use crate::util::now_secs;

fn uid_from_claims(claims: Option<&dyn IClaims>) -> Result<i32> {
    let c = claims.ok_or_else(|| Error::Http("Not authenticated".into()))?;
    c.subject()
        .parse::<i32>()
        .map_err(|_| Error::Http("Invalid user id in token".into()))
}

fn roles_from_claims(claims: Option<&dyn IClaims>) -> Vec<String> {
    claims.map(|c| c.roles().to_vec()).unwrap_or_default()
}

fn is_admin(roles: &[String]) -> bool {
    roles.iter().any(|r| r == "admin")
}

// ── Handlers ──

#[derive(Inject)]
pub struct ListBlogPostsHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct ListBlogCategoriesHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct ListMyBlogPostsHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct GetBlogPostHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct CreateBlogPostHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct UpdateBlogPostHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[derive(Inject)]
pub struct DeleteBlogPostHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[inject(scoped)]
#[async_trait]
impl IRequestHandler<ListBlogPostsRequest, Vec<BlogPostSummary>> for ListBlogPostsHandler {
    async fn handle(&self, _: ListBlogPostsRequest) -> Result<Vec<BlogPostSummary>> {
        let blogs = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Blog>(), |b: Blog| !b.is_deleted; include b.category; include b.author; order_by b.published_at desc)
                .to_list()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        Ok(blogs.into_iter().map(BlogPostSummary::from).collect())
    }
}

#[inject(scoped)]
#[async_trait]
impl IRequestHandler<ListBlogCategoriesRequest, Vec<BlogCategoryCount>> for ListBlogCategoriesHandler {
    async fn handle(&self, _: ListBlogCategoriesRequest) -> Result<Vec<BlogCategoryCount>> {
        let blogs = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Blog>(), |b: Blog| !b.is_deleted)
                .to_list()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        let mut counts: HashMap<i32, usize> = HashMap::new();
        for b in &blogs {
            *counts.entry(b.category_id).or_insert(0) += 1;
        }
        let cats = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Category>(), |c: Category| !c.is_deleted)
                .to_list()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        let mut result: Vec<BlogCategoryCount> = cats
            .into_iter()
            .map(|c| BlogCategoryCount {
                id: c.id,
                name: c.name,
                slug: c.slug,
                count: counts.get(&c.id).copied().unwrap_or(0),
            })
            .collect();
        result.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(result)
    }
}

#[inject(scoped)]
#[async_trait]
impl IRequestHandler<ListMyBlogPostsRequest, Vec<BlogPostSummary>> for ListMyBlogPostsHandler {
    async fn handle(&self, req: ListMyBlogPostsRequest) -> Result<Vec<BlogPostSummary>> {
        let uid = uid_from_claims(req.claims.as_deref())?;
        let blogs = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Blog>(), |b: Blog| b.author_id == uid && !b.is_deleted; include b.category; include b.author; order_by b.published_at desc)
                .to_list()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        Ok(blogs.into_iter().map(BlogPostSummary::from).collect())
    }
}

#[inject(scoped)]
#[async_trait]
impl IRequestHandler<GetBlogPostRequest, BlogPostModel> for GetBlogPostHandler {
    async fn handle(&self, req: GetBlogPostRequest) -> Result<BlogPostModel> {
        let slug = req.slug.clone();
        let blog = {
            let mut ctx = self.ctx.lock().await;
            linq!(ctx.set::<Blog>(), |b: Blog| b.slug == req.slug && !b.is_deleted; include b.category; include b.author)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound(format!("Blog post not found: {}", slug)))?;
        Ok(blog.to_model())
    }
}

#[inject(scoped)]
#[async_trait]
impl IRequestHandler<CreateBlogPostRequest, BlogPostModel> for CreateBlogPostHandler {
    async fn handle(&self, req: CreateBlogPostRequest) -> Result<BlogPostModel> {
        let uid = uid_from_claims(req.claims.as_deref())?;
        // slug 唯一性校验
        let slug = req.slug.clone();
        let exists = {
            let mut ctx = self.ctx.lock().await;
            let q = slug.clone();
            linq!(ctx.set::<Blog>(), |b: Blog| b.slug == q && !b.is_deleted)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        if exists.is_some() {
            return Err(Error::Http(format!("Slug already exists: {}", slug)));
        }

        let now = now_secs();
        let blog = req.to_entity(uid, now);
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Blog>().add(blog);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(format!("Failed to create blog: {}", e)))?;
        }
        // 回查以装载导航字段
        let saved = {
            let mut ctx = self.ctx.lock().await;
            let q = slug.clone();
            linq!(ctx.set::<Blog>(), |b: Blog| b.slug == q && !b.is_deleted; include b.category; include b.author)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::Internal("Blog vanished after insert".into()))?;
        tracing::info!("[Blog] Created: {} by {}", saved.slug, uid);
        Ok(saved.to_model())
    }
}

#[inject(scoped)]
#[async_trait]
impl IRequestHandler<UpdateBlogPostRequest, BlogPostModel> for UpdateBlogPostHandler {
    async fn handle(&self, req: UpdateBlogPostRequest) -> Result<BlogPostModel> {
        let uid = uid_from_claims(req.claims.as_deref())?;
        let roles = roles_from_claims(req.claims.as_deref());
        let slug = req.slug.clone();
        let mut blog = {
            let mut ctx = self.ctx.lock().await;
            let q = slug.clone();
            linq!(ctx.set::<Blog>(), |b: Blog| b.slug == q && !b.is_deleted; include b.category; include b.author)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound(format!("Blog post not found: {}", slug)))?;

        if !is_admin(&roles) && blog.author_id != uid {
            return Err(Error::Http("Forbidden: not the author".into()));
        }

        req.apply_to(&mut blog, uid, now_secs());
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Blog>().update(blog);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
        }
        let saved = {
            let mut ctx = self.ctx.lock().await;
            let q = slug.clone();
            linq!(ctx.set::<Blog>(), |b: Blog| b.slug == q && !b.is_deleted; include b.category; include b.author)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("Blog not found after update".into()))?;
        Ok(saved.to_model())
    }
}

#[inject(scoped)]
#[async_trait]
impl IRequestHandler<DeleteBlogPostRequest, String> for DeleteBlogPostHandler {
    async fn handle(&self, req: DeleteBlogPostRequest) -> Result<String> {
        let uid = uid_from_claims(req.claims.as_deref())?;
        let roles = roles_from_claims(req.claims.as_deref());
        let slug = req.slug.clone();
        let mut blog = {
            let mut ctx = self.ctx.lock().await;
            let q = slug.clone();
            linq!(ctx.set::<Blog>(), |b: Blog| b.slug == q && !b.is_deleted)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound(format!("Blog post not found: {}", slug)))?;

        if !is_admin(&roles) && blog.author_id != uid {
            return Err(Error::Http("Forbidden: not the author".into()));
        }

        blog.is_deleted = true;
        blog.updated_id = Some(uid);
        blog.updated_at = now_secs();
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Blog>().update(blog);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
        }
        tracing::info!("[Blog] Soft-deleted: {}", slug);
        Ok(format!("Deleted blog post {}", slug))
    }
}
