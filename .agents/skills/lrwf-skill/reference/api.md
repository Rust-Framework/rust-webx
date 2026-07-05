# LRWF API 参考

## 核心 Trait

### `IRequest<TResponse>`

请求标记 trait。泛型参数 `TResponse` 声明该请求的响应类型。

```rust
pub trait IRequest<TResponse>: Send + 'static
where
    TResponse: serde::Serialize + Send + 'static,
{}
```

- `TResponse: Serialize` → 框架写入 JSON，状态码 200
- `TResponse = ()` → 框架写入空 body，状态码 204

```rust
// 有响应体
impl IRequest<UserModel> for GetUserRequest {}

// 无响应体（返回 204）
impl IRequest<()> for DeleteUserRequest {}
```

### `IRequestHandler<T, R>`

双类型参数处理器。`T` 为请求类型，`R` 为响应类型。注册为 `dyn IRequestHandler<T, R>`。

```rust
#[async_trait::async_trait]
pub trait IRequestHandler<T, R>: Send + Sync
where
    T: IRequest<R> + Send + 'static,
    R: serde::Serialize + Send + 'static,
{
    async fn handle(&self, req: T) -> Result<R>;
}
```

```rust
// 使用示例
#[async_trait]
impl IRequestHandler<HelloRequest, String> for HelloHandler {
    async fn handle(&self, _req: HelloRequest) -> Result<String> {
        Ok("Hello, World!".to_string())
    }
}
```

### `IEventRequest`

事件标记 trait。实现 `Clone + Send + 'static`。

```rust
pub trait IEventRequest: Clone + Send + 'static {}
```

```rust
#[derive(Clone)]
struct UserCreatedEvent { user_id: String, name: String }
impl IEventRequest for UserCreatedEvent {}
```

### `IEventHandler<T>`

事件处理器。通过 `IMediator::publish()` 广播到所有已注册的 handler。

```rust
#[async_trait::async_trait]
pub trait IEventHandler<T: IEventRequest>: Send + Sync {
    async fn handle(&self, event: T) -> Result<()>;
}
```

---

### `IClaims`

认证 claims trait。由认证处理器（如 JWT）生成，存储在 `IHttpContext` 中。

```rust
pub trait IClaims: Send + Sync {
    fn subject(&self) -> &str;
    fn roles(&self) -> &[String];
    fn permissions(&self) -> &[String];
    fn claims(&self) -> &HashMap<String, String>;
}
```

### `IAuthenticationHandler`

认证处理器接口。从 HTTP 上下文中读取凭据并返回 claims。

```rust
#[async_trait::async_trait]
pub trait IAuthenticationHandler: Send + Sync {
    async fn authenticate(&self, ctx: &mut dyn IHttpContext) -> Result<Option<Box<dyn IClaims>>>;
}
```

### `IAuthorizationPolicy`

授权策略接口。检查已认证用户是否有权访问指定资源。

```rust
#[async_trait::async_trait]
pub trait IAuthorizationPolicy: Send + Sync {
    async fn authorize(
        &self,
        claims: &dyn IClaims,
        resource_key: &str,
        method: &str,
    ) -> Result<()>;
}
```

- `resource_key` 为路由匹配后设置的原始路由模式字符串（如 `"/api/users/{id}"`）
- 返回 `Ok(())` 表示授权通过，`Err` 表示禁止访问

### `IClaimsExt`

`IHttpContext` 的扩展 trait，用于存取认证 claims。

```rust
pub trait IClaimsExt {
    fn set_claims(&mut self, claims: Box<dyn IClaims>);
    fn claims(&self) -> Option<&dyn IClaims>;
}
```

`IHttpContext` 继承了 `IClaimsExt`，因此认证/授权中间件可直接通过 `ctx.set_claims()` / `ctx.claims()` 存取认证信息。

### `IMediator`

中介者。调度请求和处理事件。

```rust
#[async_trait::async_trait]
pub trait IMediator: Send + Sync {
    async fn send<T, R>(&self, req: T) -> Result<R>
    where
        T: IRequest<R> + Send + 'static,
        R: serde::Serialize + Send + 'static;

    async fn publish<T: IEventRequest>(&self, event: T) -> Result<()>;
}
```

> **注意：** `IMediator` 不是 dyn-compatible（`send` 和 `publish` 有泛型参数）。使用具体类型 `Arc<Mediator>` 而非 `Arc<dyn IMediator>`。

### `IMiddleware`

HTTP 中间件。按注册顺序调用，通过设置 response status 实现短路。

```rust
#[async_trait::async_trait]
pub trait IMiddleware: Send + Sync {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<()>;
}
```

### `IPipelineBehavior`

Mediator 管道拦截器。包装请求处理链，可检查/修改请求和响应，或短路整个链。

```rust
#[async_trait::async_trait]
pub trait IPipelineBehavior: Send + Sync {
    async fn handle(
        &self,
        req: Box<dyn Any + Send>,
        next: BoxedNextFn,
        svc: Arc<dyn IServiceResolver>,
    ) -> Result<Box<dyn Any + Send>>;
}
```

> **注意：** 当前版本为骨架实现，完整的管道链将在后续版本通过类型擦除实现。

### `IHttpContext`

HTTP 上下文，封装请求、响应。继承 `IClaimsExt` 以支持认证 claims 的存取。

```rust
pub trait IHttpContext: IClaimsExt + Send {
    fn request(&self) -> &dyn IHttpRequest;
    fn request_mut(&mut self) -> &mut dyn IHttpRequest;
    fn response(&self) -> &dyn IHttpResponse;
    fn response_mut(&mut self) -> &mut dyn IHttpResponse;
}
```

### `IHttpRequest`

HTTP 请求抽象（dyn-compatible）。

```rust
#[async_trait::async_trait]
pub trait IHttpRequest: Send {
    fn method(&self) -> &str;
    fn path(&self) -> &str;
    fn header(&self, name: &str) -> Option<&str>;
    fn query(&self) -> &HashMap<String, String>;
    fn route_params(&self) -> &HashMap<String, String>;
    fn route_params_mut(&mut self) -> &mut HashMap<String, String>;
    fn route_pattern(&self) -> Option<&str>;
    fn route_pattern_mut(&mut self) -> &mut Option<String>;
    async fn body_bytes(&self) -> Result<Vec<u8>>;
    async fn body_text(&self) -> Result<String>;
}
```

### `IHttpResponse`

HTTP 响应抽象（dyn-compatible）。

```rust
#[async_trait::async_trait]
pub trait IHttpResponse: Send {
    fn set_status(&mut self, code: u16);
    fn set_header(&mut self, key: &str, value: &str);
    async fn write_bytes(&mut self, data: Vec<u8>) -> Result<()>;
    async fn write_text(&mut self, text: &str) -> Result<()>;
}
```

### `IEndpoint`

端点处理器。

```rust
#[async_trait::async_trait]
pub trait IEndpoint: Send + Sync {
    async fn handle(&self, ctx: &mut dyn IHttpContext) -> Result<()>;
}
```

### `IRouter`

路由器。Trie 树匹配 method + path。

```rust
#[async_trait::async_trait]
pub trait IRouter: Send + Sync {
    fn register(&mut self, method: HttpMethod, path: &str, endpoint: Arc<dyn IEndpoint>);
    async fn match_route(
        &self,
        ctx: &mut dyn IHttpContext,
    ) -> Result<Option<(Arc<dyn IEndpoint>, HashMap<String, String>, String)>>;
}
```

返回三元组：
- `Arc<dyn IEndpoint>` — 匹配的端点处理器。
- `HashMap<String, String>` — 路由参数值（如 `{id}` → 具体值）。
- `String` — 原始路由模式字符串（如 `"/api/users/{id}"`），供授权中间件使用。

### `HostBuilder::use_middleware`

注册中间件到管道。中间件以 Singleton 注册到 DI，`build()` 时通过
`provider.get_all::<dyn IMiddleware>()` 自动收集。

```rust
impl HostBuilder {
    pub fn use_middleware<T: IMiddleware + Default + 'static>(mut self) -> Self { ... }
}
```

约束：`T` 必须实现 `Default`。需配置的中间件（如 `CorsMiddleware`）请使用
`use_cors()` 或在 `register()` 中手动注册工厂。

### `IHost`

宿主，绑定地址并启动 HTTP 服务。

```rust
#[async_trait::async_trait]
pub trait IHost: Send + Sync {
    async fn run(&self, addr: &str) -> Result<()>;
    async fn stop(&self) -> Result<()>;
}
```

### `IServiceCollectionExt`

lrdi `ServiceCollection` 扩展 trait。

```rust
pub trait IServiceCollectionExt: Sized {
    fn add_mediator(self) -> Self;
    fn add_request_endpoints(self) -> Self;
    fn add_controllers(self) -> Self;
    fn add_middleware<T: IMiddleware + Default + Send + Sync + 'static>(self) -> Self;
    fn add_pipeline<T: IPipelineBehavior + Default + Send + Sync + 'static>(self) -> Self;
}
```

---

## 类型

### `Error`

统一错误类型。

```rust
#[derive(Error, Debug)]
pub enum Error {
    Http(String),          // 400
    Di(String),            // 500
    Routing(String),       // 404
    Serialization(serde_json::Error),  // 400
    Internal(String),      // 500
    Message(String),       // 500
    Validation(String),    // 400
    NotFound(String),      // 404
}

impl Error {
    pub fn status_code(&self) -> u16;
}
```

### `HttpMethod`

HTTP 方法枚举。

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    Get, Post, Put, Delete, Patch, Head, Options,
}

impl HttpMethod {
    pub fn as_str(&self) -> &'static str;
    pub fn from_str(s: &str) -> Option<Self>;
}
```

### `HttpStatus`

HTTP 状态码常量。

```rust
pub struct HttpStatus;
impl HttpStatus {
    pub const OK: u16 = 200;
    pub const CREATED: u16 = 201;
    pub const NO_CONTENT: u16 = 204;
    pub const BAD_REQUEST: u16 = 400;
    pub const UNAUTHORIZED: u16 = 401;
    pub const FORBIDDEN: u16 = 403;
    pub const NOT_FOUND: u16 = 404;
    pub const INTERNAL_SERVER_ERROR: u16 = 500;
}
```

### `RouteEntry`

编译时路由条目。由 `#[get]`/`#[post]` 等宏通过 inventory 提交。

```rust
#[derive(Debug, Clone)]
pub struct RouteEntry {
    pub method: HttpMethod,
    pub path: &'static str,
    pub handler_type: &'static str,
    pub source: RouteSource,
}

impl RouteEntry {
    pub const fn request(method: HttpMethod, path: &'static str, handler_type: &'static str) -> Self;
    pub const fn controller(method: HttpMethod, path: &'static str, handler_type: &'static str) -> Self;
}
```

### `RouteSource`

路由来源标记。

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteSource {
    RequestEndpoint,
    ControllerMethod,
}
```

### `HandlerRegistration`

编译时 Handler 注册条目。由 `#[handler]` 宏通过 inventory 提交。

```rust
pub struct HandlerRegistration {
    pub register: fn(svc: &mut lrdi::ServiceCollection),
}
```

### `RouteMeta`

路由元数据。

```rust
#[derive(Debug, Clone)]
pub struct RouteMeta {
    pub method: HttpMethod,
    pub path: String,
}

impl RouteMeta {
    pub fn new(method: HttpMethod, path: impl Into<String>) -> Self;
}
```

---

## 实现类型（lrwf-http / lrwf-mediator）

### `Host` / `HostBuilder`

```rust
pub struct Host { /* ... */ }
impl Host {
    pub fn builder() -> HostBuilder;
}

pub struct HostBuilder { /* ... */ }
impl HostBuilder {
    pub fn new() -> Self;
    pub fn register<F>(self, f: F) -> Self
    where F: FnOnce(ServiceCollection) -> ServiceCollection + Send + 'static;
    pub fn configure<F>(self, f: F) -> Self
    where F: FnOnce(&mut HostAppBuilder) + Send + 'static;
    pub fn build(self) -> Host;
}
```

### `Mediator`

```rust
pub struct Mediator { /* ... */ }
impl Mediator {
    pub fn new(provider: Arc<ServiceProvider>) -> Self;
}

#[async_trait::async_trait]
impl IMediator for Mediator { /* ... */ }
```

### `HttpContext` / `HttpRequest` / `HttpResponse`

HTTP 上下文的具体实现。

```rust
pub struct HttpContext { /* ... */ }
impl HttpContext {
    pub async fn new(req: hyper::Request<hyper::body::Incoming>) -> Self;
    pub fn into_response(self) -> hyper::Response<hyper::body::Full<hyper::body::Bytes>>;
}
```

### `Router`

Trie 树路由器实现。

```rust
pub struct Router { /* ... */ }
impl Router {
    pub fn new() -> Self;
}

#[async_trait::async_trait]
impl IRouter for Router { /* ... */ }
```

### `MiddlewarePipeline`

中间件顺序管道。

```rust
pub struct MiddlewarePipeline { /* ... */ }
impl MiddlewarePipeline {
    pub fn new() -> Self;
    pub fn add_middleware(&mut self, middleware: Arc<dyn IMiddleware>);
    pub async fn execute(
        &self,
        ctx: &mut dyn IHttpContext,
        final_handler: HandlerFn,
    ) -> Result<()>;
}
```

---

### `JwtClaims`

JWT claims 实现，实现了 `IClaims` trait。

```rust
pub struct JwtClaims { /* ... */ }
impl JwtClaims {
    pub fn new(subject: impl Into<String>) -> Self;
}
impl IClaims for JwtClaims {
    fn subject(&self) -> &str;
    fn roles(&self) -> &[String];
    fn permissions(&self) -> &[String];
    fn claims(&self) -> &HashMap<String, String>;
}
```

### `JwtAuth`

JWT 认证处理器，实现 `IAuthenticationHandler`。从 `Authorization: Bearer <token>` 头读取并验证 JWT。

```rust
pub struct JwtAuth { /* ... */ }
impl JwtAuth {
    pub fn new(decoding_key: DecodingKey, validation: Validation) -> Self;
}

#[async_trait::async_trait]
impl IAuthenticationHandler for JwtAuth { /* ... */ }

// 便捷函数：创建认证中间件
pub fn jwt_middleware(handler: Arc<dyn IAuthenticationHandler>) -> impl IMiddleware;
```

### `ResourceAuthorization`

基于资源的角色/权限授权策略，实现 `IAuthorizationPolicy`。

```rust
pub struct ResourceAuthorization { /* ... */ }
impl ResourceAuthorization {
    pub fn new() -> Self;
    pub fn allow_role(self, resource_key: impl Into<String>, role: impl Into<String>) -> Self;
    pub fn allow_permission(self, resource_key: impl Into<String>, permission: impl Into<String>) -> Self;
}

#[async_trait::async_trait]
impl IAuthorizationPolicy for ResourceAuthorization { /* ... */ }

// 便捷函数：创建授权中间件
pub fn resource_auth_middleware(policy: Arc<dyn IAuthorizationPolicy>) -> impl IMiddleware;
```

---

## 过程宏

| 宏 | 类型 | 目标 | 说明 |
|----|------|------|------|
| `#[get("/path")]` | 属性宏 | `impl IRequest<T>` 块 | GET 路由快捷键 |
| `#[post("/path")]` | 属性宏 | `impl IRequest<T>` 块 | POST 路由快捷键 |
| `#[put("/path")]` | 属性宏 | `impl IRequest<T>` 块 | PUT 路由快捷键 |
| `#[delete("/path")]` | 属性宏 | `impl IRequest<T>` 块 | DELETE 路由快捷键 |
| `#[endpoint(HttpMethod, "/path")]` | 属性宏 | `impl IRequest<T>` 块 | 完整形式，支持所有 HTTP 方法 |
| `#[handler]` | 属性宏 | `impl IRequestHandler<T,R>` 块 | 编译时自动注册（Handler 须 Default） |
| `#[controller("/base")]` | 属性宏 | struct | Controller 标记（预留） |
| `#[http_get]` / `#[http_post]` / `#[http_put]` / `#[http_delete]` | 属性宏 | method | Controller 方法标注（预留） |
| `#[from_body]` | 属性宏 | field / param | 从 JSON body 反序列化（预留） |
| `#[from_route]` | 属性宏 | field / param | 从路径参数提取（预留） |
| `#[from_query]` | 属性宏 | field / param | 从 query string 提取（预留） |

## 声明宏

### `register_handlers!`

为 `Default` handler 生成链式 `.singleton()` 注册。

```rust
// 语法：RequestType => ResponseType => HandlerType
register_handlers!(svc,
    HelloRequest => String => HelloHandler,
    ListUsersRequest => Vec<UserModel> => ListUsersHandler,
)

// 展开为：
svc
    .singleton::<dyn IRequestHandler<HelloRequest, String>>(|_| Arc::new(HelloHandler::default()))
    .singleton::<dyn IRequestHandler<ListUsersRequest, Vec<UserModel>>>(|_| Arc::new(ListUsersHandler::default()))
```

## 辅助函数

```rust
// 从请求 body 读取 JSON
pub async fn read_json_body<T: serde::de::DeserializeOwned>(
    req: &dyn IHttpRequest,
) -> Result<T>;

// 写入 JSON 响应
pub async fn write_json_response<T: serde::Serialize + Send>(
    resp: &mut dyn IHttpResponse,
    value: &T,
) -> Result<()>;
```
