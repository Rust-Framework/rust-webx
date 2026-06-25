# IMediator 请求调度

## 接口

```rust
#[async_trait]
pub trait IMediator: Send + Sync {
    async fn send<T, R>(&self, req: T) -> Result<R>
    where
        T: IRequest<R> + Send + 'static,
        R: serde::Serialize + Send + 'static;

    async fn publish<T: IEventRequest>(&self, event: T) -> Result<()>;
}
```

## 使用方式

```rust
// 在 Handler 中注入 Mediator
pub struct CreateOrderHandler {
    mediator: Arc<Mediator>,
}

async fn handle(&self, req: CreateOrderRequest) -> Result<OrderDto> {
    let order = self.create_order(&req).await?;

    // 调度另一个请求
    let user = self.mediator.send(GetUserRequest { id: order.user_id.clone() }).await?;

    Ok(order.into())
}
```

## 注意事项

`IMediator` 不是 dyn-compatible（泛型方法），使用具体类型：

```rust
// ✅
let mediator: Arc<Mediator> = ...;

// ❌ 不能作为 dyn IMediator 存储
let mediator: Arc<dyn IMediator> = ...;
```

## 与直接调用 Handler 的区别

| 方式 | 场景 |
|------|------|
| `mediator.send()` | 跨模块调用、需要 PipelineBehavior 拦截 |
| 直接调用 Service | 同模块内复用逻辑 |
| 直接调用 Handler | 仅测试时使用 |

## 小结

`IMediator::send()` 是模块间请求调度的标准方式，等价于 MediatR 的 `Send()`。

下一节：[IPipelineBehavior 拦截链](pipeline-behaviors.md)
