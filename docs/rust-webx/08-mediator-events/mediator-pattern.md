# IMediator 请求调度

## 接口

```rust
#[async_trait]
pub trait IMediator: Send + Sync {
    // 请求：等待处理器并返回结果
    async fn send<T, R>(&self, req: T) -> Result<R>
    where
        T: IRequest<R> + Send + 'static,
        R: Send + 'static,
        Self: Sized;

    // 类型擦除请求：可在 Arc<dyn IMediator> 上调用
    async fn send_boxed(
        &self,
        req: Box<dyn Any + Send>,
    ) -> Result<Box<dyn Any + Send>>;

    // 事件：交给传输层即返回，不等待、不返回处理器结果
    async fn publish<T: IEventRequest + serde::Serialize>(&self, event: T) -> Result<()>
    where
        Self: Sized;
}
```

`publish` 的语义见[事件发布与订阅](event-pub-sub.md)：默认内存队列，可由 `HostBuilder::use_event_publisher` 替换成 Redis / 消息队列。

## 使用方式

```rust
// 在 Handler 中注入 Mediator
pub struct CreateOrderHandler {
    mediator: Arc<Mediator>,
}

async fn handle(&mut self, req: CreateOrderRequest) -> ActionResult<OrderDto> {
    let order = self.create_order(&req).await?;

    // 调度另一个请求，等待其结果
    let user = self.mediator.send(GetUserRequest { id: order.user_id.clone() }).await?;

    Ok(order.into())
}
```

## 对象安全

`send` / `publish` 是泛型方法（带 `Self: Sized`），所以不能直接在 `dyn IMediator` 上调用。需要类型擦除调用时注入 `Arc<dyn IMediator>` 并使用 `send_boxed`：

```rust
let mediator: Arc<dyn IMediator> = ...;
let response = mediator.send_boxed(Box::new(GetUserRequest { id })).await?;
let user = *response.downcast::<UserModel>().expect("typed response");
```

## 分发路径（HTTP 与进程内）

同一个 Handler 有两条 HTTP 分发路径，构建期由 Host 自动选择：

- **直连快路径**（未注册 typed pipeline behavior）：宏生成 `bind_request + 构造 + handle`，
  不经过 Mediator，省去每请求的 scope/链开销；
- **Mediator 路径**（注册了任意 `IPipelineBehavior`，或非直连路由）：HTTP 走
  `Mediator::send`，与进程内调用同一条链，保证 behavior 对 HTTP 生效。

两者共享 `HandlerCache`/宏生成工厂，语义由构建期标记保证（按 Host 隔离）。完整架构见
[架构 ADR](../../design/mediator-handler-architecture.md)。

## 与直接调用 Handler 的区别

| 方式 | 场景 |
|------|------|
| `mediator.send()` | 跨模块调用、需要 PipelineBehavior 拦截 |
| 直接调用 Service | 同模块内复用逻辑 |
| 直接调用 Handler | 仅测试时使用 |

## 小结

`IMediator::send()` 是模块间请求调度的标准方式，等价于 MediatR 的 `Send()`；`publish()` 把事件交给传输层（默认有界内存队列 + 后台 worker，可换 broker），不等待处理器、不返回处理器结果。

下一节：[IPipelineBehavior 拦截链](pipeline-behaviors.md)
