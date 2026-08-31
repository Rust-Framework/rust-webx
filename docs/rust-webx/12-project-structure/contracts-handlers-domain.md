# Contracts / Handlers / Domain 分层

基于 rust-webx 的业务应用采用**契约驱动、面向接口**的三层结构。关注契约，不关注实现。

## contracts — API 与业务接口契约

**仅依赖框架**（`rust_webx`），**禁止依赖 domain 或 handlers**。

```rust
// contracts/auth.rs — 定义「对外承诺什么」

use rust_webx::*;

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: UserView,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
pub enum UserRole {
    Admin,
    User,
}

/// 业务服务接口 — 与 Request 同级，同属契约层
pub trait IAuthService: Send + Sync {
    fn login(&self, email: &str, password: &str) -> Result<AuthResponse, String>;
}

#[post("/api/auth/login")]
impl IRequest<AuthResponse> for LoginRequest {}
```

特点：

- Request、Response DTO、共享 enum、`I…Service` trait 均在此层
- 路由宏、`#[authorize]` 等元数据在此声明
- 是 OpenAPI 生成的数据源；团队讨论 API 时**只看 contracts**
- **不含** `async fn` 业务实现、数据库访问

## handlers — Handler 与 Service 实现

**履约层**：实现 `IRequestHandler` 与 `I…Service`。

```rust
// handlers/auth.rs — 定义「如何履约」

use crate::contracts::auth::{IAuthService, LoginRequest, AuthResponse};
use crate::domain::user::UserEntity;

#[inject]
pub struct AuthService {
    ctx: Arc<Mutex<DbContext>>,
}

impl IAuthService for AuthService {
    fn login(&self, email: &str, password: &str) -> Result<AuthResponse, String> {
        // 读 UserEntity → 组装 AuthResponse（contracts DTO）
    }
}

#[inject]
pub struct LoginHandler {
    auth: Arc<dyn IAuthService>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<LoginRequest, AuthResponse> for LoginHandler {
    async fn handle(&self, req: LoginRequest) -> Result<AuthResponse> {
        self.auth.login(&req.email, &req.password)
            .map_err(|e| Error::Validation(e))
    }
}
```

特点：

- Handler 薄编排；复杂逻辑在 Service 实现中
- Handler **只注入** `Arc<dyn I…Service>`
- `#[inject]` + `#[handler(inject)]` 自动注册，无需改 `main.rs`
- 返回 `Result<T>`，不直接操作 HTTP

## domain — 持久化实体与迁移

```rust
// domain/user.rs — 定义「数据是什么」

use crate::contracts::auth::UserRole;  // 可复用 contracts 枚举

pub struct UserEntity {
    pub id: String,
    pub role: UserRole,
    // ...
}
```

特点：

- 数据库实体、EF 配置、迁移
- **可以**引用 contracts 复用枚举或共享 model
- **禁止**依赖框架类型（`serde` 除外）
- **禁止**引用 handlers

## 数据流

```
HTTP Request
    → contracts (Request 反序列化)
    → handlers (Handler 编排)
    → Arc<dyn I…Service> (handlers 内实现)
    → domain (实体读写)
    → contracts (Response DTO 序列化)
    → HTTP Response
```

## 依赖规则速查

| 从 → 到 | contracts | handlers | domain |
|---------|-----------|----------|--------|
| contracts | — | ❌ | ❌ |
| handlers | ✅ | — | ✅ |
| domain | ✅（复用 enum/model） | ❌ | — |

## 反模式

- `contracts` 中 `use crate::domain::*` → DTO 应在 contracts，映射在 handlers
- 独立 `services/` 目录 → 接口归 contracts，实现归 handlers
- Handler 依赖 `Arc<AuthService>` → 应使用 `Arc<dyn IAuthService>`

## 小结

contracts 定义「承诺什么」，handlers 定义「如何履约」，domain 定义「数据是什么」。面向接口开发，让 API、实现与持久化各自独立演化。

下一节：[测试策略](testing-strategy.md)
