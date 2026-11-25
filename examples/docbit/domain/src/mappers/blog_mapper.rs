//! Blog mappers — `Blog` ↔ `BlogPostModel` / `BlogPostSummary`.

use docbit_contracts::blog::{
    BlogPostModel, BlogPostSummary, CreateBlogPostRequest, UpdateBlogPostRequest,
};
use rust_ef::prelude::*;
use webx_core::{Error, IPatchable};

use crate::audit::operator_id;
use crate::entities::Blog;
use crate::ids::new_id;
use crate::mappers::now_secs;
use crate::seed_ids;

impl TryFrom<CreateBlogPostRequest> for Blog {
    type Error = Error;

    fn try_from(req: CreateBlogPostRequest) -> Result<Self, Self::Error> {
        let slug = req.slug.trim();
        if slug.is_empty() {
            return Err(Error::validation("Blog slug cannot be empty"));
        }
        let title = req.title.trim();
        if title.is_empty() {
            return Err(Error::validation("Blog title cannot be empty"));
        }

        let now = now_secs();
        let op = operator_id();
        Ok(Blog {
            id: new_id(),
            slug: slug.to_string(),
            title: title.to_string(),
            summary: req.summary,
            content: req.content,
            tags: serde_json::to_string(&req.tags).unwrap_or_default(),
            category_id: req
                .category_id
                .unwrap_or_else(|| seed_ids::CAT_UNCATEGORIZED.to_string()),
            author_id: op.clone().unwrap_or_default(),
            published_at: req.published_at,
            created_at: now,
            updated_at: now,
            created_id: op.clone(),
            updated_id: op,
            is_deleted: false,
            category: BelongsTo::new(),
            author: BelongsTo::new(),
            comments: HasMany::new(),
        })
    }
}

impl IPatchable<UpdateBlogPostRequest> for Blog {
    fn patch(&mut self, patch: UpdateBlogPostRequest) {
        if let Some(v) = patch.title {
            self.title = v;
        }
        if let Some(v) = patch.summary {
            self.summary = v;
        }
        if let Some(v) = patch.content {
            self.content = v;
        }
        if let Some(v) = patch.tags {
            self.tags = serde_json::to_string(&v).unwrap_or_default();
        }
        if let Some(v) = patch.category_id {
            self.category_id = v;
        }
        if let Some(v) = patch.published_at {
            self.published_at = v;
        }
        self.updated_id = operator_id();
        self.updated_at = now_secs();
    }
}

impl From<&Blog> for BlogPostModel {
    fn from(e: &Blog) -> Self {
        Self {
            id: e.id.clone(),
            slug: e.slug.clone(),
            title: e.title.clone(),
            summary: e.summary.clone(),
            content: e.content.clone(),
            tags: serde_json::from_str(&e.tags).unwrap_or_default(),
            category_id: e.category_id.clone(),
            category_name: e.category.get().map(|c| c.name.clone()).unwrap_or_default(),
            author_id: e.author_id.clone(),
            author_name: e.author.get().map(|a| a.name.clone()).unwrap_or_default(),
            published_at: e.published_at,
            created_at: e.created_at,
            updated_at: e.updated_at,
        }
    }
}

impl From<&Blog> for BlogPostSummary {
    fn from(e: &Blog) -> Self {
        Self {
            id: e.id.clone(),
            slug: e.slug.clone(),
            title: e.title.clone(),
            summary: e.summary.clone(),
            tags: serde_json::from_str(&e.tags).unwrap_or_default(),
            category_id: e.category_id.clone(),
            category_name: e.category.get().map(|c| c.name.clone()).unwrap_or_default(),
            author_id: e.author_id.clone(),
            author_name: e.author.get().map(|a| a.name.clone()).unwrap_or_default(),
            published_at: e.published_at,
        }
    }
}
