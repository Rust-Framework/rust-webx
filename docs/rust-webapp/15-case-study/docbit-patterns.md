# 可复用的模式提炼

## 模式 1：inject_attr + #[handler(inject)]

Docbit 所有 Handler 的标准模式：

```rust
#[inject_attr(singleton, as = dyn IRequestHandler<LoginRequest, AuthResponse>)]
pub struct LoginHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<LoginRequest, AuthResponse> for LoginHandler { ... }
```

**复用场景**：任何需要数据库、缓存、外部服务注入的 Handler。

## 模式 2：IHostedService 初始化

```rust
#[inject_attr(singleton, as = dyn IHostedService)]
pub struct DbInitService { ... }

#[async_trait]
impl IHostedService for DbInitService {
    async fn start(&self) -> Result<()> {
        run_migrations().await?;
        seed_data().await?;
        Ok(())
    }
}
```

**复用场景**：数据库迁移、种子数据、缓存预热、索引构建。

## 模式 3：声明式授权

```rust
#[get("/api/auth/me")]
#[authorize]
impl IRequest<UserView> for AuthMeRequest {}
```

**复用场景**：任何需要登录的端点加 `#[authorize]`，管理员端点加 `#[authorize(role = "admin")]`。

## 模式 4：Service 层复用

```rust
// services/docs.rs — 不感知 HTTP
impl DocService {
    pub fn index(&self, work: &str) -> Result<DocIndex, String> { ... }
    pub fn content(&self, work: &str, path: &str) -> Result<DocContent, String> { ... }
}

// handlers/docs.rs — 薄 Handler
impl IRequestHandler<GetDocIndexRequest, DocIndex> for GetDocIndexHandler {
    async fn handle(&self, req: GetDocIndexRequest) -> Result<DocIndex> {
        self.docs.index(&req.work).map_err(|e| Error::NotFound(e))
    }
}
```

**复用场景**：文件操作、外部 API 调用、复杂查询逻辑。

## 模式 5：全栈单体

```rust
Host::builder()
    .use_spa("wwwroot")
    .use_auth()
    .build()
```

**复用场景**：作品集、管理后台、中小型 SaaS 产品。

## 模式 6：文档即代码

```
docs/{work}/
├── INDEX.json    # 左侧菜单
├── INDEX.md      # 章节目录
├── FOREWORD.md
└── {chapter}/
    ├── INDEX.md
    └── *.md
```

**复用场景**：产品文档、API 文档、技术手册，通过 DocService 自动服务。

## 从 Docbit 到你自己项目的检查清单

- [ ] 采用 contracts / handlers / domain / services 分层
- [ ] Handler 使用 inject_attr 模式
- [ ] 初始化逻辑放在 IHostedService
- [ ] main.rs 只做 Host 配置
- [ ] 认证端点使用 #[authorize]
- [ ] 错误使用 Error 变体

## 小结

Docbit 不仅是一个展示站点，更是 rust-webapp 应用形态的**标准答案**。复制这些模式，你就掌握了框架的精髓。

下一章：[迁移指南](../16-migration/INDEX.md)
