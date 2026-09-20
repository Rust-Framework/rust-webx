# IPipelineBehavior 拦截链

## 接口

```rust
#[async_trait]
pub trait IPipelineBehavior: Send + Sync {
    async fn handle(
        &self,
        req: Box<dyn Any + Send>,
        next: BoxedNextFn,
    ) -> Result<Box<dyn Any + Send>>;
}
```

`next` 是类型擦除的续接函数；不调用它即短路，跳过后续 behavior 与最终 Handler。

## 用途

在 Handler 执行前后插入横切逻辑：

| Behavior | 职责 |
|----------|------|
| ValidationBehavior | 请求校验 |
| LoggingBehavior | 记录请求/响应 |
| CachingBehavior | 响应缓存 |
| TransactionBehavior | 数据库事务包装 |

## 示例

```rust
#[derive(Default)]
pub struct ValidationBehavior;

#[async_trait]
impl IPipelineBehavior for ValidationBehavior {
    async fn handle(
        &self,
        req: Box<dyn Any + Send>,
        next: BoxedNextFn,
    ) -> Result<Box<dyn Any + Send>> {
        // 前置：校验 req
        // 调用 next 继续管道
        let result = next(req).await?;
        // 后置：处理 result
        Ok(result)
    }
}
```

## 注册与执行

用 `svc.add_pipeline::<ValidationBehavior>()` 注册（要求 `Default`），behavior 以 singleton 进入 DI：

```rust
Host::builder()
    .register(|svc| svc.add_pipeline::<ValidationBehavior>())
    .build();
```

`Mediator::send()` 与 HTTP endpoint 分发都会在每请求作用域内取出全部 `dyn IPipelineBehavior`，
按注册顺序构造嵌套链后调用（第一个注册的最外层）。

## 小结

PipelineBehavior 是 Mediator 层的 AOP 机制，适合跨 Handler 的横切关注点。

下一节：[事件发布与订阅](event-pub-sub.md)
