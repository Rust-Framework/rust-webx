# 从 Axum / Actix 迁移

## 思维模型转变

| Axum/Actix 思维 | rust-webx 思维 |
|----------------|-----------------|
| 路由是中心 | Request 是中心 |
| 手动组装一切 | 框架提供应用骨架 |
| 函数即处理器 | Handler struct + trait |
| 自由风格 | 约定分层 |

## 路由迁移

### Axum

```rust
Router::new()
    .route("/api/users/:id", get(get_user))
    .route("/api/users", post(create_user))
```

### rust-webx

```rust
// 无需中央路由表，每个端点自声明
#[get("/api/users/{id}")]
impl IRequest<UserDto> for GetUserRequest {}

#[post("/api/users")]
impl IRequest<UserDto> for CreateUserRequest {}
```

## 中间件迁移

### Axum

```rust
let app = Router::new()
    .layer(TraceLayer::new_for_http())
    .layer(CorsLayer::permissive());
```

### rust-webx

```rust
Host::builder()
    .use_cors(CorsConfig::default())
    // RequestTracing 自动启用
    .build()
```

自定义 Axum middleware 需改写为 `impl IMiddleware`。

## 状态共享

### Axum

```rust
#[derive(Clone)]
struct AppState {
    db: Arc<DbPool>,
}
```

### rust-webx

```rust
// 通过 DI 容器管理，无需 AppState
#[inject_attr(singleton)]
pub struct GetUserHandler {
    db: Arc<DbPool>,
}
```

## 错误处理

### Axum

```rust
async fn get_user(id: String) -> Result<Json<User>, AppError> {
    ...
}
```

### rust-webx

```rust
async fn handle(&self, req: GetUserRequest) -> Result<UserDto> {
    // Error 自动映射 HTTP 状态码
}
```

## 何时保留 Axum

- 已有大量 Axum 代码且运行良好
- 需要 Axum 生态特定功能（如 axum-extra）
- 极简微服务，不需要 DI/Mediator

## 渐进式迁移策略

1. 新端点用 rust-webx 编写
2. 旧 Axum 路由通过反向代理共存
3. 逐步将旧端点改写为 Request + Handler
4. 最终统一为 rust-webx Host

## 小结

从 Axum 迁移的核心是接受「约定」换取「工程化」，减少自行拼装的工作量。

下一节：[概念对照表](concept-mapping.md)
