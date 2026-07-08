# 可复用的模式提炼

## 模式 1：inject_attr + #[handler(inject)]

```rust
#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<LoginRequest, AuthResponse>)]
pub struct LoginHandler {
    auth: Arc<dyn IAuthService>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<LoginRequest, AuthResponse> for LoginHandler { ... }
```

**复用场景**：任何需要 DI 的 HTTP 端点。

## 模式 2：接口在 contracts，实现在 handlers

```rust
// contracts/blog.rs — 契约
pub trait IBlogService: Send + Sync {
    fn list_all_posts(&self) -> Result<Vec<BlogPostSummary>, String>;
}

pub struct ListBlogPostsRequest;
#[get("/api/blog")]
impl IRequest<Vec<BlogPostSummary>> for ListBlogPostsRequest {}

// handlers/blog.rs — 实现
#[rust_dicore::inject_attr(singleton, as = dyn IBlogService)]
pub struct BlogService {
    paths: Arc<AppPaths>,
}

impl IBlogService for BlogService { ... }

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<ListBlogPostsRequest, Vec<BlogPostSummary>>)]
pub struct ListBlogPostsHandler {
    blog: Arc<dyn IBlogService>,
}
```

**复用场景**：文档、博客、订单、通知等可替换业务模块。换存储实现时 Handler 与契约无需改动。

## 模式 3：IHostedService 初始化

```rust
#[rust_dicore::inject_attr(singleton, as = dyn IHostedService)]
pub struct DbInitService {
    ctx: Arc<Mutex<DbContext>>,
    docs: Arc<dyn IDocumentService>,
    paths: Arc<AppPaths>,
}
```

**复用场景**：迁移、索引构建、资源同步——不在 `main()` 写初始化逻辑。

## 模式 4：薄 Handler + 厚 Service 实现

```rust
async fn handle(&self, req: GetDocIndexRequest) -> Result<DocIndex> {
    self.docs.index(&req.work).map_err(|e| Error::NotFound(e))
}
```

Service 实现不感知 HTTP；Handler 只做参数传递与 `Error` 映射。

## 模式 5：组合根最小化

```rust
.register(common::bootstrap::configure)  // 仅 AppPaths + DbContext
```

业务代码通过 `inject_attr` 在 handlers 自注册，开发时聚焦 `contracts` / `handlers` / `domain`。

## 模式 6：DTO 在 contracts，实体在 domain

```rust
// contracts/blog.rs
#[derive(Serialize)]
pub struct BlogPostSummary { pub slug: String, pub title: String }

// domain/comment.rs — 仅持久化实体
pub struct BlogCommentEntity { ... }

// handlers/blog_service.rs — 映射
fn to_summary(meta: &BlogPostMeta) -> BlogPostSummary { ... }
```

contracts 不引用 domain；domain 可通过 `use crate::contracts::…` 复用枚举。

## 从 Docbit 到新项目的检查清单

- [ ] `contracts` / `handlers` / `domain` 三层，无 `services/` 目录
- [ ] `I…Service` trait 在 `contracts/`，实现在 `handlers/`
- [ ] `contracts` 仅依赖框架，不引用 `domain`
- [ ] Handler 注入 `Arc<dyn I…Service>`，不用具体类型
- [ ] `bootstrap::configure` 只注册 DbContext、路径等基础设施
- [ ] 初始化放在 `IHostedService`
- [ ] `main.rs` 仅 Host 配置
- [ ] `appsettings.json` 配置框架运行时参数
- [ ] 认证端点使用 `#[authorize]`

## 小结

面向接口 + 自动注册让业务开发**自动化、聚焦契约、组合根简洁**——这是 rust-webx 业务应用模板的核心价值。

下一章：[迁移指南](../16-migration/INDEX.md)
