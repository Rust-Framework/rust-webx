//! Application startup — database initialization, seeding, and doc index generation.

use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*, provider::DbValue};
use rust_webapp::*;
use tokio::sync::Mutex;

use crate::services::docs::DocService;

/// Database initialization service.
#[rust_dicore::inject_attr(singleton, as = dyn IHostedService)]
pub struct DbInitService {
    ctx: Arc<Mutex<DbContext>>,
    docs: Arc<DocService>,
}

#[async_trait]
impl IHostedService for DbInitService {
    async fn start(&self) -> Result<()> {
        tracing::info!("[DbInitService] Starting initialization...");

        {
            let mut ctx = self.ctx.lock().await;
            crate::domain::migrations::m001_initial_20260611::up(&mut ctx)
                .await
                .map_err(|e| Error::Internal(format!("Migration failed: {}", e)))?;
        }
        tracing::info!("[DbInitService] Migrations applied.");

        self.seed_admin().await?;
        self.seed_works().await?;
        self.seed_blog().await?;

        self.docs
            .ensure_all_indexes()
            .map_err(|e| Error::Internal(format!("Doc index generation failed: {}", e)))?;

        tracing::info!("[DbInitService] Initialization complete.");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        tracing::info!("[DbInitService] Shutting down.");
        Ok(())
    }
}

impl DbInitService {
    async fn seed_admin(&self) -> Result<()> {
        let mut ctx = self.ctx.lock().await;
        let query = ctx
            .set::<crate::domain::user::UserEntity>()
            .query()
            .filter_column("email", "=", DbValue::String("admin@docbit.dev".into()));
        drop(ctx);

        if query.first_or_default().await.ok().flatten().is_some() {
            return Ok(());
        }

        let ctx = self.ctx.lock().await;
        let hashed = bcrypt::hash("admin123", bcrypt::DEFAULT_COST)
            .map_err(|e| Error::Internal(format!("Hash failed: {}", e)))?;
        let sql = format!(
            "INSERT INTO users (id, name, email, password_hash, role, created_at) \
             VALUES ('admin-001', 'Admin', 'admin@docbit.dev', '{}', 'admin', '{}')",
            crate::common::escape_sql(&hashed),
            now_secs()
        );
        ctx.provider()
            .execute_migration_command(&sql)
            .await
            .map_err(|e| Error::Internal(format!("Failed to insert admin: {}", e)))?;
        tracing::info!("[DbInitService] Default admin: admin@docbit.dev / admin123");
        Ok(())
    }

    async fn seed_works(&self) -> Result<()> {
        let mut ctx = self.ctx.lock().await;
        let count = ctx
            .set::<crate::domain::work::WorkEntity>()
            .query()
            .count()
            .await
            .unwrap_or(0);
        drop(ctx);

        if count > 0 {
            return Ok(());
        }

        let works = [
            (
                "rust-webapp",
                "rust-webapp",
                "rust-webapp",
                "Rust WebApi Framework",
                "ASP.NET Core 风格的 Rust Web 服务框架，附带完整开发者手册（16 章 + 案例研究）。基于 DI + 中介者模式，支持编译时路由、JWT 认证、OpenAPI、SPA 托管等生产级能力。本站文档即《rust-webapp 开发者手册》在线版。",
                "framework",
                r#"["rust","webapi","framework","di","mediator"]"#,
                "https://gitcode.com/rf2026/rust-webapp",
                "",
                "rust-webapp",
                true,
                1,
            ),
            (
                "docbit",
                "docbit",
                "Docbit Portfolio",
                "开发者个人作品展网站",
                "基于 rust-webapp 构建的作品集站点，集成作品展示、技术博客与自动解析的文档系统。",
                "product",
                r#"["portfolio","docs","blog"]"#,
                "https://gitcode.com/rf2026/rust-webapp",
                "",
                "",
                true,
                2,
            ),
        ];

        for (id, slug, title, subtitle, desc, cat, tags, repo, demo, docs, featured, order) in works {
            let ctx = self.ctx.lock().await;
            let sql = format!(
                "INSERT INTO works (id, slug, title, subtitle, description, category, tags, \
                 repo_url, demo_url, docs_slug, featured, sort_order, created_at) \
                 VALUES ('{}', '{}', '{}', '{}', '{}', '{}', '{}', '{}', '{}', '{}', {}, {}, '{}')",
                id,
                slug,
                crate::common::escape_sql(title),
                crate::common::escape_sql(subtitle),
                crate::common::escape_sql(desc),
                cat,
                tags,
                repo,
                demo,
                docs,
                if featured { 1 } else { 0 },
                order,
                now_secs()
            );
            ctx.provider()
                .execute_migration_command(&sql)
                .await
                .map_err(|e| Error::Internal(format!("Failed to seed work: {}", e)))?;
        }
        tracing::info!("[DbInitService] Seeded portfolio works");
        Ok(())
    }

    async fn seed_blog(&self) -> Result<()> {
        let mut ctx = self.ctx.lock().await;
        let count = ctx
            .set::<crate::domain::blog::BlogPostEntity>()
            .query()
            .count()
            .await
            .unwrap_or(0);
        drop(ctx);

        if count > 0 {
            return Ok(());
        }

        let content = r#"## 为什么选择 rust-webapp？

在传统 Rust Web 生态中，开发者往往需要在路由、DI、中间件之间手动拼装大量样板代码。rust-webapp 借鉴 ASP.NET Core 的设计理念，将 **请求即端点** 的模式带入 Rust：

- `IRequest<T>` + `#[get("/path")]` 一行定义路由
- `#[handler]` 编译时自动注册到 DI 容器
- `IMediator` 统一调度请求与事件

## 快速体验

```bash
cargo run -p docbit
```

访问作品集首页，点击 **rust-webapp** 卡片即可查看完整文档。

## 下一步

阅读 [rust-webapp 文档](/works/rust-webapp/docs) 了解框架核心概念与 API 设计。
"#;

        let ctx = self.ctx.lock().await;
        let sql = format!(
            "INSERT INTO blog_posts (id, slug, title, summary, content, tags, published_at, created_at) \
             VALUES ('blog-001', 'welcome-to-rust-webapp', '欢迎使用 rust-webapp 作品集', \
             '介绍 rust-webapp 框架的设计理念和本作品集站点的用途。', '{}', \
             '[\"rust\",\"webapi\",\"portfolio\"]', '{}', '{}')",
            crate::common::escape_sql(content),
            now_secs(),
            now_secs()
        );
        ctx.provider()
            .execute_migration_command(&sql)
            .await
            .map_err(|e| Error::Internal(format!("Failed to seed blog: {}", e)))?;
        tracing::info!("[DbInitService] Seeded sample blog post");
        Ok(())
    }
}

fn now_secs() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}
