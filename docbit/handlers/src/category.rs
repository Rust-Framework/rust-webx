//! Category handlers — CRUD plus hierarchical tree assembly.

use std::collections::HashMap;
use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*, provider::DbValue};
use rust_webapp::*;
use tokio::sync::Mutex;

use docbit_contracts::category::{
    CategoryModel, CategoryTreeNode, CreateCategoryRequest, DeleteCategoryRequest,
    ListCategoriesRequest, UpdateCategoryRequest,
};
use docbit_domain::entities::Category;

use crate::util::{now_secs, operator_id, parse_id};

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<ListCategoriesRequest, Vec<CategoryTreeNode>>)]
pub struct ListCategoriesHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<CreateCategoryRequest, CategoryModel>)]
pub struct CreateCategoryHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<UpdateCategoryRequest, CategoryModel>)]
pub struct UpdateCategoryHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<DeleteCategoryRequest, String>)]
pub struct DeleteCategoryHandler {
    ctx: Arc<Mutex<DbContext>>,
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
    async fn handle(&self, _: ListCategoriesRequest) -> Result<Vec<CategoryTreeNode>> {
        let cats = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Category>()
                .query()
                .filter_column("is_deleted", "=", DbValue::Bool(false))
                .to_list()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        let models: Vec<CategoryModel> = cats.into_iter().map(CategoryModel::from).collect();
        Ok(build_tree(models))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateCategoryRequest, CategoryModel> for CreateCategoryHandler {
    async fn handle(&self, _: CreateCategoryRequest) -> Result<CategoryModel> {
        unreachable!("handle_with_claims is always called")
    }
    async fn handle_with_claims(
        &self,
        req: CreateCategoryRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<CategoryModel> {
        let op = operator_id(claims);
        let now = now_secs();
        let cat = Category {
            id: 0,
            name: req.name.clone(),
            slug: req.slug.clone(),
            parent_id: req.parent_id,
            sort_order: req.sort_order,
            created_id: op,
            created_at: now,
            updated_id: op,
            updated_at: now,
            is_deleted: false,
            parent: BelongsTo::new(),
            children: HasMany::new(),
        };
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Category>().add(cat);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(format!("Failed to create category: {}", e)))?;
        }
        let created = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Category>()
                .query()
                .filter_column("slug", "=", DbValue::String(req.slug.clone()))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::Internal("Category disappeared after insert".into()))?;
        tracing::info!("[Category] Created: {} ({})", created.name, created.id);
        Ok(CategoryModel::from(created))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateCategoryRequest, CategoryModel> for UpdateCategoryHandler {
    async fn handle(&self, _: UpdateCategoryRequest) -> Result<CategoryModel> {
        unreachable!("handle_with_claims is always called")
    }
    async fn handle_with_claims(
        &self,
        req: UpdateCategoryRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<CategoryModel> {
        let id = parse_id(&req.id)?;
        let mut cat = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Category>()
                .query()
                .filter_column("id", "=", DbValue::I32(id))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("Category not found".into()))?;

        if let Some(n) = req.name {
            cat.name = n;
        }
        if let Some(s) = req.sort_order {
            cat.sort_order = s;
        }
        cat.updated_id = operator_id(claims);
        cat.updated_at = now_secs();
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Category>().update(cat);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
        }
        let updated = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Category>()
                .query()
                .filter_column("id", "=", DbValue::I32(id))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("Category not found after update".into()))?;
        Ok(CategoryModel::from(updated))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteCategoryRequest, String> for DeleteCategoryHandler {
    async fn handle(&self, _: DeleteCategoryRequest) -> Result<String> {
        unreachable!("handle_with_claims is always called")
    }
    async fn handle_with_claims(
        &self,
        req: DeleteCategoryRequest,
        claims: Option<&dyn IClaims>,
    ) -> Result<String> {
        let id = parse_id(&req.id)?;
        let mut cat = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Category>()
                .query()
                .filter_column("id", "=", DbValue::I32(id))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::NotFound("Category not found".into()))?;
        cat.is_deleted = true;
        cat.updated_id = operator_id(claims);
        cat.updated_at = now_secs();
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<Category>().update(cat);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
        }
        tracing::info!("[Category] Soft-deleted: {}", id);
        Ok(format!("Deleted category {}", id))
    }
}
