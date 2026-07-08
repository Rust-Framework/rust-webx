# IRequest 与 IRequestHandler

## 核心契约

rust-webx 用两个 trait 定义一个完整端点：

```rust
// 声明：这是一个请求，响应类型为 T
pub trait IRequest<TResponse>: Send + 'static
where
    TResponse: serde::Serialize + Send + 'static,
{}

// 实现：如何处理这个请求
#[async_trait]
pub trait IRequestHandler<T, R>: Send + Sync
where
    T: IRequest<R> + Send + 'static,
    R: serde::Serialize + Send + 'static,
{
    async fn handle(&self, req: T) -> Result<R>;
}
```

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

| 响应类型 | HTTP 行为 |
|---------|----------|
| `String`, `UserDto`, `Vec<T>` 等 | 200 + JSON body |
| `()` | 204 No Content，空 body |

## IRequestHandler 的职责

Handler 是**唯一执行业务逻辑**的地方：

```rust
#[async_trait]
impl IRequestHandler<CreateUserRequest, UserDto> for CreateUserHandler {
    async fn handle(&self, req: CreateUserRequest) -> Result<UserDto> {
        // 1. 校验（或用 PipelineBehavior）
        // 2. 调用领域服务
        // 3. 返回结果或 Error
        let user = self.service.create(&req.name, &req.email).await?;
        Ok(user)
    }
}
```

## 注册为 dyn trait object

框架通过 `dyn IRequestHandler<T, R>` 做类型擦除和 DI 解析：

```rust
svc.singleton::<dyn IRequestHandler<CreateUserRequest, UserDto>>(
    |_| Arc::new(CreateUserHandler::default())
)
```

**必须注册为 dyn trait**，不能注册具体类型。

## 与 IMediator 的协作

```rust
// 在另一个 Handler 或 Service 中
let user = self.mediator.send(GetUserRequest { id: "123".into() }).await?;
```

`send()` 根据 `GetUserRequest` 的类型自动找到对应 Handler，无需手动指定。

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
