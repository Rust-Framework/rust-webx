# IPipelineBehavior 拦截链

## 接口

pipeline 是**类型化**的：

```rust
#[async_trait]
pub trait IPipelineBehavior<Req: Send + 'static, Rsp: Send + 'static>: Send + Sync {
    async fn handle(&self, req: Req, next: Next<Req, Rsp>) -> Result<Rsp>;
}
```

`req` / `Rsp` 是真实类型；`next.run(req)` 继续执行剩余链与 Handler。不调用 `next` 即**短路**，
返回值必须就是 Handler 的响应类型——**编译期检查**，没有 `Box<dyn Any>` 与运行时 downcast。

## 用途

在 Handler 执行前后插入横切逻辑：

| Behavior | 职责 |
|----------|------|
| ValidationBehavior | 请求校验（失败可直接 `Err` 返回） |
| LoggingBehavior | 记录请求/响应 |
| CachingBehavior | 响应缓存（命中时短路返回 `Rsp`） |
| TransactionBehavior | 数据库事务包装 |

## 示例

```rust
#[derive(Default)]
pub struct RequireNote;

#[async_trait]
impl IPipelineBehavior<CreateOrderRequest, OrderView> for RequireNote {
    async fn handle(
        &self,
        req: CreateOrderRequest,
        next: Next<CreateOrderRequest, OrderView>,
    ) -> Result<OrderView> {
        if req.note.trim().is_empty() {
            return Err(Error::Validation("note is required".into()));
        }
        next.run(req).await
    }
}
```

## 注册与执行

```rust
svc.add_typed_pipeline::<RequireNote, CreateOrderRequest, OrderView>();
// 需要构造注入时：
svc.add_typed_pipeline_with::<_, CreateOrderRequest, OrderView>(|resolver| {
    let clock: Arc<Clock> = resolver
        .get_any(std::any::type_name::<Clock>())
        .and_then(|service| service.downcast::<Arc<Clock>>().ok())
        .map(|service| Arc::clone(&*service))
        .expect("register Clock first");
    Arc::new(RequireNoteWithClock { clock })
});
```

Behavior 以 singleton 进入 DI，**只对注册的 `<Req, Rsp>` 对生效**；每请求作用域内
`get_all::<dyn IPipelineBehavior<Req, Rsp>>()` 按注册顺序构造嵌套链（第一个注册的最外层）。
`Mediator::send()` 与 HTTP endpoint 分发走同一条链。

## 小结

PipelineBehavior 是 Mediator 层的 AOP 机制，适合跨 Handler 的横切关注点。

下一节：[事件发布与订阅](event-pub-sub.md)
