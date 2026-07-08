//! Mapper extensions — `to_model()` / `to_entity()` / `apply_to()`.

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

use crate::audit::operator_id;
use crate::entities::*;
use crate::seed_ids;

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

/// Convert create-request to entity. `id` is assigned by the caller before insert.
pub trait ToEntity<T> {
    fn to_entity(self, id: String, now: i64) -> T;
}

impl ToEntity<Blog> for CreateBlogPostRequest {
    fn to_entity(self, id: String, now: i64) -> Blog {
        let op = operator_id();
        Blog {
            id,
            slug: self.slug,
            title: self.title,
            summary: self.summary,
            content: self.content,
            tags: serde_json::to_string(&self.tags).unwrap_or_default(),
            category_id: self
                .category_id
                .unwrap_or_else(|| seed_ids::CAT_UNCATEGORIZED.to_string()),
            author_id: op.clone().unwrap_or_default(),
            published_at: self.published_at,
            created_at: now,
            updated_at: now,
            created_id: op.clone(),
            updated_id: op,
            is_deleted: false,
            category: BelongsTo::new(),
            author: BelongsTo::new(),
            comments: HasMany::new(),
        }
    }
}

impl ToEntity<Category> for CreateCategoryRequest {
    fn to_entity(self, id: String, now: i64) -> Category {
        let op = operator_id();
        Category {
            id,
            name: self.name,
            slug: self.slug,
            parent_id: self.parent_id,
            sort_order: self.sort_order,
            created_id: op.clone(),
            created_at: now,
            updated_id: op,
            updated_at: now,
            is_deleted: false,
            parent: BelongsTo::new(),
            children: HasMany::new(),
        }
    }
}

impl ToEntity<Comment> for CreateCommentRequest {
    fn to_entity(self, id: String, now: i64) -> Comment {
        let op = operator_id();
        Comment {
            id,
            blog_id: self.blog_id,
            user_id: op.clone().unwrap_or_default(),
            user_name: String::new(),
            content: self.content,
            parent_id: self.parent_id,
            quoted_id: self.quoted_id,
            created_at: now,
            updated_id: op,
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
    fn to_entity(self, id: String, now: i64) -> User {
        let op = operator_id();
        User {
            id,
            name: self.name,
            email: self.email,
            password_hash: String::new(),
            created_id: op.clone(),
            created_at: now,
            updated_id: op,
            updated_at: now,
            is_deleted: false,
            roles: HasMany::new(),
        }
    }
}

impl ToEntity<Exhibition> for UpsertExhibitionRequest {
    fn to_entity(self, id: String, now: i64) -> Exhibition {
        let op = operator_id();
        Exhibition {
            id,
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
            created_id: op.clone(),
            updated_id: op,
            is_deleted: false,
            category: BelongsTo::new(),
        }
    }
}

impl ToEntity<Role> for CreateRoleRequest {
    fn to_entity(self, id: String, now: i64) -> Role {
        let op = operator_id();
        Role {
            id,
            name: self.name,
            description: self.description,
            created_id: op.clone(),
            created_at: now,
            updated_id: op,
            updated_at: now,
            is_deleted: false,
            users: HasMany::new(),
            resources: HasMany::new(),
        }
    }
}

impl ToEntity<Resource> for CreateResourceRequest {
    fn to_entity(self, id: String, now: i64) -> Resource {
        let op = operator_id();
        Resource {
            id,
            name: self.name,
            description: self.description,
            resource_type: self.r#type,
            value: self.value,
            properties: self.properties,
            created_id: op.clone(),
            created_at: now,
            updated_id: op,
            updated_at: now,
            is_deleted: false,
            roles: HasMany::new(),
        }
    }
}

impl ToEntity<Authorize> for CreateAuthorizeRequest {
    fn to_entity(self, id: String, now: i64) -> Authorize {
        Authorize {
            id,
            role_id: self.role_id,
            resource_id: self.resource_id,
            created_at: now,
        }
    }
}

pub trait ApplyTo<T> {
    fn apply_to(self, entity: &mut T, now: i64);
}

impl ApplyTo<Blog> for UpdateBlogPostRequest {
    fn apply_to(self, entity: &mut Blog, now: i64) {
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
        entity.updated_id = operator_id();
        entity.updated_at = now;
    }
}

impl ApplyTo<Category> for UpdateCategoryRequest {
    fn apply_to(self, entity: &mut Category, now: i64) {
        if let Some(v) = self.name {
            entity.name = v;
        }
        if let Some(v) = self.sort_order {
            entity.sort_order = v;
        }
        entity.updated_id = operator_id();
        entity.updated_at = now;
    }
}

impl ApplyTo<User> for UpdateUserRequest {
    fn apply_to(self, entity: &mut User, now: i64) {
        if let Some(v) = self.name {
            entity.name = v;
        }
        if let Some(v) = self.email {
            entity.email = v;
        }
        entity.updated_id = operator_id();
        entity.updated_at = now;
    }
}

impl ApplyTo<Role> for UpdateRoleRequest {
    fn apply_to(self, entity: &mut Role, now: i64) {
        if let Some(v) = self.name {
            entity.name = v;
        }
        if let Some(v) = self.description {
            entity.description = v;
        }
        entity.updated_id = operator_id();
        entity.updated_at = now;
    }
}

impl ApplyTo<Resource> for UpdateResourceRequest {
    fn apply_to(self, entity: &mut Resource, now: i64) {
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
        entity.updated_id = operator_id();
        entity.updated_at = now;
    }
}

impl ApplyTo<Exhibition> for UpsertExhibitionRequest {
    fn apply_to(self, entity: &mut Exhibition, now: i64) {
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
        entity.updated_id = operator_id();
        entity.updated_at = now;
    }
}
