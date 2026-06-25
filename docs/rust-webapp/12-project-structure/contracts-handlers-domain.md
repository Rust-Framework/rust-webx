# Contracts / Handlers / Domain 分层

## contracts — API 契约

```rust
// contracts/auth.rs — 只定义「对外承诺什么」

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

#[post("/api/auth/login")]
impl IRequest<AuthResponse> for LoginRequest {}
```

特点：
- 一个文件可包含多个相关 Request
- 是 OpenAPI 生成的数据源
- 团队讨论 API 设计时只需看 contracts

## handlers — 用例实现

```rust
// handlers/auth.rs — 只定义「如何履约」

#[inject_attr(singleton, as = dyn IRequestHandler<LoginRequest, AuthResponse>)]
pub struct LoginHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<LoginRequest, AuthResponse> for LoginHandler {
    async fn handle(&self, req: LoginRequest) -> Result<AuthResponse> {
        // 验证凭据 → 签发 Token → 返回
    }
}
```

特点：
- 通过 DI 获取依赖
- 返回 `Result<T>`，不直接操作 HTTP
- 可独立单元测试

## domain — 领域模型

```rust
// domain/user.rs — 只定义「业务是什么」

pub struct UserEntity { ... }  // 数据库映射
pub struct UserModel { ... }   // 业务模型

impl UserModel {
    pub fn from_entity(e: &UserEntity) -> Self { ... }
}
```

特点：
- 不依赖 rust-webapp 框架类型
- 包含数据库迁移
- 最稳定、变更最少

## 数据流

```
HTTP Request
    → contracts (Request struct 反序列化)
    → handlers (业务编排)
    → services (领域逻辑)
    → domain (实体操作)
    → handlers (组装 Response DTO)
    → contracts (Response DTO 序列化)
    → HTTP Response
```

## 小结

三层分离让 API 契约、业务逻辑和领域模型各自独立演化。

下一节：[测试策略](testing-strategy.md)
