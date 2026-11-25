//! Exhibition mappers — `Exhibition` ↔ `ExhibitionModel`.

use docbit_contracts::exhibition::{ExhibitionModel, UpsertExhibitionRequest};
use rust_ef::prelude::*;
use webx_core::{Error, IPatchable};

use crate::audit::operator_id;
use crate::entities::Exhibition;
use crate::ids::new_id;
use crate::mappers::now_secs;

impl TryFrom<UpsertExhibitionRequest> for Exhibition {
    type Error = Error;

    fn try_from(req: UpsertExhibitionRequest) -> Result<Self, Self::Error> {
        let slug = req.slug.trim();
        if slug.is_empty() {
            return Err(Error::validation("Exhibition slug cannot be empty"));
        }
        let title = req.title.trim();
        if title.is_empty() {
            return Err(Error::validation("Exhibition title cannot be empty"));
        }

        let now = now_secs();
        let op = operator_id();
        Ok(Exhibition {
            id: new_id(),
            slug: slug.to_string(),
            title: title.to_string(),
            subtitle: req.subtitle,
            description: req.description,
            category_id: req.category_id,
            tags: serde_json::to_string(&req.tags).unwrap_or_default(),
            repo_url: req.repo_url,
            demo_url: req.demo_url,
            docs_slug: req.docs_slug,
            featured: req.featured,
            sort_order: req.sort_order,
            logo_url: req.logo_url,
            created_at: now,
            updated_at: now,
            created_id: op.clone(),
            updated_id: op,
            is_deleted: false,
            category: BelongsTo::new(),
        })
    }
}

impl IPatchable<UpsertExhibitionRequest> for Exhibition {
    fn patch(&mut self, patch: UpsertExhibitionRequest) {
        self.title = patch.title;
        self.subtitle = patch.subtitle;
        self.description = patch.description;
        self.category_id = patch.category_id;
        self.tags = serde_json::to_string(&patch.tags).unwrap_or_default();
        self.repo_url = patch.repo_url;
        self.demo_url = patch.demo_url;
        self.docs_slug = patch.docs_slug;
        self.featured = patch.featured;
        self.sort_order = patch.sort_order;
        self.logo_url = patch.logo_url;
        self.updated_id = operator_id();
        self.updated_at = now_secs();
    }
}

impl From<&Exhibition> for ExhibitionModel {
    fn from(e: &Exhibition) -> Self {
        Self {
            id: e.id.clone(),
            slug: e.slug.clone(),
            title: e.title.clone(),
            subtitle: e.subtitle.clone(),
            description: e.description.clone(),
            category_id: e.category_id.clone(),
            category_name: e.category.get().map(|c| c.name.clone()).unwrap_or_default(),
            category: e.category.get().map(|c| c.slug.clone()).unwrap_or_default(),
            tags: serde_json::from_str(&e.tags).unwrap_or_default(),
            repo_url: e.repo_url.clone(),
            demo_url: e.demo_url.clone(),
            docs_slug: e.docs_slug.clone(),
            featured: e.featured,
            sort_order: e.sort_order,
            logo_url: e.logo_url.clone(),
            created_at: e.created_at,
            updated_at: e.updated_at,
        }
    }
}
