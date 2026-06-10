# LRWF 快速开始指南

本指南从零开始，带你搭建一个完整的 LRWF WebApi 项目。

## 第一步：创建项目

```bash
cargo new my-lrwf-app
cd my-lrwf-app
```

## 第二步：添加依赖

编辑 `Cargo.toml`：

```toml
[package]
name = "my-lrwf-app"
version = "0.1.0"
edition = "2021"

[dependencies]
lrwf = { path = "../lrwf/crates/lrwf" }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

> **注意：** 本地开发时使用 `path` 依赖。发布到 crates.io 后使用 `lrwf = "0.1"`。

## 第三步：编写 Hello World

在 `src/main.rs` 中：

```rust
use lrwf::*;

// 1. 定义 Request（承载输入参数）
struct HelloRequest;

// 2. 标注路由 + 声明响应类型
//    #[get] 是过程宏，将 RouteEntry 提交到编译时 inventory
//    impl IRequest<String> 声明响应类型为 String
#[get("/hello")]
impl IRequest<String> for HelloRequest {}

// 3. 实现 Handler
//    #[derive(Default)] 配合 #[handler] 实现编译时自动注册
#[derive(Default)]
struct HelloHandler;

#[handler]
#[async_trait]
impl IRequestHandler<HelloRequest, String> for HelloHandler {
    async fn handle(&self, _req: HelloRequest) -> Result<String> {
        Ok("Hello, World! Welcome to LRWF.".to_string())
    }
}

// 4. 启动服务
#[tokio::main]
async fn main() {
    println!("Server starting at http://0.0.0.0:5000");
    
    Host::builder()
        .build()
        .run("0.0.0.0:5000")
        .await
        .expect("Server failed");
}
```

## 第四步：运行和测试

```bash
cargo run
```

测试端点：

```bash
curl http://localhost:5000/hello
# 输出: "Hello, World! Welcome to LRWF."
```

## 第五步：添加更多端点

### GET 带路径参数

```rust
struct GetUserRequest {
    id: String,
}

#[get("/api/users/{id}")]
impl IRequest<UserModel> for GetUserRequest {}

#[derive(Default)]
struct GetUserHandler;

#[handler]
#[async_trait]
impl IRequestHandler<GetUserRequest, UserModel> for GetUserHandler {
    async fn handle(&self, req: GetUserRequest) -> Result<UserModel> {
        // 从数据库查找用户...
        Ok(UserModel { id: req.id, name: "Alice".into(), email: "alice@example.com".into() })
    }
}
```

### POST 带 JSON body

```rust
#[derive(serde::Deserialize)]
struct CreateUserRequest {
    name: String,
    email: String,
}

#[post("/api/users")]
impl IRequest<UserModel> for CreateUserRequest {}

#[derive(Default)]
struct CreateUserHandler;

#[handler]
#[async_trait]
impl IRequestHandler<CreateUserRequest, UserModel> for CreateUserHandler {
    async fn handle(&self, req: CreateUserRequest) -> Result<UserModel> {
        Ok(UserModel {
            id: "new-id".into(),
            name: req.name,
            email: req.email,
        })
    }
}
```

### DELETE 返回空（204）

```rust
struct DeleteUserRequest {
    id: String,
}

#[delete("/api/users/{id}")]
impl IRequest<()> for DeleteUserRequest {}

#[derive(Default)]
struct DeleteUserHandler;

#[handler]
#[async_trait]
impl IRequestHandler<DeleteUserRequest, ()> for DeleteUserHandler {
    async fn handle(&self, _req: DeleteUserRequest) -> Result<()> {
        // 删除用户...
        Ok(())
    }
}
```

## 第六步：添加中间件

```rust
#[derive(Default)]
struct LoggingMiddleware;

#[async_trait]
impl IMiddleware for LoggingMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        let start = std::time::Instant::now();
        let method = ctx.request().method().to_string();
        let path = ctx.request().path().to_string();
        
        println!("→ {} {}", method, path);
        
        // 注意：当前中间件为顺序模型，无法在 response 后执行
        // 后续版本将支持洋葱模型
        
        Ok(())
    }
}

// 注册中间件
.register(|svc| svc.add_middleware::<LoggingMiddleware>())
```

## 第七步：使用 #[handler] 零配置注册

`#[handler]` 是框架内置功能——放在 Handler 的 impl 块上，编译时自动注册到 DI 容器。
路由扫描同样内置，无需手动调用 `add_request_endpoints()`。

零配置代码示例：

```rust
#[derive(Default)]
struct PingHandler;

#[handler]
#[async_trait]
impl IRequestHandler<PingRequest, String> for PingHandler {
    async fn handle(&self, _req: PingRequest) -> Result<String> {
        Ok("pong".to_string())
    }
}
```

`main()` 函数无需 `register` 或 `configure`：

```rust
#[tokio::main]
async fn main() {
    Host::builder()
        .build()
        .run("0.0.0.0:5000")
        .await?;
}
```

> **限制：** `#[handler]` 要求 handler struct 实现 `Default`。有注入依赖的 handler 见第八步。

## 第八步：添加数据库和依赖注入

```rust
struct UserRepository {
    db_pool: Arc<MyDbPool>,
}

impl UserRepository {
    fn new(pool: Arc<MyDbPool>) -> Self { Self { db_pool: pool } }
}

struct GetUserHandler {
    repo: Arc<UserRepository>,
}

#[async_trait]
impl IRequestHandler<GetUserRequest, UserModel> for GetUserHandler {
    async fn handle(&self, req: GetUserRequest) -> Result<UserModel> {
        self.repo.get(&req.id)
            .ok_or_else(|| Error::NotFound(format!("User not found: {}", req.id)))
    }
}

// 启动时手动注册带依赖的 handler
#[tokio::main]
async fn main() {
    let db_pool = Arc::new(MyDbPool::connect("postgres://...").await.unwrap());
    
    Host::builder()
        .register(move |svc| {
            let repo = Arc::new(UserRepository::new(Arc::clone(&db_pool)));
            svc.singleton::<dyn IRequestHandler<GetUserRequest, UserModel>>(
                move |_| Arc::new(GetUserHandler { repo: Arc::clone(&repo) })
            )
        })
        .build()
        .run("0.0.0.0:5000")
        .await
        .expect("Server failed");
}
```

## 第九步：添加事件系统

```rust
// 定义事件
#[derive(Clone)]
struct UserCreatedEvent {
    user_id: String,
    name: String,
}

impl IEventRequest for UserCreatedEvent {}

// 定义事件处理器
struct SendWelcomeEmailHandler {
    email_service: Arc<dyn EmailService>,
}

#[async_trait]
impl IEventHandler<UserCreatedEvent> for SendWelcomeEmailHandler {
    async fn handle(&self, event: UserCreatedEvent) -> Result<()> {
        self.email_service.send_welcome(&event.user_id, &event.name).await
    }
}

// 在 CreateUserHandler 中注入 Mediator 并发布事件
struct CreateUserHandler {
    repo: Arc<UserRepository>,
    mediator: Arc<Mediator>,
}

#[async_trait]
impl IRequestHandler<CreateUserRequest, UserModel> for CreateUserHandler {
    async fn handle(&self, req: CreateUserRequest) -> Result<UserModel> {
        let user = self.repo.create(&req.name, &req.email)?;
        
        // 发布事件
        self.mediator.publish(UserCreatedEvent {
            user_id: user.id.clone(),
            name: user.name.clone(),
        }).await?;
        
        Ok(user)
    }
}

// 注册事件处理器
.register(move |svc| {
    svc.singleton::<dyn IEventHandler<UserCreatedEvent>>(
        |_| Arc::new(SendWelcomeEmailHandler { email_service: ... })
    )
})
```

## 完整项目结构

```
my-lrwf-app/
├── Cargo.toml
└── src/
    ├── main.rs            # 服务启动
    ├── requests/          # 请求定义
    │   ├── mod.rs
    │   ├── hello.rs       # HelloRequest + #[get("/hello")]
    │   └── user.rs        # User CRUD requests
    ├── handlers/          # 请求处理器
    │   ├── mod.rs
    │   ├── hello.rs       # HelloHandler
    │   └── user.rs        # User CRUD handlers
    ├── middleware/         # 中间件
    │   ├── mod.rs
    │   └── logging.rs     # LoggingMiddleware
    ├── events/            # 事件定义
    │   ├── mod.rs
    │   └── user.rs        # UserCreatedEvent
    └── event_handlers/    # 事件处理器
        ├── mod.rs
        └── email.rs       # SendWelcomeEmailHandler
```

## 常见问题

### Q: 如何返回 404？

```rust
async fn handle(&self, req: GetUserRequest) -> Result<UserModel> {
    self.repo.get(&req.id)
        .ok_or_else(|| Error::NotFound(format!("User not found: {}", req.id)))
}
```

### Q: 如何返回 400 验证错误？

```rust
async fn handle(&self, req: CreateUserRequest) -> Result<UserModel> {
    if req.name.is_empty() {
        return Err(Error::Validation("name is required".into()));
    }
    // ...
}
```

### Q: 如何处理路径参数？

```rust
// URL: /api/users/{id}
// 当前版本需手动从 route_params 提取（后续版本将支持自动绑定）
// 示例中使用 struct 字段承载，运行时绑定将在后续版本完善
```

### Q: 如何设置自定义响应头？

```rust
async fn handle(&self, req: MyRequest) -> Result<MyResponse> {
    // 在 handler 中无法直接访问 HttpContext
    // 当前版本通过 IRequest<T> 框架自动序列化
    // 自定义头需通过 IMiddleware 在管道中设置
    Ok(response)
}
```
