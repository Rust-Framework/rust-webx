# 第三方库集成

## 数据库（rust-ef 示例）

Docbit 使用 `rust-ef` + SQLite：

```rust
// main.rs
let options = Arc::new(DbContextOptionsBuilder::new()
    .use_sqlite("app.db")
    .build());

Host::builder()
    .register(move |svc| {
        svc.singleton::<Mutex<DbContext>>(move |_| {
            Arc::new(Mutex::new(DbContext::from_options(&options).unwrap()))
        });
    })
```

迁移在 `IHostedService` 中执行：

```rust
async fn start(&self) -> Result<()> {
    migrations::m001_initial::up(&mut ctx).await?;
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

1. 第三方类型在 `main.rs` 注册到 DI
2. 业务代码通过 trait 或 Service 封装访问
3. 初始化逻辑放 `IHostedService`
4. domain 层不直接依赖第三方 crate

## 小结

框架不限制技术选型，通过 DI 和分层与任何库集成。

下一章：[最佳实践](../14-best-practices/INDEX.md)
