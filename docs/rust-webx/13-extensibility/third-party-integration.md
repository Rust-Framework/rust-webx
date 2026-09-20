# 第三方库集成

## 数据库（rust-ef 示例）

Docbit 使用 `rust-ef` + SQLite：

```rust
// host/src/startup/extensions/service_collection.rs
use std::sync::Arc;

use rust_ef::db_context::{DbContext, DbContextOptionsBuilder};
use rust_ef_sqlite::DbContextOptionsBuilderExt as _; // 提供 .use_sqlite()
use webx::rust_dix::ServiceCollection;
use webx::*;

pub trait ServiceCollectionExt {
    fn add_app_db(self) -> Self;
}

impl ServiceCollectionExt for ServiceCollection {
    fn add_app_db(self) -> Self {
        // 路径来自 app_base()，不是相对路径
        let path = app_base().join("app.db");

        let mut options = DbContextOptionsBuilder::new();
        options.use_sqlite(&path.to_string_lossy());
        let options = Arc::new(options.build());
        options.create_provider().expect("provider initialization failed");

        // Scoped：每次请求生成一份 owned DbContext
        self.scoped(move |_| {
            Arc::new(DbContext::from_options(&options).expect("Failed to create DbContext"))
        })
    }
}
```

Handler 以 owned 字段注入上下文：

```rust
#[derive(Inject)]
pub struct CreateItemHandler {
    #[inject(owned)]
    ctx: DbContext,
}
```

schema 初始化在 `IHostedService` 中执行（没有版本化迁移模块，直接 ensure schema 后 seed）：

```rust
async fn start(&self) -> Result<()> {
    let mut ctx: DbContext = dispatch_provider()
        .get_owned()
        .map_err(|e| Error::Internal(format!("DbContext resolution failed: {}", e)))?;

    ctx.ensure_created()
        .await
        .map_err(|e| Error::Internal(format!("ensure_created failed: {}", e)))?;
    // 之后可执行 seed
    Ok(())
}
```

## 密码哈希（bcrypt）

```rust
let hashed = bcrypt::hash(password, bcrypt::DEFAULT_COST)?;
let valid = bcrypt::verify(password, &stored_hash)?;
```

## JWT 签发（jsonwebtoken）

```rust
use jsonwebtoken::{encode, EncodingKey, Header};

let token = encode(
    &Header::default(),
    &claims,
    &EncodingKey::from_secret(jwt_secret().as_bytes()),
)?;
```

## 外部 HTTP 客户端

```rust
// 在 Service 中封装
pub struct GitHubClient {
    client: reqwest::Client,
    token: String,
}

impl GitHubClient {
    pub async fn get_repo(&self, owner: &str, repo: &str) -> Result<RepoInfo> {
        let resp = self.client
            .get(format!("https://api.github.com/repos/{}/{}", owner, repo))
            .header("Authorization", format!("Bearer {}", self.token))
            .send().await?;
        resp.json().await.map_err(|e| Error::Internal(e.to_string()))
    }
}
```

通过 DI 注入到 Handler。

## 集成原则

1. 第三方类型在 `startup/extensions/` 的 DI 扩展中注册
2. 业务代码通过 trait 或 Service 封装访问
3. 初始化逻辑放 `IHostedService`
4. domain 层不直接依赖第三方 crate

## 小结

框架不限制技术选型，通过 DI 和分层与任何库集成。

下一章：[最佳实践](../14-best-practices/INDEX.md)
