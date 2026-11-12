# IRequest 与 IRequestHandler

## 核心契约

rust-webx 用两个 trait 定义一个完整端点：

```rust
// 声明：这是一个请求，响应类型为 T
pub trait IRequest<TResponse>: Send + 'static
where
    TResponse: Send + 'static,
{}

// 实现：如何处理这个请求
#[async_trait]
pub trait IRequestHandler<T, R>: Send + Sync
where
    T: IRequest<R> + Send + 'static,
    R: Send + 'static,
{
    async fn handle(&mut self, req: T) -> ActionResult<R>;
}
```

### 返回类型：`ActionResult<T>`

`handle` 返回 **`ActionResult<T>`** —— 框架级类型，对标 ASP.NET Core 的 `ActionResult<T>`：
`Ok(value)` 是成功的响应数据（框架序列化为 JSON），`Err(webx::Error)` 由框架映射为
`problem+json` 并用对应状态码写出。它与通用错误别名 `Result<T>` 是同一个类型
（`ActionResult<T> = Result<T, Error>`），区别只在命名面向"端点动作"：Handler、管道行为、
`Next::run` 用 `ActionResult<T>`，基础设施代码（host、中间件、缓存）继续用 `Result<T>`。

## 类型绑定规则（铁律）

**`IRequest<T>` 的泛型参数 T 必须等于 `IRequestHandler<T, R>` 的 R。**

```rust
// ✅ 正确
#[get("/users/{id}")]
impl IRequest<UserDto> for GetUserRequest {}

impl IRequestHandler<GetUserRequest, UserDto> for GetUserHandler { ... }

// ❌ 错误：响应类型不一致
impl IRequest<String> for GetUserRequest {}
impl IRequestHandler<GetUserRequest, UserDto> for GetUserHandler { ... }
```

违反此规则会导致编译失败或运行时类型不匹配。

## IRequest 的职责

`IRequest<T>` 是一个**标记 trait**（marker trait），本身无方法。它的价值在于：

1. **承载响应类型信息** — 框架据此决定序列化什么、返回什么状态码
2. **关联路由元数据** — 通过 `#[get]` 等宏在 impl 块上附加
3. **作为类型参数** — `IMediator::send()` 的泛型约束

### 响应类型语义

下表是 `ActionResult<T>` 中 `T` 的取值语义。

| 响应类型 | HTTP 行为 |
|---------|----------|
| `String`, `UserDto`, `Vec<T>` 等（`Serialize`） | 200 + JSON body |
| `()` | 204 No Content，空 body |
| `PagedResult<T>` | 200 + `{items, total, page, page_size, total_pages}` |
| `OperationResult<T>` | 200 + `{success, status, message, result}` |
| `OperationResult` | 200 + `{success, status, message}` |
| `ResponseData` | Handler 自行掌控状态码、响应头与响应体 |

`PagedResult<T>` 与 `OperationResult` 是框架随附的两个业务 DTO 信封：列表用前者，
更新/删除/状态流转等操作结果用后者，业务模块不必再各自定义 `{success, status, message}`。

```rust
#[put("/users/{id}")]
impl IRequest<OperationResult<UserDto>> for UpdateUserRequest {}

impl IRequestHandler<UpdateUserRequest, OperationResult<UserDto>> for UpdateUserHandler {
    async fn handle(&mut self, req: UpdateUserRequest) -> ActionResult<OperationResult<UserDto>> {
        let user = self.update(req).await?;
        Ok(OperationResult::ok(user, "user updated"))
    }
}
```

`OperationResult` 的 `status` 是**业务状态码**（错误码、流程状态，含义由业务定义），
不是 HTTP 状态码；HTTP 状态仍由框架决定——正常返回 `200` + 信封 JSON，
`Err(webx::Error)` 仍写 `problem+json`（见[错误处理](error-handling.md)）。构造器：
`ok` / `ok_with_status` / `failed` / `success` / `success_with_status`；不带泛型的
`OperationResult` 没有 `result` 成员。结果常在业务执行过程中才确定，因此创建后可以继续调整：
`with_status` / `with_message` / `with_result`（把无数据信封升级为带 `result` 的形态），
或直接给公开字段赋值。

## IRequestHandler 的职责

Handler 是**唯一执行业务逻辑**的地方：

```rust
#[async_trait]
impl IRequestHandler<CreateUserRequest, UserDto> for CreateUserHandler {
    async fn handle(&mut self, req: CreateUserRequest) -> ActionResult<UserDto> {
        // 1. 校验（或用 PipelineBehavior）
        // 2. 调用领域服务
        // 3. 返回结果或 Error
        let user = self.service.create(&req.name, &req.email).await?;
        Ok(user)
    }
}
```

## 分发方式：inventory + HandlerCache

HTTP 请求分发使用 **编译时 inventory 收集**，不通过 DI 查找 `dyn IRequestHandler`：

| 宏 | 提交内容 |
|----|---------|
| `#[handler]` / `#[handler(inject)]` | `HandlerRegistration` — 每请求工厂与调用桥接 |

`Host::build()` 将这些记录收集为 `HandlerCache`，按请求类型（`TypeId` / 类型名）查找后在每请求作用域内构造处理器。手动把处理器注册为 `dyn IRequestHandler<T, R>` 仅用于非 HTTP 的显式 DI 场景，不会参与 HTTP 路由分发。详见 [Handler 注册策略](handler-registration.md)。

## 与 IMediator 的协作

在 Handler 或 Service 中跨模块发送请求，见 [IMediator 请求调度](../08-mediator-events/mediator-pattern.md)。

## 设计优势

| 优势 | 说明 |
|------|------|
| 一个文件一个端点 | Request + Handler 可放在同一文件 |
| 编译期类型检查 | 响应类型不匹配 = 编译错误 |
| 可测试 | 直接实例化 Handler 调用 `handle()` |
| OpenAPI 友好 | 类型信息可自动提取 |

## 小结

`IRequest<T>` 声明「要什么」，`IRequestHandler<T,R>` 实现「怎么做」。掌握这对 trait 是使用框架的全部钥匙。

下一节：[路由宏详解](route-macros.md)
