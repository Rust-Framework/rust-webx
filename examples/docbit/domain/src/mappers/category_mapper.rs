//! Category mappers — `Category` ↔ `CategoryModel`.

use docbit_contracts::category::{CategoryModel, CreateCategoryRequest, UpdateCategoryRequest};
use rust_ef::prelude::*;
use webx_core::{Error, IPatchable};

use crate::audit::operator_id;
use crate::entities::Category;
use crate::ids::new_id;
use crate::mappers::now_secs;

impl TryFrom<CreateCategoryRequest> for Category {
    type Error = Error;

    fn try_from(req: CreateCategoryRequest) -> Result<Self, Self::Error> {
        let name = req.name.trim();
        if name.is_empty() {
            return Err(Error::validation("Category name cannot be empty"));
        }
        let slug = req.slug.trim();
        if slug.is_empty() {
            return Err(Error::validation("Category slug cannot be empty"));
        }

        let now = now_secs();
        let op = operator_id();
        Ok(Category {
            id: new_id(),
            name: name.to_string(),
            slug: slug.to_string(),
            parent_id: req.parent_id,
            sort_order: req.sort_order,
            created_id: op.clone(),
            created_at: now,
            updated_id: op,
            updated_at: now,
            is_deleted: false,
            parent: BelongsTo::new(),
            children: HasMany::new(),
        })
    }
}

impl IPatchable<UpdateCategoryRequest> for Category {
    fn patch(&mut self, patch: UpdateCategoryRequest) {
        if let Some(v) = patch.name {
            self.name = v;
        }
        if let Some(v) = patch.sort_order {
            self.sort_order = v;
        }
        self.updated_id = operator_id();
        self.updated_at = now_secs();
    }
}

impl From<&Category> for CategoryModel {
    fn from(e: &Category) -> Self {
        Self {
            id: e.id.clone(),
            name: e.name.clone(),
            slug: e.slug.clone(),
            parent_id: e.parent_id.clone(),
            sort_order: e.sort_order,
            created_at: e.created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn category() -> Category {
        Category {
            id: "c1".into(),
            name: "ORM".into(),
            slug: "orm".into(),
            parent_id: None,
            sort_order: 1,
            created_id: None,
            created_at: 0,
            updated_id: None,
            updated_at: 0,
            is_deleted: false,
            parent: BelongsTo::new(),
            children: HasMany::new(),
        }
    }

    #[test]
    fn create_request_is_validated_and_server_fields_are_assigned() {
        let entity = Category::try_from(CreateCategoryRequest {
            name: "  Databases  ".into(),
            slug: " databases ".into(),
            parent_id: None,
            sort_order: 2,
        })
        .expect("valid category");
        assert_eq!(entity.name, "Databases");
        assert_eq!(entity.slug, "databases");
        assert!(!entity.id.is_empty());
        assert_eq!(entity.created_at, entity.updated_at);

        let invalid = CreateCategoryRequest {
            name: "   ".into(),
            slug: "databases".into(),
            ..Default::default()
        };
        assert!(Category::try_from(invalid).is_err());
    }

    #[test]
    fn patch_only_overwrites_the_fields_it_carries() {
        let mut entity = category();
        entity.patch(UpdateCategoryRequest {
            id: "c1".into(),
            name: Some("Data".into()),
            sort_order: None,
        });
        assert_eq!(entity.name, "Data");
        assert_eq!(entity.sort_order, 1, "absent sort_order is untouched");
    }
}
