# 可复用的模式提炼

## 模式 1：#[derive(Inject)] + #[handler(inject)]

```rust
#[derive(Inject)]
pub struct GetDocIndexHandler {
    #[inject]
    docs: Arc<dyn IDocumentService>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<GetDocIndexRequest, DocIndex> for GetDocIndexHandler { ... }
```

**复用场景**：任何需要 DI 的 HTTP 端点。

## 模式 2：中介者直连 DbContext（无 service 抽象）

```rust
// contracts/blog.rs — 契约：DTO + 路由，不定义 service trait
#[derive(Serialize)]
pub struct BlogPostSummary { /* ... */ }

#[get("/api/blog")]
impl IRequest<Vec<BlogPostSummary>> for ListBlogPostsRequest {}

// handlers/blog.rs — 直接持有 owned DbContext
#[derive(Inject)]
pub struct ListBlogPostsHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListBlogPostsRequest, Vec<BlogPostSummary>> for ListBlogPostsHandler {
    async fn handle(&mut self, _: ListBlogPostsRequest) -> ActionResult<Vec<BlogPostSummary>> {
        let blogs = linq!(self.ctx.set::<Blog>();).to_list().await.map_ef()?;
        Ok(blogs.into_iter().map(BlogPostSummary::from).collect())
    }
}
```

**复用场景**：CRUD 类业务模块（博客、分类、评论）。只有实现需要可替换时才抽 `I…Service`——`IDocumentService`（文件系统实现可 mock）就是这种场景。

## 模式 3：IHostedService 初始化

```rust
#[derive(Inject)]
pub struct DbInitService {
    #[inject]
    docs: Arc<dyn IDocumentService>,
}

#[inject]
#[async_trait]
impl IHostedService for DbInitService {
    async fn start(&self) -> Result<()> {
        // ensure_created → seed → 索引/资源同步
        Ok(())
    }
}
```

**复用场景**：schema 初始化、seed、索引构建、资源同步——不在 `main()` 写初始化逻辑。

## 模式 4：薄 Handler + 厚 Service 实现

```rust
async fn handle(&mut self, req: GetDocIndexRequest) -> ActionResult<DocIndex> {
    self.docs.index(&req.work).map_err(Error::NotFound)
}
```

Service 实现不感知 HTTP；Handler 只做参数传递与 `Error` 映射。

## 模式 5：组合根最小化

```rust
.register(|svc| svc.add_docbit_db())  // 仅 DbContext 等基础设施
```

业务代码通过 `#[derive(Inject)]` + `#[inject]` 在 handlers 自注册，开发时聚焦 `contracts` / `handlers` / `domain`。

## 模式 6：DTO 在 contracts，实体在 domain

```rust
// contracts/blog.rs
#[derive(Serialize)]
pub struct BlogPostSummary { pub slug: String, pub title: String }

// domain/entities/blog.rs — 仅持久化实体
pub struct Blog { /* ... */ }

// domain/conversions.rs — 映射
impl From<Blog> for BlogPostSummary { /* ... */ }
```

contracts 不引用 domain；domain 可通过 `use docbit_contracts::…` 复用枚举。

## 模式 7：身份作为请求字段

docbit 的博客/评论端点把身份直接声明为 `IdentityUser`，Handler 不再读 claims：

```rust
// contracts/comment.rs
#[derive(WebxRequestMeta)]
pub struct CreateCommentRequest {
    #[fromclaim]
    pub user: IdentityUser,
    pub blog_id: String,
    pub content: String,
}

// handlers/comment.rs
let user = req.require_user()?;
let comment = Comment {
    user_id: user.id.clone(),
    user_name: user.display_name().to_string(),
    content: req.content,
    ..Default::default()
};
```

好处：`401`、空值、角色判断都在绑定层完成；多租户模块用 `user.tenant_id` 过滤，不再依赖每个查询自觉。
完整场景见[用户身份与多租户](../05-request-pattern/user-identity.md)。

## 新项目检查清单

见 [代码审查清单](../14-best-practices/code-review-checklist.md) 与 [职责归属与边界](../12-project-structure/responsibility-division.md)。

## 小结

面向接口 + 自动注册让业务开发**自动化、聚焦契约、组合根简洁**——这是 rust-webx 业务应用模板的核心价值。

下一章：[迁移指南](../16-migration/INDEX.md)
