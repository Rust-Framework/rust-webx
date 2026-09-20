# 分层模型与依赖方向

## 应用内部分层

rust-webx 推荐的业务应用分层：

```
┌─────────────────────────────────────────┐
│  main.rs / startup.rs    启动与组合根    │
├─────────────────────────────────────────┤
│  handlers/               应用层（履约）   │
│    IRequestHandler 实现                  │
│    I…Service 实现                        │
├─────────────────────────────────────────┤
│  domain/                 持久化与实体层   │
├─────────────────────────────────────────┤
│  contracts/              接口契约层       │
│    Request/Response DTO                  │
│    enum / I…Service trait                │
└─────────────────────────────────────────┘
```

**不设**独立的 `services/` 层。业务接口在 contracts，实现在 handlers。

## 各层职责

### contracts — 契约层

**拥有**：API 路由声明、Request/Response DTO、共享 enum、`I…Service` trait、授权元数据

```rust
// contracts/auth.rs
#[derive(Default, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: UserView,
}

#[post("/api/auth/login")]
impl IRequest<AuthResponse> for LoginRequest {}
```

- 不含业务实现
- **仅依赖框架**（`webx`），**禁止依赖 domain**
- 是对外 API 与业务抽象的「说明书」
- `I…Service` trait 只在该层定义；Docbit 用到的只有 `IDocumentService`，其余业务直接经
  mediator 操作 `DbContext`。约定见 [职责归属与边界](../12-project-structure/responsibility-division.md)。

### handlers — 应用层

**拥有**：`IRequestHandler` 实现、`I…Service` 具体实现、用例编排

```rust
// handlers/auth.rs
#[derive(Inject)]
pub struct LoginHandler {
    #[inject(owned)]
    ctx: DbContext,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<LoginRequest, AuthResponse> for LoginHandler {
    async fn handle(&mut self, req: LoginRequest) -> Result<AuthResponse> {
        // 领域查询经 DbContext，业务规则留在 domain
        ...
    }
}
```

- 薄 Handler：参数传递、`Error` 映射
- 重业务：规则与 domain 访问在 Service 或 domain 中
- 需要抽象时 Handler 只依赖 `Arc<dyn I…Service>`（如 `Arc<dyn IDocumentService>`）

### domain — 领域模型层

**拥有**：实体、值对象、数据库迁移、EF 配置

```rust
// domain/user.rs
use crate::contracts::auth::UserRole;

pub struct UserEntity {
    pub role: UserRole,
    // ...
}
```

- 最稳定的一层
- **可以**引用 contracts 复用枚举或 model
- 不依赖框架类型（除 `serde`）

### main.rs / startup/ — 组合根

**拥有**：Host 配置、基础设施注册、`IHostedService`

- 唯一允许手动注册 `DbContext` 等框架外基础设施的地方（路径统一由 `webx::app_base()` 派生）
- 业务 Handler / Service 由 `#[inject]` 自动收集，**不在 main 手动注册**

### appsettings.json

框架运行时配置，与 ASP.NET Core 同名约定。

## 依赖方向

```
framework (webx)
    ↑
contracts  ←── domain（可复用 contracts 类型）
    ↑
handlers ──→ domain
```

**禁止**：

- contracts → domain / handlers
- domain → handlers
- handlers 依赖具体 Service 类型（应使用 `Arc<dyn I…Service>`）

## Docbit 实例（目标结构）

```
docbit/
├── contracts/src/   # LoginRequest, IDocumentService trait, BlogPostSummary DTO, ...
├── domain/src/      # entities/（User 等实体）、seed
├── handlers/src/    # LoginHandler, DocService (impl IDocumentService), ...
└── host/src/        # Host::builder() 组合根
    ├── main.rs
    └── startup/     # mod.rs、hosted/（DbInitService 等）、seed/
```

## 小结

分层是**职责边界**：contracts 定义「对外承诺什么」，handlers 定义「如何履约」，domain 定义「数据是什么」。面向接口，关注契约，不关注实现。

下一节：[编译时扫描机制](compile-time-scan.md)
