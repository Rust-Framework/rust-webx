//! Application startup — database initialization and documentation index sync.
//!
//! `DbInitService` 实现 `IHostedService`，在 host 启动时按序执行：
//! 1. `ensure_created()` — 建表 + 写入 `seed.rs` 中的种子数据（角色/分类/资源）；
//! 2. 创建默认 admin 用户（如不存在），密码哈希用 bcrypt；
//! 3. `ensure_all_indexes()` — 扫描 `docs/` 并补齐 INDEX.json；
//! 4. `sync_portfolio_assets()` — 把每个作品的 logo 拷贝到 `wwwroot/assets/works/`。

use std::sync::Arc;

use bcrypt::{hash, DEFAULT_COST};
use rust_ef::{db_context::DbContext, prelude::*, provider::DbValue};
use rust_webapp::*;
use tokio::sync::Mutex;

use docbit_contracts::docs::IDocumentService;
use docbit_domain::seed::seed;

use crate::paths::AppPaths;

const ADMIN_EMAIL: &str = "admin@docbit.local";
const ADMIN_DEFAULT_PASSWORD: &str = "admin123";

#[rust_dicore::inject_attr(singleton, as = dyn IHostedService)]
pub struct DbInitService {
    ctx: Arc<Mutex<DbContext>>,
    docs: Arc<dyn IDocumentService>,
    paths: Arc<AppPaths>,
}

#[async_trait]
impl IHostedService for DbInitService {
    async fn start(&self) -> Result<()> {
        tracing::info!("[DbInitService] Starting initialization...");

        {
            let mut ctx = self.ctx.lock().await;
            seed(&mut ctx); // 注册种子数据到 model builder
            ctx.ensure_created()
                .await
                .map_err(|e| Error::Internal(format!("ensure_created failed: {}", e)))?;
            tracing::info!("[DbInitService] Tables created and seed data applied.");
        }

        // 创建默认 admin 用户（仅当不存在时）
        self.ensure_admin_user().await?;

        // 文档索引补齐 + 作品 logo 同步
        self.docs
            .ensure_all_indexes()
            .map_err(|e| Error::Internal(format!("Doc index generation failed: {}", e)))?;

        self.docs
            .sync_portfolio_assets(&self.paths.wwwroot)
            .map_err(|e| Error::Internal(format!("Portfolio asset sync failed: {}", e)))?;

        tracing::info!("[DbInitService] Initialization complete.");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        tracing::info!("[DbInitService] Shutting down.");
        Ok(())
    }
}

impl DbInitService {
    async fn ensure_admin_user(&self) -> Result<()> {
        let existing = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<docbit_domain::entities::User>()
                .query()
                .filter_column("email", "=", DbValue::String(ADMIN_EMAIL.into()))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        };
        if existing.is_some() {
            return Ok(());
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let password_hash =
            hash(ADMIN_DEFAULT_PASSWORD, DEFAULT_COST).map_err(|e| Error::Internal(e.to_string()))?;

        let user = docbit_domain::entities::User {
            id: 0,
            name: "Administrator".into(),
            email: ADMIN_EMAIL.into(),
            password_hash,
            created_id: None, // 首条 admin 无创建人
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            roles: HasMany::new(),
        };
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<docbit_domain::entities::User>().add(user);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(format!("Failed to create admin user: {}", e)))?;
        }

        // 关联 admin 角色（role_id=1）
        let admin_user = {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<docbit_domain::entities::User>()
                .query()
                .filter_column("email", "=", DbValue::String(ADMIN_EMAIL.into()))
                .first_or_default()
                .await
                .map_err(|e| Error::Internal(e.to_string()))?
        }
        .ok_or_else(|| Error::Internal("Admin user disappeared after insert".into()))?;

        let role_user = docbit_domain::entities::RoleUser {
            id: 0,
            user_id: admin_user.id,
            role_id: 1, // admin
            created_at: now,
        };
        {
            let mut ctx = self.ctx.lock().await;
            ctx.set::<docbit_domain::entities::RoleUser>().add(role_user);
            ctx.save_changes()
                .await
                .map_err(|e| Error::Internal(format!("Failed to assign admin role: {}", e)))?;
        }

        tracing::info!(
            "[DbInitService] Created default admin user: {} / {}",
            ADMIN_EMAIL,
            ADMIN_DEFAULT_PASSWORD
        );
        Ok(())
    }
}
