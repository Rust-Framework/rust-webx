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
use docbit_domain::{ApplyTo, ToEntity, ToModel};

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
    let mut nodes: HashMap<i32, CategoryTreeNode> = cats
        .into_iter()
        .map(|c| {
            (
                c.id,
                CategoryTreeNode {
                    category: c,
                    children: Vec::new(),
                    level: 0,
                },
            )
        })
        .collect();

    let roots: Vec<i32> = nodes
        .values()
        .filter(|n| n.category.parent_id.is_none())
        .map(|n| n.category.id)
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
    pool: &mut HashMap<i32, CategoryTreeNode>,
    level: u32,
) -> CategoryTreeNode {
    node.level = level;
    let child_ids: Vec<i32> = pool
        .values()
        .filter(|n| n.category.parent_id == Some(node.category.id))
        .map(|n| n.category.id)
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
        let op = operator_id(req.claims.as_deref()).unwrap_or(0);
        let now = now_secs();
        let slug = req.slug.clone();
        let cat = req.to_entity(op, now);
        self.ctx.set::<Category>().add(cat);
        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(format!("Failed to create category: {}", e)))?;
        // FIXME(framework): rust-ef 1.3.0 save_changes 不回填自增 id，按 slug 回查。
        let created = {
            let q = slug.clone();
            linq!(self.ctx.set::<Category>(), |c: Category| c.slug == q && !c.is_deleted)
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::Internal("Category disappeared after insert".into()))?;
        tracing::info!("[Category] Created: {} ({})", created.name, created.id);
        Ok(created.to_model())
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
            .find(id)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("Category not found".into()))?;

        let op = operator_id(req.claims.as_deref()).unwrap_or(0);
        req.apply_to(&mut cat, op, now_secs());
        self.ctx.set::<Category>().update(cat);
        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        let updated = self
            .ctx
            .set::<Category>()
            .query()
            .find(id)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("Category not found after update".into()))?;
        Ok(updated.to_model())
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
            .find(id)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("Category not found".into()))?;
        cat.is_deleted = true;
        cat.updated_id = operator_id(req.claims.as_deref());
        cat.updated_at = now_secs();
        self.ctx.set::<Category>().update(cat);
        self.ctx
            .save_changes()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        tracing::info!("[Category] Soft-deleted: {}", id);
        Ok(format!("Deleted category {}", id))
    }
}
