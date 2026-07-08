---
name: lrwf-skill
description: >
  使用 LRWF（Rust WebApi Framework）构建 ASP.NET Core 风格的 Rust Web 服务。
  涵盖 IRequest 直接路由、#[get]/#[post] 快捷键宏、IRequestHandler 编译时自动注册、
  中间件管道、IMediator 中介者模式、IPipelineBehavior 请求拦截器链、
  IEventHandler 事件发布/订阅、IHostedService 后台服务/数据初始化、
  编译时 inventory 路由收集、结构化异常处理、
  JWT Bearer Token 身份认证、基于资源（路由模式）的角色/权限授权、
  业务应用标准分层（contracts/handlers/domain，面向接口 I…Service）。
  当用户需要在 Rust 中构建 WebApi、使用 DI+中介者模式、设计模块化后端架构
  或从 ASP.NET Core 迁移到 Rust 时使用此技能。
---

# LRWF 技能 · Agent 操作指令

你是 LRWF（Rust WebApi Framework）专家。本文件告诉你如何使用 LRWF 解决用户问题。
详细 API 签名、架构原理、完整示例存放在本目录的支持文件中——在需要时加载它们。

---

## 何时激活此技能

当用户请求涉及以下任一场景时，加载此技能：

- 在 Rust 项目中构建 WebApi 服务
- 使用 `IRequest<T>` 泛型请求模式 + `#[get]/#[post]` 快捷键宏定义端点
- 使用 `IRequestHandler<T, R>` 双类型参数处理器模式
- 使用 `IMediator` 中介者模式调度请求和发布事件
- 使用 `IMiddleware` 实现 HTTP 请求中间件
- 使用 `IPipelineBehavior` 实现 Mediator 管道拦截器
- 使用 `IEventHandler` 实现事件发布/订阅
- 使用 `IHostedService` 实现后台服务（数据初始化、定时任务、连接池预热）
- 使用 `#[handler]` 编译时自动注册或 `register_handlers!` 手动注册
- 使用 `Error::status_code()` 映射异常到 HTTP 状态码
- 使用 `JwtAuth` + `jwt_middleware` 实现 JWT Bearer Token 身份认证
- 使用 `ResourceAuthorization` + `resource_auth_middleware` 实现基于资源的角色/权限授权
- 从 ASP.NET Core / MediatR 架构迁移到 Rust

**不激活的场景：** 不使用 LRWF 的纯 Rust 项目、极简脚本、不涉及 WebApi 的系统编程。

---

## 支持文件索引

在回答问题前，根据问题类型加载相应的支持文件获取详细知识：

| 文件 | 加载条件 | 内容 |
|------|---------|------|
| `reference/api.md` | 用户询问具体 API 签名、宏用法、类型定义 | 所有公开 API 的完整签名、使用示例 |
| `guides/quickstart.md` | 用户问"怎么开始"、"写一个 Hello World"、"搭建项目" | 从零到运行的最小化项目完整步骤 |
| `guides/project-structure.md` | 用户问项目结构、分层、contracts/handlers/domain、面向接口开发 | 业务应用标准目录、依赖规则、反模式 |

---

## 核心工作流程

### 当用户要求"用 LRWF 搭建 WebApi 项目"

**先加载 `guides/project-structure.md`**，按标准目录创建模块：

```
src/
├── main.rs
├── contracts/     # Request/Response/enum/I…Service — 仅依赖框架
├── handlers/      # Handler + Service 实现 — inject_attr 自动注册
└── domain/        # 实体 + 迁移 — 可引用 contracts
```

`appsettings.json` 放在项目根目录。

1. 在 `Cargo.toml` 添加依赖：`lrwf = "0.1"`、`tokio`、`serde`
2. 在 `main.rs` 添加 `use lrwf::*;`
3. 按以下四步定义端点（Request 定义在 `contracts/`，Handler 定义在 `handlers/`）：
   - 定义 Request 结构体（承载输入参数）
   - 用 `#[get("/path")]`（或 `#[post]`/`#[put]`/`#[delete]`）标注 `impl IRequest<TResponse> for Request`
   - 实现 handler：`#[derive(Default)]` + `#[async_trait] impl IRequestHandler<Req, Rsp> for Handler`
   - 启动：`Host::builder().register(...).configure(...).build().run("0.0.0.0:5000").await`

### 当用户要定义新端点

```
定义 Request struct
    │
    ├── 简单请求（无参数）→ struct MyRequest;
    ├── 带路径参数       → struct MyRequest { id: String }            （路径从 route_params 提取）
    └── 带 JSON body     → #[derive(serde::Deserialize)] struct MyRequest { ... }
         │
         ▼
    #[get("/path")] 或 #[post("/path")] 等标注
    impl IRequest<TResponse> for MyRequest {}
         │
         ▼
    #[derive(Default)]
    struct MyHandler;
    
    #[async_trait]
    impl IRequestHandler<MyRequest, TResponse> for MyHandler {
        async fn handle(&self, req: MyRequest) -> Result<TResponse> { ... }
    }
         │
         ▼
    注册：register_handlers!(svc, MyRequest => TResponse => MyHandler)
    或使用 #[handler] 属性实现编译时自动注册
```

### 当用户要注册 Handler

**方式一：`register_handlers!` 宏（适合 Default handler）**
```rust
.register(|svc| {
    register_handlers!(svc,
        HelloRequest => String => HelloHandler,
        ListUsersRequest => Vec<UserModel> => ListUsersHandler,
    )
})
```

**方式二：手动注册（handler 有注入依赖）**
```rust
.register(move |svc| {
    let repo = Arc::new(MyRepository::new());
    svc.singleton::<dyn IRequestHandler<GetUserRequest, UserModel>>(
        move |_| Arc::new(GetUserHandler { repo: Arc::clone(&repo) })
    )
})
```

**方式三：`#[handler]` 编译时自动注册（handler 实现 Default）**
```rust
#[derive(Default)]
struct HelloHandler;

#[handler]  // 自动注册到 inventory，Host::build() 时生效
#[async_trait]
impl IRequestHandler<HelloRequest, String> for HelloHandler {
    async fn handle(&self, _req: HelloRequest) -> Result<String> { ... }
}
```

### 当用户要添加中间件

```rust
#[derive(Default)]
struct LoggingMiddleware;

#[async_trait]
impl IMiddleware for LoggingMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        println!("[{}] {}", ctx.request().method(), ctx.request().path());
        Ok(())
    }
}

// 注册
.register(|svc| {
    svc.add_middleware::<LoggingMiddleware>()
})
```

### 当用户要处理异常

所有 handler 返回 `Result<T>`。框架内置异常中间件会捕获错误并自动映射：

| Error 变体 | HTTP 状态码 |
|-----------|-----------|
| `Error::NotFound(msg)` | 404 |
| `Error::Validation(msg)` | 400 |
| `Error::Serialization(e)` | 400 |
| `Error::Http(msg)` | 400 |
| `Error::Di(msg)` | 500 |
| `Error::Internal(msg)` | 500 |
| `Error::Message(msg)` | 500 |
| `Error::Routing(msg)` | 404 |

响应格式为 JSON：`{"error": "message", "status": code}`

### 当用户要使用 IHostedService（后台服务 / 数据初始化）

`IHostedService` 是 ASP.NET Core 风格的背景服务接口，
在 `Host::run()` 调用时自动启动（在 HTTP 监听器启动之前），
并在服务关闭时自动停止。

适用于：
- **数据库迁移和数据初始化**（替代在 `main()` 中显式调用初始化函数）
- **后台轮询 / 队列消费者**
- **连接池预热**
- **预计算缓存数据**

```rust
use lrwf::*;

#[derive(Default)]
struct DbInitService;

#[async_trait]
impl IHostedService for DbInitService {
    async fn start(&self) -> Result<()> {
        tracing::info!("Running migrations...");
        run_migrations().await?;
        tracing::info!("Seeding data...");
        seed_data().await?;
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        tracing::info!("Shutting down...");
        Ok(())
    }
}

// 注册到 DI 容器
Host::builder()
    .register(|svc| {
        svc.add_hosted_service::<DbInitService>()
    })
    .build()
    .run()
    .await?;
```

`stop()` 有默认空实现，如果不需要关闭逻辑可以省略。

多个 hosted service 会按注册顺序依次启动，关闭时反向停止。

### 当用户要使用事件系统（发布/订阅）

```rust
// 1. 定义事件
#[derive(Clone)]
struct UserCreatedEvent { user_id: String, name: String }
impl IEventRequest for UserCreatedEvent {}

// 2. 定义事件处理器
struct SendWelcomeEmailHandler;
#[async_trait]
impl IEventHandler<UserCreatedEvent> for SendWelcomeEmailHandler {
    async fn handle(&self, event: UserCreatedEvent) -> Result<()> {
        println!("Sending welcome email to {}", event.name);
        Ok(())
    }
}

// 3. 在 handler 中通过 Mediator 发布事件
#[async_trait]
impl IRequestHandler<CreateUserRequest, UserModel> for CreateUserHandler {
    async fn handle(&self, req: CreateUserRequest) -> Result<UserModel> {
        let user = self.repo.create(&req.name, &req.email)?;
        // 发布事件到所有 IEventHandler<UserCreatedEvent>
        self.mediator.publish(UserCreatedEvent {
            user_id: user.id.clone(),
            name: user.name.clone(),
        }).await?;
        Ok(user)
    }
}
```

---

## 必须遵守的规则

### 规则 0：项目分层与依赖方向（业务应用）

加载 `guides/project-structure.md` 获取完整说明。核心约束：

| 层 | 拥有 | 依赖 |
|----|------|------|
| `contracts/` | Request、Response DTO、enum、`I…Service` trait、路由宏 | **仅**框架（`lrwf` / `rust_webx`） |
| `handlers/` | `IRequestHandler` 实现、`I…Service` 实现 | contracts、domain、基础设施 |
| `domain/` | 实体、迁移、EF 配置 | contracts（复用枚举/model）、**禁止** handlers / 框架 |
| `main.rs` | Host 配置、`bootstrap` 基础设施注册 | 不做业务 Service 手动注册 |

- **面向接口**：Handler 注入 `Arc<dyn IBlogService>`，禁止 `Arc<BlogService>`
- **接口在 contracts，实现在 handlers**：禁止独立 `services/` 目录
- **contracts 禁止引用 domain**：DTO 属于契约层；domain 实体映射在 handlers 的 Service 实现中完成
- 新增业务能力：**先写 trait + Request → 写实现 + Handler**，`main.rs` 通常无需改动

### 规则 1：请求和响应类型绑定一致

```rust
// ✅ 正确：IRequest<TResponse> 的泛型参数 = IRequestHandler<T, R> 的第二个参数
#[get("/users/{id}")]
impl IRequest<UserModel> for GetUserRequest {}

impl IRequestHandler<GetUserRequest, UserModel> for GetUserHandler { ... }

// register_handlers! 中三要素：Request => ResponseType => HandlerType
register_handlers!(svc,
    GetUserRequest => UserModel => GetUserHandler,
)

// ❌ 错误：响应类型不匹配
impl IRequest<String> for GetUserRequest {}         // 声明响应为 String
impl IRequestHandler<GetUserRequest, UserModel> for GetUserHandler { ... }  // 处理器返回 UserModel
```

### 规则 2：Handler 的 DI 注册类型 = 解析类型

```rust
// ✅ 正确：注册为 dyn IRequestHandler<T, R>
svc.singleton::<dyn IRequestHandler<HelloRequest, String>>(
    |_| Arc::new(HelloHandler::default())
)

// ❌ 错误：注册为具体类型，框架无法通过 dyn trait 解析
svc.singleton(|_| Arc::new(HelloHandler::default()))
```

### 规则 3：`#[handler]` 要求 Handler 实现 Default

```rust
// ✅ 正确：handler 有 Default
#[derive(Default)]
struct HelloHandler;

#[handler]
#[async_trait]
impl IRequestHandler<HelloRequest, String> for HelloHandler { ... }

// ❌ 错误：handler 没有 Default，#[handler] 编译失败
struct GetUserHandler { repo: Arc<MyRepo> }  // 无 Default

// 改用 register_handlers! 或手动注册
```

### 规则 4：`#[get]` 等快捷键标注在 `impl IRequest<T>` 块上

```rust
// ✅ 正确：标注在 impl IRequest<T> 上
#[get("/hello")]
impl IRequest<String> for HelloRequest {}

// ❌ 错误：标注在 struct 上
#[get("/hello")]
struct HelloRequest;
```

---

## 常见任务模板

### 模板 1：最小化 Hello World

```rust
use lrwf::*;

struct HelloRequest;

#[get("/hello")]
impl IRequest<String> for HelloRequest {}

#[derive(Default)]
struct HelloHandler;

#[handler]
#[async_trait]
impl IRequestHandler<HelloRequest, String> for HelloHandler {
    async fn handle(&self, _req: HelloRequest) -> Result<String> {
        Ok("Hello, World!".to_string())
    }
}

#[tokio::main]
async fn main() {
    Host::builder()
        .build()
        .run("0.0.0.0:5000")
        .await
        .expect("Server failed");
}
```

### 模板 2：带路径参数的 GET 端点

```rust
use lrwf::*;

struct GetUserRequest {
    id: String,
}

#[get("/api/users/{id}")]
impl IRequest<String> for GetUserRequest {}

#[derive(Default)]
struct GetUserHandler;

#[handler]
#[async_trait]
impl IRequestHandler<GetUserRequest, String> for GetUserHandler {
    async fn handle(&self, req: GetUserRequest) -> Result<String> {
        Ok(format!("User: {}", req.id))
    }
}

#[tokio::main]
async fn main() {
    Host::builder()
        .build()
        .run("0.0.0.0:5000")
        .await
        .expect("Server failed");
}
```

### 模板 3：带依赖注入的 Handler

```rust
struct GetUserHandler { repo: Arc<MyRepository> }

#[async_trait]
impl IRequestHandler<GetUserRequest, UserModel> for GetUserHandler {
    async fn handle(&self, req: GetUserRequest) -> Result<UserModel> {
        self.repo.get(&req.id)
            .ok_or_else(|| Error::NotFound(format!("Not found: {}", req.id)))
    }
}

// 启动时手动注册（路由扫描和Handler注册已内置，无需额外调用）
Host::builder()
    .register(move |svc| {
        let repo = Arc::new(MyRepository::new());
        svc.singleton::<dyn IRequestHandler<GetUserRequest, UserModel>>(
            move |_| Arc::new(GetUserHandler { repo: Arc::clone(&repo) })
        )
    })
    .build()
    .run("0.0.0.0:5000").await?;
```

---

## 常见错误排查

当用户遇到错误时，按以下顺序排查：

1. **"No handler registered for request"** → 路由扫描和 Handler 注册已内置，检查 handler 是否正确实现了 `Default` 或已手动注册到 DI 容器（Rule 2）
2. **"route not found" (404)** → 检查 `#[get("/path")]` 是否标注在 `impl IRequest<T>` 上（Rule 4），路径是否匹配
3. **`#[handler]` 编译失败** → 检查 handler struct 是否实现了 `Default`（Rule 3）
4. **类型不匹配 panic** → 检查 `IRequest<T>` 的泛型参数是否与 `IRequestHandler<T, R>` 的第二个泛型参数一致（Rule 1）

---

## 已知限制与后续计划

| 限制 | 说明 | 后续计划 |
|------|------|---------|
| IMiddleware 无 next 闭包 | 顺序调用，无法 post-process | 待 Rust async closure 稳定后升级洋葱模型 |
| IMediator 非 dyn-compatible | 泛型 send/publish 方法 | 用户使用具体 Arc\<Mediator\>，或提供 type-erased wrapper |
| lrwf::request! stub | 未实现完整的 DSL 解析 | 后续版本实现 struct + IRequest + route 一键生成 |
| 参数绑定 pass-through | `#[from_body]` 等仅作为 metadata | 后续版本实现运行时 request → struct 自动反序列化 |
| IPipelineBehavior 异步泛型链 | 管道链的类型擦除复杂 | 后续版本通过 downcast 或 type-map 实现 |

---

## 身份认证和授权

### JWT Bearer Token 认证

```rust
use lrwf::*;
use lrwf_http::auth_jwt::{JwtAuth, jwt_middleware};
use jsonwebtoken::{DecodingKey, Validation};
use std::sync::Arc;

// 创建 JWT 认证处理器
let auth = Arc::new(JwtAuth::new(
    DecodingKey::from_secret(b"my-secret-key"),
    Validation::default(),
));

// 注册认证中间件
Host::builder()
    .register(move |svc| {
        svc.add_middleware_instance(jwt_middleware(Arc::clone(&auth)))
    })
    .build()
    .run("0.0.0.0:5000").await?;
```

认证成功后，`JwtClaims`（实现 `IClaims`）会存储在 `IHttpContext` 中。Claims 包含 `sub`（用户标识）、`roles`、`permissions` 和原始 claims map。

```rust
// 在 handler 中访问 claims（通过从 DI 解析的 IHttpContext）
let claims = ctx.claims().unwrap();
println!("User: {}", claims.subject());
println!("Roles: {:?}", claims.roles());
```

### 基于资源的角色/权限授权

```rust
use lrwf_http::authz::{ResourceAuthorization, resource_auth_middleware};

// 定义授权策略：将路由模式映射到角色和权限
let policy = Arc::new(ResourceAuthorization::new()
    .allow_role("/api/admin/**", "admin")
    .allow_role("/api/users/{id}", "user")
    .allow_permission("/api/settings", "settings:write")
);

// 注册授权中间件（必须在认证中间件之后）
Host::builder()
    .register(move |svc| {
        let auth = Arc::new(JwtAuth::new(/* ... */));
        let policy = Arc::new(ResourceAuthorization::new()
            .allow_role("/api/admin/**", "admin"));
        svc.add_middleware_instance(jwt_middleware(auth))
           .add_middleware_instance(resource_auth_middleware(policy))
    })
    .build()
    .run("0.0.0.0:5000").await?;
```

`ResourceAuthorization` 使用 `route_pattern()`（路由匹配后自动设置的原始路由字符串，如 `"/api/users/{id}"`）作为资源键，与用户 claims 中的 roles/permissions 进行匹配。

### 在 Handler 中使用 IClaimsExt

```rust
#[async_trait]
impl IRequestHandler<MyRequest, String> for MyHandler {
    async fn handle(&self, req: MyRequest) -> Result<String> {
        // 通过 DI 注入 IHttpContext 或从 Mediator 中获取
        // IHttpContext 实现了 IClaimsExt，可直接调用 set_claims / claims
        Ok("ok".to_string())
    }
}
```

---

## 处理流程总结

```
用户问题 → 判断场景 → 加载对应支持文件 → 理解 API/模式 → 生成代码 → 验证
    │
    ├── "怎么搭建项目？"   → 加载 guides/quickstart.md → 四步创建端点 → 启动服务
    ├── "怎么定义端点？"   → 查看核心工作流程 → 选 Request 结构 → 选路由宏 → 写 Handler
    ├── "怎么注册 Handler？" → 选注册方式 → register_handlers!/#[handler]/手动注册
    ├── "怎么使用中间件？" → impl IMiddleware → register
    ├── "怎么处理错误？"   → 使用 Error 变体 → 框架自动映射 HTTP 状态码
    ├── "怎么使用事件？"   → impl IEventRequest → impl IEventHandler → mediator.publish()
    └── "特定 API 怎么用？" → 加载 reference/api.md → 查看完整签名

始终：
- 业务应用使用 contracts / handlers / domain 三层，接口在 contracts、实现在 handlers
- contracts 仅依赖框架，禁止引用 domain 或 handlers
- Handler 依赖 Arc<dyn I…Service>，Service 用 inject_attr(as = dyn I…Service) 注册
- 路由标注在 impl IRequest<T> 上，不是 struct 上
- IRequest<T> 泛型参数 = IRequestHandler<T, R> 第二个参数
- 路由扫描和Handler注册已内置，无需手动调用
- 注册为 dyn IRequestHandler<T, R>，不是具体类型
- #[handler] 要求 handler 实现 Default；有 DI 依赖时用 inject_attr + #[handler(inject)]
```
