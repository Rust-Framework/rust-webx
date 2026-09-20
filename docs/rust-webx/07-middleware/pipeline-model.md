# 管道模型与执行顺序

## IMiddleware 接口

```rust
use std::ops::ControlFlow;

#[async_trait]
pub trait IMiddleware: Send + Sync {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>>;

    /// Handler 执行后调用，默认空实现
    async fn after(&self, _ctx: &mut dyn IHttpContext) -> Result<()> {
        Ok(())
    }
}
```

## 执行模型（洋葱模型）

请求先按注册顺序执行各中间件的 `invoke`，最终处理器返回后，**已执行**的中间件再按**逆序**执行
`after` 钩子：

```
invoke 顺序: MW1 → MW2 → MW3 → Router → Endpoint
after  顺序: MW3 → MW2 → MW1
```

中间件可**短路**——返回 `ControlFlow::Break(())` 跳过后续中间件与最终处理器，但已执行中间件的
`after` 钩子仍按逆序运行。

## 短路示例

```rust
use std::ops::ControlFlow;

#[async_trait]
impl IMiddleware for AuthMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        if ctx.claims().is_none() {
            ctx.response_mut().set_status(401);
            return Ok(ControlFlow::Break(()));  // 短路，不继续
        }
        Ok(ControlFlow::Continue(()))
    }
}
```

## IHttpContext

中间件通过 `IHttpContext` 访问请求和响应：

```rust
let method = ctx.request().method();
let path = ctx.request().path();
ctx.response_mut().set_header("X-Custom", "value");
```

## 小结

中间件是 HTTP 层的横切拦截器，通过短路机制实现认证拒绝、限流等。

下一节：[内置中间件一览](built-in-middleware.md)
