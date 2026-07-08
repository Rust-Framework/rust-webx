//! Blog handlers — 直接使用 DbContext 的 CRUD（中介者模式，无 service 抽象）。
//!
//! Tags 序列化为 JSON 字符串存库；列表查询 include `category`+`author`
//! 以填充 `category_name`/`author_name`。
//!
//! 每个 handler 持有 owned `DbContext`（通过 `#[derive(Inject)]` 的 bare T 字段
//! 自动以 `get_owned` 解析），实现 EFCore 风格的 per-request unit-of-work。
//! `handle(&mut self, ...)` 直接调用 `self.ctx.set::<T>()` / `save_changes()`，
//! 无需 `Arc<Mutex>` 内部可变性。

use std::collections::HashMap;

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

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
//
// `ctx: DbContext`（bare T）由 `#[derive(Inject)]` 通过 `resolver.get_owned`
// 解析为 owned 实例。`#[handler(inject)]` 生成 per-request factory + call bridge，
// 使 dispatch 通过 `HandlerCache` 调用 `handle(&mut self, ...)`。

#[derive(Inject)]
pub struct ListBlogPostsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct ListBlogCategoriesHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct ListMyBlogPostsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct GetBlogPostHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct CreateBlogPostHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct UpdateBlogPostHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct DeleteBlogPostHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListBlogPostsRequest, Vec<BlogPostSummary>> for ListBlogPostsHandler {
    async fn handle(&mut self, _: ListBlogPostsRequest) -> Result<Vec<BlogPostSummary>> {
        let blogs = linq!(self.ctx.set::<Blog>(), |b: Blog| !b.is_deleted; include b.category; include b.author; order_by b.published_at desc)
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(blogs.into_iter().map(BlogPostSummary::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListBlogCategoriesRequest, Vec<BlogCategoryCount>> for ListBlogCategoriesHandler {
    async fn handle(&mut self, _: ListBlogCategoriesRequest) -> Result<Vec<BlogCategoryCount>> {
        let blogs = linq!(self.ctx.set::<Blog>(), |b: Blog| !b.is_deleted)
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        let mut counts: HashMap<i32, usize> = HashMap::new();
        for b in &blogs {
            *counts.entry(b.category_id).or_insert(0) += 1;
        }
        let cats = linq!(self.ctx.set::<Category>(), |c: Category| !c.is_deleted)
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
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

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListMyBlogPostsRequest, Vec<BlogPostSummary>> for ListMyBlogPostsHandler {
    async fn handle(&mut self, req: ListMyBlogPostsRequest) -> Result<Vec<BlogPostSummary>> {
        let uid = uid_from_claims(req.claims.as_deref())?;
        let blogs = linq!(self.ctx.set::<Blog>(), |b: Blog| b.author_id == uid && !b.is_deleted; include b.category; include b.author; order_by b.published_at desc)
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(blogs.into_iter().map(BlogPostSummary::from).collect())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetBlogPostRequest, BlogPostModel> for GetBlogPostHandler {
    async fn handle(&mut self, req: GetBlogPostRequest) -> Result<BlogPostModel> {
        let slug = req.slug.clone();
        let blog = linq!(self.ctx.set::<Blog>(), |b: Blog| b.slug == req.slug && !b.is_deleted; include b.category; include b.author)
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound(format!("Blog post not found: {}", slug)))?;
        Ok(blog.to_model())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateBlogPostRequest, BlogPostModel> for CreateBlogPostHandler {
    async fn handle(&mut self, req: CreateBlogPostRequest) -> Result<BlogPostModel> {
        let uid = uid_from_claims(req.claims.as_deref())?;
        // slug 唯一性校验
        let slug = req.slug.clone();
        let q = slug.clone();
        let exists = linq!(self.ctx.set::<Blog>(), |b: Blog| b.slug == q && !b.is_deleted)
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        if exists.is_some() {
            return Err(Error::Http(format!("Slug already exists: {}", slug)));
        }

        let now = now_secs();
        let blog = req.to_entity(uid, now);
        self.ctx.set::<Blog>().add(blog);
        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(format!("Failed to create blog: {}", e)))?;
        // 回查以装载导航字段
        let q = slug.clone();
        let saved = linq!(self.ctx.set::<Blog>(), |b: Blog| b.slug == q && !b.is_deleted; include b.category; include b.author)
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::Internal("Blog vanished after insert".into()))?;
        tracing::info!("[Blog] Created: {} by {}", saved.slug, uid);
        Ok(saved.to_model())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateBlogPostRequest, BlogPostModel> for UpdateBlogPostHandler {
    async fn handle(&mut self, req: UpdateBlogPostRequest) -> Result<BlogPostModel> {
        let uid = uid_from_claims(req.claims.as_deref())?;
        let roles = roles_from_claims(req.claims.as_deref());
        let slug = req.slug.clone();
        let q = slug.clone();
        let mut blog = linq!(self.ctx.set::<Blog>(), |b: Blog| b.slug == q && !b.is_deleted; include b.category; include b.author)
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound(format!("Blog post not found: {}", slug)))?;

        if !is_admin(&roles) && blog.author_id != uid {
            return Err(Error::Http("Forbidden: not the author".into()));
        }

        req.apply_to(&mut blog, uid, now_secs());
        self.ctx.set::<Blog>().update(blog);
        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        let q = slug.clone();
        let saved = linq!(self.ctx.set::<Blog>(), |b: Blog| b.slug == q && !b.is_deleted; include b.category; include b.author)
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("Blog not found after update".into()))?;
        Ok(saved.to_model())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteBlogPostRequest, String> for DeleteBlogPostHandler {
    async fn handle(&mut self, req: DeleteBlogPostRequest) -> Result<String> {
        let uid = uid_from_claims(req.claims.as_deref())?;
        let roles = roles_from_claims(req.claims.as_deref());
        let slug = req.slug.clone();
        let q = slug.clone();
        let mut blog = linq!(self.ctx.set::<Blog>(), |b: Blog| b.slug == q && !b.is_deleted)
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound(format!("Blog post not found: {}", slug)))?;

        if !is_admin(&roles) && blog.author_id != uid {
            return Err(Error::Http("Forbidden: not the author".into()));
        }

        blog.is_deleted = true;
        blog.updated_id = Some(uid);
        blog.updated_at = now_secs();
        self.ctx.set::<Blog>().update(blog);
        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        tracing::info!("[Blog] Soft-deleted: {}", slug);
        Ok(format!("Deleted blog post {}", slug))
    }
}
