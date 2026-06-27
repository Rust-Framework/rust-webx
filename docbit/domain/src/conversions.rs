//! Entity → Model conversions.
//!
//! These `From` impls convert domain entities into contracts-layer DTOs.
//! Navigation-derived fields (e.g. `category_name`, `author_name`, `roles`)
//! require the caller to have loaded the navigation via `linq!(include ...)`
//! before conversion; unloaded navigations yield empty defaults.
//!
//! 导航访问 API（rust-ef 1.1.0 源码实证）：
//! - `HasMany<T,J>`：`e.roles.items().iter()`（无 `.iter()` 直接方法）
//! - `BelongsTo<T>`：`e.category.get().map(...)`（无 `.as_ref()`）

use docbit_contracts::{
    blog::{BlogPostModel, BlogPostSummary},
    category::CategoryModel,
    comment::CommentModel,
    exhibition::ExhibitionModel,
    rbac::{AuthorizeModel, ResourceModel, RoleModel},
    tracking::TrackingModel,
    user::UserModel,
};

use crate::entities::*;

impl From<User> for UserModel {
    fn from(e: User) -> Self {
        Self {
            id: e.id,
            name: e.name,
            email: e.email,
            roles: e.roles.items().iter().map(|r| r.name.clone()).collect(),
            created_at: e.created_at,
        }
    }
}

impl From<Blog> for BlogPostModel {
    fn from(e: Blog) -> Self {
        Self {
            id: e.id,
            slug: e.slug,
            title: e.title,
            summary: e.summary,
            content: e.content,
            tags: serde_json::from_str(&e.tags).unwrap_or_default(),
            category_id: e.category_id,
            category_name: e
                .category
                .get()
                .map(|c| c.name.clone())
                .unwrap_or_default(),
            author_id: e.author_id,
            author_name: e
                .author
                .get()
                .map(|a| a.name.clone())
                .unwrap_or_default(),
            published_at: e.published_at,
            created_at: e.created_at,
            updated_at: e.updated_at,
        }
    }
}

impl From<Blog> for BlogPostSummary {
    fn from(e: Blog) -> Self {
        Self {
            id: e.id,
            slug: e.slug,
            title: e.title,
            summary: e.summary,
            tags: serde_json::from_str(&e.tags).unwrap_or_default(),
            category_id: e.category_id,
            category_name: e
                .category
                .get()
                .map(|c| c.name.clone())
                .unwrap_or_default(),
            author_id: e.author_id,
            author_name: e
                .author
                .get()
                .map(|a| a.name.clone())
                .unwrap_or_default(),
            published_at: e.published_at,
        }
    }
}

impl From<Comment> for CommentModel {
    fn from(e: Comment) -> Self {
        Self {
            id: e.id,
            blog_id: e.blog_id,
            user_id: e.user_id,
            user_name: e.user_name,
            content: e.content,
            parent_id: e.parent_id,
            quoted_id: e.quoted_id,
            created_at: e.created_at,
        }
    }
}

impl From<Category> for CategoryModel {
    fn from(e: Category) -> Self {
        Self {
            id: e.id,
            name: e.name,
            slug: e.slug,
            parent_id: e.parent_id,
            sort_order: e.sort_order,
            created_at: e.created_at,
        }
    }
}

impl From<Exhibition> for ExhibitionModel {
    fn from(e: Exhibition) -> Self {
        Self {
            id: e.id,
            slug: e.slug,
            title: e.title,
            subtitle: e.subtitle,
            description: e.description,
            category_id: e.category_id,
            category_name: e
                .category
                .get()
                .map(|c| c.name.clone())
                .unwrap_or_default(),
            tags: serde_json::from_str(&e.tags).unwrap_or_default(),
            repo_url: e.repo_url,
            demo_url: e.demo_url,
            docs_slug: e.docs_slug,
            featured: e.featured,
            sort_order: e.sort_order,
            logo_url: e.logo_url,
            created_at: e.created_at,
            updated_at: e.updated_at,
        }
    }
}

impl From<Role> for RoleModel {
    fn from(e: Role) -> Self {
        Self {
            id: e.id,
            name: e.name,
            description: e.description,
            created_at: e.created_at,
            updated_at: e.updated_at,
        }
    }
}

impl From<Resource> for ResourceModel {
    fn from(e: Resource) -> Self {
        Self {
            id: e.id,
            name: e.name,
            description: e.description,
            r#type: e.resource_type,
            value: e.value,
            properties: e.properties,
            created_at: e.created_at,
            updated_at: e.updated_at,
        }
    }
}

impl From<Authorize> for AuthorizeModel {
    fn from(e: Authorize) -> Self {
        Self {
            id: e.id,
            role_id: e.role_id,
            resource_id: e.resource_id,
        }
    }
}

impl From<Tracking> for TrackingModel {
    fn from(e: Tracking) -> Self {
        Self {
            id: e.id,
            path: e.path,
            method: e.method,
            ip: e.ip,
            user_agent: e.user_agent,
            referer: e.referer,
            status: e.status,
            duration_ms: e.duration_ms,
            visited_at: e.visited_at,
        }
    }
}
