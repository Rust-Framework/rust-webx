//! Application startup — database initialization and documentation index sync.
//!
//! `DbInitService` 实现 `IHostedService`，在 host 启动时按序执行：
//! 1. `ensure_created()` — 建表 + 写入 `seed.rs` 中的种子数据（角色/分类/资源）；
//! 2. 创建默认 admin 用户（如不存在），密码哈希用 bcrypt；
//! 3. `ensure_all_indexes()` — 扫描 `docs/` 并补齐 INDEX.json；
//! 4. `sync_portfolio_assets()` — 把每个作品的 logo 拷贝到 `wwwroot/assets/works/`。
//!
//! wwwroot 路径按框架约定写死为 `<app_base>/wwwroot`，与 SPA 中间件一致，
//! 不再通过 `AppPaths` 注入。
//!
//! `DbInitService` 是 Singleton（`#[inject]` on `impl IHostedService`），不能在
//! 构造时持有 Scoped 的 `DbContext`（captive dependency）。改为在 `start()` 中
//! 通过 `global_provider().get_owned::<DbContext>()` 获取 owned 实例——按
//! rust-ef 文档，从 root provider 无 scope 解析时退化为 transient（每次全新），
//! 正适合启动期的一次性种子任务。

use std::sync::Arc;

use bcrypt::{hash, DEFAULT_COST};
use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use docbit_contracts::docs::IDocumentService;
use docbit_domain::entities::User;
use docbit_domain::seed::seed;

const ADMIN_EMAIL: &str = "admin@docbit.local";
const ADMIN_DEFAULT_PASSWORD: &str = "admin123";

// `#[derive(Inject)]` 生成 `__rdi_construct_DbInitService` 构造器，
// 自动从 DI 容器解析 `docs: Arc<dyn IDocumentService>`。
// `DbContext` 不在构造期注入（避免 Singleton 持有 Scoped 服务的 captive dependency），
// 改在 `start()` 中通过 `global_provider().get_owned()` 获取 owned 实例。
#[derive(Inject)]
pub struct DbInitService {
    #[inject]
    docs: Arc<dyn IDocumentService>,
}

// `#[inject]` 在 trait impl 上：注册为 `dyn IHostedService`（默认 singleton），
// 框架在 `Host::build()` 时统一收集并启动。
#[inject]
#[async_trait]
impl IHostedService for DbInitService {
    async fn start(&self) -> Result<()> {
        tracing::info!("[DbInitService] Starting initialization...");

        // 从 root provider 解析 owned DbContext（transient：全新实例）。
        let mut ctx: DbContext = global_provider()
            .get_owned()
            .map_err(|e| Error::Internal(format!("DbContext resolution failed: {}", e)))?;
        seed(&mut ctx); // 注册种子数据到 model builder
        ctx.ensure_created()
            .await
            .map_err(|e| Error::Internal(format!("ensure_created failed: {}", e)))?;
        tracing::info!("[DbInitService] Tables created and seed data applied.");

        // 创建默认 admin 用户（仅当不存在时）
        self.ensure_admin_user(&mut ctx).await?;

        // 文档索引补齐 + 作品 logo 同步
        self.docs
            .ensure_all_indexes()
            .map_err(|e| Error::Internal(format!("Doc index generation failed: {}", e)))?;

        let wwwroot = rust_webx::app_base().join("wwwroot");
        self.docs
            .sync_portfolio_assets(&wwwroot)
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
    async fn ensure_admin_user(&self, ctx: &mut DbContext) -> Result<()> {
        let existing = linq!(ctx.set::<User>(), |u: User| u.email == ADMIN_EMAIL)
            .first_or_default()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        if existing.is_some() {
            return Ok(());
        }

        let user_id = docbit_domain::new_id();
        let role_user_id = docbit_domain::new_id();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let password_hash =
            hash(ADMIN_DEFAULT_PASSWORD, DEFAULT_COST).map_err(|e| Error::Internal(e.to_string()))?;

        let user = User {
            id: user_id.clone(),
            name: "Administrator".into(),
            email: ADMIN_EMAIL.into(),
            password_hash,
            created_id: None,
            created_at: now,
            updated_id: None,
            updated_at: now,
            is_deleted: false,
            roles: HasMany::new(),
        };
        let role_user = docbit_domain::entities::RoleUser {
            id: role_user_id,
            user_id,
            role_id: docbit_domain::seed_ids::ROLE_ADMIN.into(),
            created_at: now,
        };
        let users = ctx.set::<User>();
        users.add(user);
        let role_users = ctx.set::<docbit_domain::entities::RoleUser>();
        role_users.add(role_user);
        ctx.save_changes()
            .await
            .map_err(|e| Error::Internal(format!("Failed to create admin user: {}", e)))?;

        tracing::info!(
            "[DbInitService] Created default admin user: {} / {}",
            ADMIN_EMAIL,
            ADMIN_DEFAULT_PASSWORD
        );
        Ok(())
    }
}
