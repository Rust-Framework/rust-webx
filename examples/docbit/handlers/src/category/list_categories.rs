//! `GET /api/categories` — the category forest, sorted by `sort_order` then id.

use std::collections::HashMap;

use rust_ef::{db_context::DbContext, prelude::*};
use webx::*;

use docbit_contracts::category::{CategoryModel, CategoryTreeNode, ListCategoriesRequest};
use docbit_domain::entities::Category;

use crate::common::MAX_LIST_ROWS;

#[derive(Inject)]
pub struct ListCategoriesHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListCategoriesRequest, Vec<CategoryTreeNode>> for ListCategoriesHandler {
    async fn handle(&mut self, _: ListCategoriesRequest) -> ActionResult<Vec<CategoryTreeNode>> {
        let cats = linq!(self.ctx.set::<Category>(); order_by c.sort_order asc)
            .take(MAX_LIST_ROWS)
            .to_list()
            .await?;

        Ok(build_tree(cats.iter().map(CategoryModel::from).collect()))
    }
}

/// Assemble the flat, sorted category list into a forest.
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
