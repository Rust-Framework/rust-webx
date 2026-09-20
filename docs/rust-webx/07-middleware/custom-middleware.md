# 自定义中间件

## 实现步骤

### 1. 定义中间件 struct

```rust
#[derive(Default)]
pub struct RequestLoggingMiddleware;
```

### 2. 实现 IMiddleware

```rust
use std::ops::ControlFlow;

#[async_trait]
impl IMiddleware for RequestLoggingMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        tracing::info!("→ {} {}", ctx.request().method(), ctx.request().path());
        Ok(ControlFlow::Continue(()))
    }

    async fn after(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        // `after` 在 Handler 执行后运行，可记录状态码或追加响应头
        tracing::info!("← {} {}", ctx.response().status(), ctx.request().path());
        Ok(())
    }
}
```

### 3. 注册

```rust
Host::builder()
    .register(|svc| {
        svc.add_middleware::<RequestLoggingMiddleware>()
    })
```

## 封装为可复用组件

将中间件 + 配置 + 注册逻辑封装为扩展方法：

```rust
// 在你的 crate 中
pub trait HostBuilderExt {
    fn use_tenant_resolver(self, resolver: Arc<TenantResolver>) -> Self;
}

impl HostBuilderExt for HostBuilder {
    fn use_tenant_resolver(self, resolver: Arc<TenantResolver>) -> Self {
        // `tenant_middleware(resolver) -> Arc<dyn IMiddleware>`
        self.use_middleware_with(move || tenant_middleware(Arc::clone(&resolver)))
    }
}
```

这样消费方只需 `.use_tenant_resolver(resolver)` 一行启用。

## 工厂函数模式

需要构造参数的中间件用工厂函数 + `HostBuilder::use_middleware_with` 挂载：

```rust
Host::builder().use_middleware_with(move || {
    Arc::new(TenantMiddleware::new(Arc::clone(&resolver))) as Arc<dyn IMiddleware>
})
```

也可以直接注册到 `register()` 的 `ServiceCollection`：

```rust
svc.add(ServiceLifetime::Singleton, move |_| {
    Arc::new(TenantMiddleware::new(Arc::clone(&resolver))) as Arc<dyn IMiddleware>
})
```

框架自带的 JWT 中间件同样由工厂函数产出：

```rust
pub fn jwt_middleware(handler: Arc<dyn IAuthenticationHandler>) -> impl IMiddleware
```

它由 `add_authentication()` 自动接入管道，通常无需手动注册。

## 小结

自定义中间件只需 `impl IMiddleware` + 注册。复杂场景用工厂函数或 Builder 扩展方法封装。

下一节：[中间件编排策略](ordering-strategy.md)
