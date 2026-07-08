//! Category handlers — CRUD plus hierarchical tree assembly.
//!
//! 每个 handler 持有 owned `DbContext`，`handle(&mut self, ...)` 直接操作 `self.ctx`。

use std::collections::HashMap;

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use docbit_contracts::category::{
    CategoryModel, CategoryTreeNode, CreateCategoryRequest, DeleteCategoryRequest,
    ListCategoriesRequest, UpdateCategoryRequest,
};
use docbit_domain::entities::Category;
use docbit_domain::{new_id, ApplyTo, ToEntity, ToModel};

use crate::util::{now_secs, operator_id, parse_id};

#[derive(Inject)]
pub struct ListCategoriesHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct CreateCategoryHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct UpdateCategoryHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[derive(Inject)]
pub struct DeleteCategoryHandler {
    #[inject(owned)]
    ctx: DbContext,
}

/// 把扁平分类列表组装为森林，按 `sort_order` 升序、`id` 升序排序。
fn build_tree(mut cats: Vec<CategoryModel>) -> Vec<CategoryTreeNode> {
    cats.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.id.cmp(&b.id)));

    let mut nodes: HashMap<String, CategoryTreeNode> = cats
        .into_iter()
        .map(|c| {
            (
                c.id.clone(),
                CategoryTreeNode {
                    category: c,
                    children: Vec::new(),
                    level: 0,
                },
            )
        })
        .collect();

    let roots: Vec<String> = nodes
        .values()
        .filter(|n| n.category.parent_id.is_none())
        .map(|n| n.category.id.clone())
        .collect();

    let mut result = Vec::new();
    for root_id in roots {
        if let Some(node) = nodes.remove(&root_id) {
            result.push(attach_children(node, &mut nodes, 0));
        }
    }
    result
}

fn attach_children(
    mut node: CategoryTreeNode,
    pool: &mut HashMap<String, CategoryTreeNode>,
    level: u32,
) -> CategoryTreeNode {
    node.level = level;

    let child_ids: Vec<String> = pool
        .values()
        .filter(|n| n.category.parent_id.as_deref() == Some(node.category.id.as_str()))
        .map(|n| n.category.id.clone())
        .collect();

    for cid in child_ids {
        if let Some(child) = pool.remove(&cid) {
            node.children.push(attach_children(child, pool, level + 1));
        }
    }
    node
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListCategoriesRequest, Vec<CategoryTreeNode>> for ListCategoriesHandler {
    async fn handle(&mut self, _: ListCategoriesRequest) -> Result<Vec<CategoryTreeNode>> {
        let cats = linq!(self.ctx.set::<Category>(), |c: Category| !c.is_deleted)
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        let models: Vec<CategoryModel> = cats.into_iter().map(CategoryModel::from).collect();
        Ok(build_tree(models))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateCategoryRequest, CategoryModel> for CreateCategoryHandler {
    async fn handle(&mut self, req: CreateCategoryRequest) -> Result<CategoryModel> {
        let op = operator_id(req.claims.as_deref());
        let now = now_secs();
        let id = new_id();

        let entity = req.to_entity(id, op, now);

        let set = self.ctx.set::<Category>();
        set.add(entity.clone());

        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(format!("Failed to create category: {}", e)))?;

        tracing::info!("[Category] Created: {} ({})", entity.name, entity.id);
        Ok(entity.to_model())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateCategoryRequest, CategoryModel> for UpdateCategoryHandler {
    async fn handle(&mut self, req: UpdateCategoryRequest) -> Result<CategoryModel> {
        let id = parse_id(&req.id)?;

        let mut cat = self
            .ctx
            .set::<Category>()
            .query()
            .find(id.clone())
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("Category not found".into()))?;

        let op = operator_id(req.claims.as_deref());
        req.apply_to(&mut cat, op, now_secs());

        let set = self.ctx.set::<Category>();
        set.update(cat.clone());

        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        Ok(cat.to_model())
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteCategoryRequest, String> for DeleteCategoryHandler {
    async fn handle(&mut self, req: DeleteCategoryRequest) -> Result<String> {
        let id = parse_id(&req.id)?;

        let mut cat = self
            .ctx
            .set::<Category>()
            .query()
            .find(id.clone())
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("Category not found".into()))?;

        cat.is_deleted = true;
        cat.updated_id = operator_id(req.claims.as_deref());
        cat.updated_at = now_secs();

        let set = self.ctx.set::<Category>();
        set.update(cat);

        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        tracing::info!("[Category] Soft-deleted: {}", id);
        Ok(format!("Deleted category {}", id))
    }
}
