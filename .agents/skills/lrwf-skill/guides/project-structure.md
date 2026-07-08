# LRWF 业务应用项目结构（最佳实践）

基于 LRWF 开发的业务应用应遵循**面向接口、契约驱动**的分层。目标是高内聚、低耦合：只关注契约，不关注实现细节。

## 标准目录

```
my-app/
├── appsettings.json              # 框架配置（Development 可用 appsettings.Development.json）
├── Cargo.toml
├── wwwroot/                      # SPA 静态资源（可选）
└── src/
    ├── main.rs                   # 组合根：Host 启动，仅框架级配置
    ├── startup.rs                # IHostedService（迁移、种子数据等）
    ├── common/                   # 基础设施（路径解析、bootstrap、拦截器）
    ├── contracts/                # 契约层：Request / Response / enum / I…Service
    ├── handlers/                 # 应用层：Handler + Service 实现
    └── domain/                   # 领域层：实体、EF 配置、迁移
```

**禁止**单独设立 `services/`、`requests/` 等与上述职责重叠的目录。业务服务接口在 `contracts/`，实现在 `handlers/`。

## 各层职责

### contracts — 对外契约（仅依赖框架）

拥有：

- `IRequest` / `IRequestHandler` 相关的 **Request、Response DTO**
- 跨层共享的 **enum、值对象**
- 业务 **接口 trait**（`IBlogService`、`IUserService` 等）
- 路由宏（`#[get]`、`#[post]`）、授权元数据（`#[authorize]`）

```rust
// contracts/blog.rs
use lrwf::*;  // 或 rust_webx::*

#[derive(Serialize, Deserialize)]
pub struct BlogPostSummary {
    pub slug: String,
    pub title: String,
}

pub trait IBlogService: Send + Sync {
    fn list_posts(&self) -> Result<Vec<BlogPostSummary>, String>;
}

pub struct ListBlogPostsRequest;

#[get("/api/blog")]
impl IRequest<Vec<BlogPostSummary>> for ListBlogPostsRequest {}
```

**硬性规则：**

| 允许 | 禁止 |
|------|------|
| `use lrwf::*` / `use rust_webx::*` | `use crate::domain::*` |
| `serde`、`std` | `use crate::handlers::*` |
| 纯数据类型 + trait 定义 | `async fn` 业务实现、数据库访问 |

contracts 是 API 说明书与抽象边界，团队讨论接口时**只看 contracts**。

### handlers — 履约实现（Handler + Service）

拥有：

- `IRequestHandler<Req, Rsp>` 实现（薄编排）
- `I…Service` 的**具体实现**（`BlogService`、`DocService` 等）
- 通过 `inject_attr` + `#[handler(inject)]` **自动注册**

```rust
// handlers/blog.rs
use crate::contracts::blog::{IBlogService, BlogPostSummary, ListBlogPostsRequest};
use crate::domain::blog::BlogPostEntity;

#[rust_dicore::inject_attr(singleton, as = dyn IBlogService)]
pub struct BlogService {
    paths: Arc<AppPaths>,
}

impl IBlogService for BlogService {
    fn list_posts(&self) -> Result<Vec<BlogPostSummary>, String> {
        // 读 domain 实体 → 映射为 contracts DTO
    }
}

#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<ListBlogPostsRequest, Vec<BlogPostSummary>>)]
pub struct ListBlogPostsHandler {
    blog: Arc<dyn IBlogService>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<ListBlogPostsRequest, Vec<BlogPostSummary>> for ListBlogPostsHandler {
    async fn handle(&self, _req: ListBlogPostsRequest) -> Result<Vec<BlogPostSummary>> {
        self.blog.list_posts().map_err(|e| Error::Internal(e))
    }
}
```

要点：

- Handler **只注入** `Arc<dyn I…Service>`，不依赖具体实现类型
- Service 实现与对应 Handler 可同文件或同模块，按业务域拆分
- 复杂逻辑在 Service 实现中，Handler 负责参数传递与 `Error` 映射

### domain — 持久化与领域实体

拥有：

- 数据库实体（`UserEntity`、`BlogPostEntity`）
- EF / rust-ef 实体配置
- 迁移（`domain/migrations/`）

```rust
// domain/blog.rs
use crate::contracts::blog::PostStatus;  // 可复用 contracts 中的枚举

pub struct BlogPostEntity {
    pub slug: String,
    pub status: PostStatus,
    // ...
}
```

**规则：**

- **可以**引用 `contracts` 复用枚举或共享 model
- **禁止**引用 `handlers`
- **禁止**依赖 `lrwf` / `rust_webx` 框架类型（`serde` 除外）

### main.rs — 组合根

```rust
#[tokio::main]
async fn main() -> Result<()> {
    Host::builder()
        .register(common::bootstrap::configure)  // 仅基础设施：DbContext、AppPaths
        .use_auth()
        .build()
        .run()
        .await
}
```

`main.rs` **不做**业务 Handler / Service 的手动注册。`ServiceCollection::from_injected()` 自动收集带 `inject_attr` 的类型。

### appsettings.json

框架配置文件，与 ASP.NET Core 同名约定。Host 通过 `AppMode` 加载 `appsettings.json` 及环境覆盖文件。

## 依赖方向

```
                    ┌─────────────┐
                    │  framework  │  lrwf / rust_webx
                    └──────┬──────┘
                           │
                    ┌──────▼──────┐
                    │  contracts  │  仅依赖 framework
                    └──────┬──────┘
              ┌────────────┼────────────┐
              │            │            │
       ┌──────▼──────┐     │     ┌──────▼──────┐
       │   domain    │     │     │  handlers   │
       │ (可引用     │     │     │ (实现契约)   │
       │  contracts) │     │     └─────────────┘
       └─────────────┘     │
              ▲             │
              └─────────────┘  handlers 可引用 domain + contracts
```

**禁止的依赖：**

- `contracts` → `domain` / `handlers`
- `domain` → `handlers`
- Handler 依赖具体 Service 类型（应使用 `Arc<dyn I…Service>`）

## 请求数据流

```
HTTP Request
  → contracts（Request 反序列化）
  → handlers（Handler 编排）
  → Arc<dyn I…Service>（业务接口）
  → handlers 内 Service 实现
  → domain（实体读写）
  → contracts（Response DTO）
  → HTTP Response
```

## 新增业务能力的标准流程

1. 在 `contracts/` 定义 Response DTO、enum、`I…Service` trait、`IRequest` 路由
2. 在 `handlers/` 实现 `I…Service` 与 `IRequestHandler`，加 `inject_attr`
3. 如需持久化，在 `domain/` 添加实体与迁移
4. `main.rs` 无需修改（除非新增基础设施注册）

## 反模式（必须纠正）

| 反模式 | 正确做法 |
|--------|---------|
| `contracts` 中 `use crate::domain::*` | DTO 定义在 contracts；domain 映射实体 |
| 独立的 `services/` 目录 | 接口在 contracts，实现在 handlers |
| Handler 注入 `Arc<BlogService>` | `Arc<dyn IBlogService>` |
| `main.rs` 逐个 `singleton::<BlogService>` | `inject_attr(as = dyn IBlogService)` |
| contracts 中写 `impl IBlogService` | 实现放 handlers |
| domain 中 `use rust_webx::*` | domain 保持框架无关 |

## 与 Docbit 参考实现

Docbit 案例应用已按本结构实现。新代码应遵循 contracts / handlers / domain 三层规范。
