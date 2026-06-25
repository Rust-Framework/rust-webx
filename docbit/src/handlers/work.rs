//! Work handlers — portfolio showcase CRUD.

use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*, provider::DbValue};
use rust_webapp::*;
use tokio::sync::Mutex;

use crate::contracts::work::*;
use crate::domain::work::{WorkEntity, WorkModel};

fn str_or_empty(v: Option<String>) -> String {
    v.unwrap_or_default()
}

fn tags_json(tags: &[String]) -> String {
    serde_json::to_string(tags).unwrap_or_else(|_| "[]".into())
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<ListWorksRequest, Vec<WorkModel>>)]
pub struct ListWorksHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<GetWorkRequest, WorkModel>)]
pub struct GetWorkHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<CreateWorkRequest, WorkModel>)]
pub struct CreateWorkHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<UpdateWorkRequest, WorkModel>)]
pub struct UpdateWorkHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<DeleteWorkRequest, String>)]
pub struct DeleteWorkHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListWorksRequest, Vec<WorkModel>> for ListWorksHandler {
    async fn handle(&self, _req: ListWorksRequest) -> Result<Vec<WorkModel>> {
        let query = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<WorkEntity>()
                .query()
        };
        let works = query
            .to_list()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        let mut models: Vec<WorkModel> = works.into_iter().map(WorkModel::from).collect();
        models.sort_by_key(|w| w.sort_order);
        Ok(models)
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetWorkRequest, WorkModel> for GetWorkHandler {
    async fn handle(&self, req: GetWorkRequest) -> Result<WorkModel> {
        let query = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<WorkEntity>()
                .query()
                .filter_column("slug", "=", DbValue::String(req.slug))
        };
        let work = query
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("Work not found".into()))?;
        Ok(WorkModel::from(work))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateWorkRequest, WorkModel> for CreateWorkHandler {
    async fn handle(&self, req: CreateWorkRequest) -> Result<WorkModel> {
        let id = new_id();
        let now = now_secs();
        let repo_url = req.repo_url.clone();
        let demo_url = req.demo_url.clone();
        let docs_slug = req.docs_slug.clone();
        let sql = format!(
            "INSERT INTO works (id, slug, title, subtitle, description, category, tags, \
             repo_url, demo_url, docs_slug, featured, sort_order, created_at) \
             VALUES ('{}', '{}', '{}', '{}', '{}', '{}', '{}', '{}', '{}', '{}', {}, {}, '{}')",
            id,
            crate::common::escape_sql(&req.slug),
            crate::common::escape_sql(&req.title),
            crate::common::escape_sql(&req.subtitle),
            crate::common::escape_sql(&req.description),
            crate::common::escape_sql(&req.category),
            crate::common::escape_sql(&tags_json(&req.tags)),
            crate::common::escape_sql(&str_or_empty(repo_url.clone())),
            crate::common::escape_sql(&str_or_empty(demo_url.clone())),
            crate::common::escape_sql(&str_or_empty(docs_slug.clone())),
            if req.featured { 1 } else { 0 },
            req.sort_order,
            now
        );
        {
            let ctx = self.ctx.lock().await;
            ctx.provider()
                .execute_migration_command(&sql)
                .await
                .map_err(|e| Error::Internal(format!("Failed to create work: {}", e)))?;
        }
        Ok(WorkModel {
            id,
            slug: req.slug,
            title: req.title,
            subtitle: req.subtitle,
            description: req.description,
            category: req.category,
            tags: req.tags,
            repo_url,
            demo_url,
            docs_slug,
            featured: req.featured,
            sort_order: req.sort_order,
            created_at: now,
        })
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UpdateWorkRequest, WorkModel> for UpdateWorkHandler {
    async fn handle(&self, req: UpdateWorkRequest) -> Result<WorkModel> {
        let query = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<WorkEntity>()
                .query()
                .filter_column("slug", "=", DbValue::String(req.slug.clone()))
        };
        let existing = query
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| Error::NotFound("Work not found".into()))?;

        let title = req.title.unwrap_or(existing.title);
        let subtitle = req.subtitle.unwrap_or(existing.subtitle);
        let description = req.description.unwrap_or(existing.description);
        let category = req.category.unwrap_or(existing.category);
        let tags = req
            .tags
            .map(|t| tags_json(&t))
            .unwrap_or(existing.tags);
        let repo_url = req.repo_url.unwrap_or(existing.repo_url);
        let demo_url = req.demo_url.unwrap_or(existing.demo_url);
        let docs_slug = req.docs_slug.unwrap_or(existing.docs_slug);
        let featured = req.featured.unwrap_or(existing.featured != 0);
        let sort_order = req.sort_order.unwrap_or(existing.sort_order);

        let sql = format!(
            "UPDATE works SET title='{}', subtitle='{}', description='{}', category='{}', \
             tags='{}', repo_url='{}', demo_url='{}', docs_slug='{}', featured={}, sort_order={} \
             WHERE slug='{}'",
            crate::common::escape_sql(&title),
            crate::common::escape_sql(&subtitle),
            crate::common::escape_sql(&description),
            crate::common::escape_sql(&category),
            crate::common::escape_sql(&tags),
            crate::common::escape_sql(&repo_url),
            crate::common::escape_sql(&demo_url),
            crate::common::escape_sql(&docs_slug),
            if featured { 1 } else { 0 },
            sort_order,
            crate::common::escape_sql(&req.slug)
        );
        {
            let ctx = self.ctx.lock().await;
            ctx.provider()
                .execute_migration_command(&sql)
                .await
                .map_err(|e| Error::Internal(format!("Failed to update work: {}", e)))?;
        }
        Ok(WorkModel::from(WorkEntity {
            id: existing.id,
            slug: req.slug,
            title,
            subtitle,
            description,
            category,
            tags,
            repo_url,
            demo_url,
            docs_slug,
            featured: if featured { 1 } else { 0 },
            sort_order,
            created_at: existing.created_at,
        }))
    }
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<DeleteWorkRequest, String> for DeleteWorkHandler {
    async fn handle(&self, req: DeleteWorkRequest) -> Result<String> {
        let sql = format!(
            "DELETE FROM works WHERE slug='{}'",
            crate::common::escape_sql(&req.slug)
        );
        {
            let ctx = self.ctx.lock().await;
            ctx.provider()
                .execute_migration_command(&sql)
                .await
                .map_err(|e| Error::Internal(format!("Failed to delete work: {}", e)))?;
        }
        Ok(format!("Deleted work {}", req.slug))
    }
}

fn new_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| format!("{:x}", d.as_nanos()))
        .unwrap_or_else(|_| "0".to_string())
}

fn now_secs() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}
