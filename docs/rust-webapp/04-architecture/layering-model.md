# 分层模型与依赖方向

## 应用内部分层

rust-webapp 推荐的应用分层模型：

```
┌─────────────────────────────────────────┐
│  main.rs / startup.rs    启动与组合根    │
├─────────────────────────────────────────┤
│  handlers/               应用层（用例）   │
├─────────────────────────────────────────┤
│  services/               领域服务层       │
├─────────────────────────────────────────┤
│  domain/                 领域模型层       │
├─────────────────────────────────────────┤
│  contracts/              接口契约层       │
└─────────────────────────────────────────┘
```

## 各层职责

### contracts — 契约层

**拥有**：API 路由声明、请求/响应 DTO、授权元数据

```rust
// contracts/auth.rs
#[post("/api/auth/login")]
impl IRequest<AuthResponse> for LoginRequest {}
```

- 不含业务逻辑
- 可序列化类型定义
- 是对外 API 的「说明书」

### handlers — 应用层

**拥有**：用例编排、调用领域服务、返回 Result

```rust
impl IRequestHandler<LoginRequest, AuthResponse> for LoginHandler {
    async fn handle(&self, req: LoginRequest) -> Result<AuthResponse> {
        let user = self.verify_credentials(&req).await?;
        let token = self.create_token(&user)?;
        Ok(AuthResponse { token, user })
    }
}
```

- 薄 Handler 原则：复杂逻辑下沉到 services
- 通过 DI 获取依赖，不 `new` 具体实现

### services — 领域服务层

**拥有**：跨实体业务规则、可复用逻辑

```rust
// services/docs.rs
impl DocService {
    pub fn index(&self, work: &str) -> Result<DocIndex, String> { ... }
}
```

- 不感知 HTTP
- 可被多个 Handler 复用
- 可被 `IHostedService` 调用

### domain — 领域模型层

**拥有**：实体、值对象、数据库迁移

```rust
// domain/user.rs
pub struct UserEntity { ... }
pub struct UserModel { ... }
```

- 最稳定的一层
- 不依赖框架类型（除 serde）

### main.rs / startup.rs — 组合根

**拥有**：DI 注册、Host 配置、后台服务

- 唯一允许「组装」具体实现的地方
- `startup.rs` 中的 `IHostedService` 负责初始化

## 依赖方向

```
handlers → services → domain
   ↓
contracts（handlers 和 contracts 可互相引用类型）
   ↓
rust_webapp（框架）
```

**禁止**：
- domain → handlers（领域不依赖用例）
- services → handlers（服务不依赖 HTTP）
- contracts → handlers（契约不依赖实现）

## Docbit 实例

```
docbit/src/
├── contracts/     # LoginRequest, GetDocIndexRequest, ...
├── handlers/      # LoginHandler, GetDocIndexHandler, ...
├── services/      # DocService, SiteService
├── domain/        # UserEntity, BlogPostEntity, migrations/
├── startup.rs     # DbInitService (IHostedService)
└── main.rs        # Host::builder() 组合根
```

## 小结

分层不是教条，而是**职责边界**。contracts 定义「对外承诺什么」，handlers 定义「如何履约」，services/domain 定义「业务是什么」。

下一节：[编译时扫描机制](compile-time-scan.md)
