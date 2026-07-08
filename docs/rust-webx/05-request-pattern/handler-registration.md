# Handler 注册策略

## 三种注册方式

| 方式 | 适用场景 | 依赖注入 |
|------|---------|---------|
| `#[handler]` | Handler 实现 Default | 无依赖 |
| `#[handler(inject)]` + `inject_attr` | Handler 有 DI 依赖 | 自动注入 |
| 手动 `singleton` 注册 | 复杂构造逻辑 | 手动控制 |

## 方式一：#[handler] 零配置

```rust
#[derive(Default)]
struct HelloHandler;

#[handler]
#[async_trait]
impl IRequestHandler<HelloRequest, String> for HelloHandler { ... }
```

要求：
- Handler struct 实现 `Default`
- 无构造函数参数

## 方式二：register_handlers! 批量注册

```rust
Host::builder()
    .register(|svc| {
        register_handlers!(svc,
            HelloRequest => String => HelloHandler,
            ListUsersRequest => Vec<UserDto> => ListUsersHandler,
            DeleteUserRequest => () => DeleteUserHandler,
        )
    })
```

等价于多条 `.singleton::<dyn IRequestHandler<...>>()` 调用。

## 方式三：手动注册（有依赖）

```rust
Host::builder()
    .register(move |svc| {
        let repo = Arc::new(UserRepository::new());
        svc.singleton::<dyn IRequestHandler<GetUserRequest, UserDto>>(
            move |_| Arc::new(GetUserHandler { repo: Arc::clone(&repo) })
        );
    })
```

## 方式四：inject_attr + #[handler(inject)]

Docbit 推荐的生产模式：

```rust
#[rust_dicore::inject_attr(singleton, as = dyn IRequestHandler<LoginRequest, AuthResponse>)]
pub struct LoginHandler {
    ctx: Arc<Mutex<DbContext>>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<LoginRequest, AuthResponse> for LoginHandler { ... }
```

- `inject_attr` 声明如何构造 Handler（自动解析 `ctx` 等依赖）
- `#[handler(inject)]` 标记走注入路径
- 无需在 `main.rs` 中逐个手动注册

## 选择指南

```mermaid
graph TD
    A[新 Handler] --> B{有外部依赖?}
    B -->|否| C[#[derive Default] + #[handler]]
    B -->|是| D{使用 rust-dicore inject?}
    D -->|是| E[inject_attr + #[handler inject]]
    D -->|否| F[手动 singleton 注册]
```

## 排查：No handler registered

1. 检查是否使用了 `#[handler]` 或手动注册
2. 检查注册类型是否为 `dyn IRequestHandler<T, R>`
3. 检查 Handler 模块是否被 `mod handlers;` 引用（inventory 链接）
4. 检查 `IRequest<T>` 与 `IRequestHandler<T, R>` 类型是否一致

## 小结

简单 Handler 用 `#[handler]` 零配置；生产项目推荐 `inject_attr` 模式，保持 `main.rs` 简洁。

下一节：[参数绑定与序列化](parameter-binding.md)
