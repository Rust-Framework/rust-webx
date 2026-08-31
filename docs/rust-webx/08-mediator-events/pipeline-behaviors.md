# IPipelineBehavior 拦截链

## 接口

```rust
#[async_trait]
pub trait IPipelineBehavior: Send + Sync {
    async fn handle(
        &self,
        req: Box<dyn Any + Send>,
        next: BoxedNextFn,
        svc: Arc<dyn IServiceResolver>,
    ) -> Result<Box<dyn Any + Send>>;
}
```

## 用途

在 Handler 执行前后插入横切逻辑：

| Behavior | 职责 |
|----------|------|
| ValidationBehavior | 请求校验 |
| LoggingBehavior | 记录请求/响应 |
| CachingBehavior | 响应缓存 |
| TransactionBehavior | 数据库事务包装 |

## 示例骨架

```rust
pub struct ValidationBehavior;

#[async_trait]
impl IPipelineBehavior for ValidationBehavior {
    async fn handle(
        &self,
        req: Box<dyn Any + Send>,
        next: BoxedNextFn,
        svc: Arc<dyn IServiceResolver>,
    ) -> Result<Box<dyn Any + Send>> {
        // 前置：校验 req
        // 调用 next 继续管道
        let result = next(req, svc).await?;
        // 后置：处理 result
        Ok(result)
    }
}
```

## 当前状态

`IPipelineBehavior` 当前为骨架实现，完整的类型安全管道链在后续版本通过类型擦除完善。生产项目可先在 Handler 内实现校验逻辑。

## 小结

PipelineBehavior 是 Mediator 层的 AOP 机制，适合跨 Handler 的横切关注点。

下一节：[事件发布与订阅](event-pub-sub.md)
