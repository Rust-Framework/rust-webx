//! Mapper extensions — `to_model()` / `to_entity()` / `apply_to()` 一行式转换。
//!
//! 大幅降低 handler 中的 Entity ↔ Model/Request 转换样板代码：
//! - `ToModel<M>`：Entity → Model（复用 `conversions.rs` 中的 `From` impl）
//! - `ToEntity<T>`：CreateRequest → Entity（自动初始化导航字段、审计字段、默认值）
//! - `ApplyTo<T>`：UpdateRequest → &mut Entity（部分字段更新 + 审计字段）
//!
//! 调用约定：`op` = 操作人 ID（来自 claims），`now` = 当前 Unix 时间戳。
//!
//! ## 用法示例
//!
//! ```ignore
//! // Entity → Model
//! let model: BlogPostModel = blog.to_model();
//!
//! // Request → Entity (create)
//! let blog = req.to_entity(uid, now);
//!
//! // Request → &mut Entity (update)
//! req.apply_to(&mut blog, uid, now);
//! ```

use rust_ef::prelude::*;

use docbit_contracts::{
    blog::{CreateBlogPostRequest, UpdateBlogPostRequest},
    category::{CreateCategoryRequest, UpdateCategoryRequest},
    comment::CreateCommentRequest,
    exhibition::UpsertExhibitionRequest,
    rbac::{
        CreateAuthorizeRequest, CreateResourceRequest, CreateRoleRequest,
        UpdateResourceRequest, UpdateRoleRequest,
    },
    user::{CreateUserRequest, UpdateUserRequest},
};

use crate::entities::*;

// ────────────────── ToModel: Entity → Model ──────────────────

/// Extension trait: convert entity to model via existing `From` impl.
///
/// 复用 `conversions.rs` 中的 `From<Entity> for Model` 实现。
/// 当同一 Entity 有多个目标 Model 时（如 `Blog` → `BlogPostModel` / `BlogPostSummary`），
/// 需用 turbofish 或类型标注指定：`blog.to_model::<BlogPostSummary>()`。
pub trait ToModel<M> {
    fn to_model(self) -> M;
}

impl<E, M> ToModel<M> for E
where
    M: From<E>,
{
    #[inline]
    fn to_model(self) -> M {
        self.into()
    }
}

// ────────────────── ToEntity: Request → Entity (create) ──────────────────

/// Extension trait: convert create-request to entity with audit context.
///
/// 自动设置 `id: 0`、`is_deleted: false`、导航字段为空、审计字段为 `(op, now)`。
///
/// `op` = 操作人 ID（来自 claims），`now` = 当前 Unix 时间戳。
pub trait ToEntity<T> {
    fn to_entity(self, op: i32, now: i64) -> T;
}

impl ToEntity<Blog> for CreateBlogPostRequest {
    fn to_entity(self, op: i32, now: i64) -> Blog {
        Blog {
            id: 0,
            slug: self.slug,
            title: self.title,
            summary: self.summary,
            content: self.content,
            tags: serde_json::to_string(&self.tags).unwrap_or_default(),
            category_id: self.category_id.unwrap_or(1),
            author_id: op,
            published_at: self.published_at,
            created_at: now,
            updated_at: now,
            created_id: Some(op),
            updated_id: Some(op),
            is_deleted: false,
            category: BelongsTo::new(),
            author: BelongsTo::new(),
            comments: HasMany::new(),
        }
    }
}

impl ToEntity<Category> for CreateCategoryRequest {
    fn to_entity(self, op: i32, now: i64) -> Category {
        Category {
            id: 0,
            name: self.name,
            slug: self.slug,
            parent_id: self.parent_id,
            sort_order: self.sort_order,
            created_id: Some(op),
            created_at: now,
            updated_id: Some(op),
            updated_at: now,
            is_deleted: false,
            parent: BelongsTo::new(),
            children: HasMany::new(),
        }
    }
}

impl ToEntity<Comment> for CreateCommentRequest {
    fn to_entity(self, op: i32, now: i64) -> Comment {
        Comment {
            id: 0,
            blog_id: self.blog_id,
            user_id: op,
            user_name: String::new(), // 调用方设置（来自 claims 用户名）
            content: self.content,
            parent_id: self.parent_id,
            quoted_id: self.quoted_id,
            created_at: now,
            updated_id: Some(op),
            updated_at: now,
            is_deleted: false,
            blog: BelongsTo::new(),
            user: BelongsTo::new(),
            parent: BelongsTo::new(),
            quoted: BelongsTo::new(),
        }
    }
}

impl ToEntity<User> for CreateUserRequest {
    fn to_entity(self, op: i32, now: i64) -> User {
        User {
            id: 0,
            name: self.name,
            email: self.email,
            password_hash: String::new(),
            created_id: Some(op),
            created_at: now,
            updated_id: Some(op),
            updated_at: now,
            is_deleted: false,
            roles: HasMany::new(),
        }
    }
}

impl ToEntity<Exhibition> for UpsertExhibitionRequest {
    fn to_entity(self, op: i32, now: i64) -> Exhibition {
        Exhibition {
            id: 0,
            slug: self.slug,
            title: self.title,
            subtitle: self.subtitle,
            description: self.description,
            category_id: self.category_id,
            tags: serde_json::to_string(&self.tags).unwrap_or_default(),
            repo_url: self.repo_url,
            demo_url: self.demo_url,
            docs_slug: self.docs_slug,
            featured: self.featured,
            sort_order: self.sort_order,
            logo_url: self.logo_url,
            created_at: now,
            updated_at: now,
            created_id: Some(op),
            updated_id: Some(op),
            is_deleted: false,
            category: BelongsTo::new(),
        }
    }
}

impl ToEntity<Role> for CreateRoleRequest {
    fn to_entity(self, op: i32, now: i64) -> Role {
        Role {
            id: 0,
            name: self.name,
            description: self.description,
            created_id: Some(op),
            created_at: now,
            updated_id: Some(op),
            updated_at: now,
            is_deleted: false,
            users: HasMany::new(),
            resources: HasMany::new(),
        }
    }
}

impl ToEntity<Resource> for CreateResourceRequest {
    fn to_entity(self, op: i32, now: i64) -> Resource {
        Resource {
            id: 0,
            name: self.name,
            description: self.description,
            resource_type: self.r#type,
            value: self.value,
            properties: self.properties,
            created_id: Some(op),
            created_at: now,
            updated_id: Some(op),
            updated_at: now,
            is_deleted: false,
            roles: HasMany::new(),
        }
    }
}

impl ToEntity<Authorize> for CreateAuthorizeRequest {
    fn to_entity(self, _op: i32, now: i64) -> Authorize {
        Authorize {
            id: 0,
            role_id: self.role_id,
            resource_id: self.resource_id,
            created_at: now,
        }
    }
}

// ────────────────── ApplyTo: UpdateRequest → &mut Entity ──────────────────

/// Extension trait: apply update-request fields to an existing entity.
///
/// 仅更新请求中提供的字段（`Option::Some`），并设置审计字段 `updated_id`/`updated_at`。
///
/// `op` = 操作人 ID，`now` = 当前 Unix 时间戳。
pub trait ApplyTo<T> {
    fn apply_to(self, entity: &mut T, op: i32, now: i64);
}

impl ApplyTo<Blog> for UpdateBlogPostRequest {
    fn apply_to(self, entity: &mut Blog, op: i32, now: i64) {
        if let Some(v) = self.title {
            entity.title = v;
        }
        if let Some(v) = self.summary {
            entity.summary = v;
        }
        if let Some(v) = self.content {
            entity.content = v;
        }
        if let Some(v) = self.tags {
            entity.tags = serde_json::to_string(&v).unwrap_or_default();
        }
        if let Some(v) = self.category_id {
            entity.category_id = v;
        }
        if let Some(v) = self.published_at {
            entity.published_at = v;
        }
        entity.updated_id = Some(op);
        entity.updated_at = now;
    }
}

impl ApplyTo<Category> for UpdateCategoryRequest {
    fn apply_to(self, entity: &mut Category, op: i32, now: i64) {
        if let Some(v) = self.name {
            entity.name = v;
        }
        if let Some(v) = self.sort_order {
            entity.sort_order = v;
        }
        entity.updated_id = Some(op);
        entity.updated_at = now;
    }
}

impl ApplyTo<User> for UpdateUserRequest {
    fn apply_to(self, entity: &mut User, op: i32, now: i64) {
        if let Some(v) = self.name {
            entity.name = v;
        }
        if let Some(v) = self.email {
            entity.email = v;
        }
        entity.updated_id = Some(op);
        entity.updated_at = now;
    }
}

impl ApplyTo<Role> for UpdateRoleRequest {
    fn apply_to(self, entity: &mut Role, op: i32, now: i64) {
        if let Some(v) = self.name {
            entity.name = v;
        }
        if let Some(v) = self.description {
            entity.description = v;
        }
        entity.updated_id = Some(op);
        entity.updated_at = now;
    }
}

impl ApplyTo<Resource> for UpdateResourceRequest {
    fn apply_to(self, entity: &mut Resource, op: i32, now: i64) {
        if let Some(v) = self.name {
            entity.name = v;
        }
        if let Some(v) = self.description {
            entity.description = v;
        }
        if let Some(v) = self.r#type {
            entity.resource_type = v;
        }
        if let Some(v) = self.value {
            entity.value = v;
        }
        if let Some(v) = self.properties {
            entity.properties = v;
        }
        entity.updated_id = Some(op);
        entity.updated_at = now;
    }
}

impl ApplyTo<Exhibition> for UpsertExhibitionRequest {
    fn apply_to(self, entity: &mut Exhibition, op: i32, now: i64) {
        entity.title = self.title;
        entity.subtitle = self.subtitle;
        entity.description = self.description;
        entity.category_id = self.category_id;
        entity.tags = serde_json::to_string(&self.tags).unwrap_or_default();
        entity.repo_url = self.repo_url;
        entity.demo_url = self.demo_url;
        entity.docs_slug = self.docs_slug;
        entity.featured = self.featured;
        entity.sort_order = self.sort_order;
        entity.logo_url = self.logo_url;
        entity.updated_id = Some(op);
        entity.updated_at = now;
    }
}
