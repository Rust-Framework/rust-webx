# Rust 惯用法与类型安全

## 编译期契约

rust-webx 将尽可能多的错误前移到编译期：

### 响应类型绑定

```rust
// ✅ 编译通过
#[get("/users/{id}")]
impl IRequest<UserDto> for GetUserRequest {}

impl IRequestHandler<GetUserRequest, UserDto> for GetUserHandler { ... }

// ❌ 编译失败：IRequest 声明 UserDto，Handler 返回 String
impl IRequestHandler<GetUserRequest, String> for GetUserHandler { ... }
```

### Handler 注册类型

```rust
// ✅ #[handler] 向 inventory 提交注册，HTTP 分发通过 HandlerCache 查找
#[handler]
#[async_trait]
impl IRequestHandler<GetUserRequest, UserDto> for GetUserHandler { ... }

// ❌ 手动把 Handler 注册进 DI：HTTP 分发不会查找它
svc.singleton::<dyn IRequestHandler<GetUserRequest, UserDto>>(...)
```

### #[handler] 前置条件

```rust
// ✅ Default 可构造
#[derive(Default)]
struct HelloHandler;

// ❌ 有字段的 Handler 不能用 #[handler]，需改用 #[handler(inject)]
struct GetUserHandler { repo: Arc<Repo> }
```

## 零成本抽象

框架的 trait 抽象在运行时开销极低：

- 路由匹配使用 matchit（radix 树），每请求零堆分配
- Handler 解析结果缓存在 `HandlerCache` 中
- `inventory` 元数据在编译期收集，`build()` 时一次性注册

## Send + Sync 约束

所有 Handler 和中间件必须 `Send + Sync`：

```rust
pub trait IRequestHandler<T, R>: Send + Sync { ... }
pub trait IMiddleware: Send + Sync { ... }
```

这保证了在多线程 Tokio 运行时中安全共享。若 Handler 持有 `Rc<RefCell<T>>` 等非 Send 类型，编译器会拒绝。

## async trait

Rust 的 async trait 通过 `async-trait` crate 实现：

```rust
#[async_trait]
impl IRequestHandler<HelloRequest, String> for HelloHandler {
    async fn handle(&mut self, req: HelloRequest) -> Result<String> { ... }
}
```

所有 Handler 和中间件的 `async fn` 均需 `#[async_trait]` 标注。

## Arc 与共享状态

Rust 无 GC，跨请求共享状态使用 `Arc`：

```rust
let store = Arc::new(RwLock::new(HashMap::new()));

Host::builder()
    .register(move |svc| {
        let store = Arc::clone(&store);
        svc.singleton::<UserStore>(move |_| Arc::clone(&store));
    })
```

| 类型 | 场景 |
|------|------|
| `Arc<T>` | 不可变共享（连接池、配置） |
| `Arc<RwLock<T>>` | 读写锁保护的可变状态 |
| `Arc<Mutex<T>>` | 互斥锁保护的共享可变状态 |

## 显式错误处理

```rust
// ✅ 业务错误用 Error 变体
return Err(Error::Validation("email is required".into()));

// ❌ 不要用 panic 处理可预期错误
panic!("user not found");
```

| Error 变体 | HTTP | 场景 |
|-----------|------|------|
| `NotFound` | 404 | 资源不存在 |
| `Validation` | 400 | 参数校验 |
| `Http` | 400 | 协议错误 |
| `Unauthorized` | 401 | 未认证 |
| `Forbidden` | 403 | 未授权 |
| `Internal` | 500 | 未预期内部错误 |

## 小结

rust-webx 充分利用 Rust 类型系统在编译期捕获错误，用 `Arc` + `async-trait` 处理并发与异步，用显式 `Result` 替代异常。

下一节：[渐进式披露与框架边界](progressive-disclosure.md)
