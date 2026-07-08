# 管道模型与执行顺序

## IMiddleware 接口

```rust
#[async_trait]
pub trait IMiddleware: Send + Sync {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<()>;
}
```

## 执行模型

当前版本为**顺序管道**（非洋葱模型）：

```
请求 → MW1 → MW2 → MW3 → Router → Endpoint
```

每个中间件按注册顺序调用。中间件可**短路**——设置 response status 后直接返回，跳过后续处理和路由。

## 短路示例

```rust
#[async_trait]
impl IMiddleware for AuthMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        if ctx.claims().is_none() {
            ctx.response_mut().set_status(401);
            return Ok(());  // 短路，不继续
        }
        Ok(())
    }
}
```

## 与 ASP.NET Core 的差异

ASP.NET Core 中间件支持 `next()` 闭包实现洋葱模型（请求进入和响应返回各经过一次）。rust-webapp 当前为顺序调用，响应阶段不回溯中间件。

> 后续版本计划在 async closure 稳定后升级为洋葱模型。

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
