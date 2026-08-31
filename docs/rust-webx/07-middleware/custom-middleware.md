# 自定义中间件

## 实现步骤

### 1. 定义中间件 struct

```rust
#[derive(Default)]
pub struct RequestLoggingMiddleware;
```

### 2. 实现 IMiddleware

```rust
#[async_trait]
impl IMiddleware for RequestLoggingMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        let method = ctx.request().method().to_string();
        let path = ctx.request().path().to_string();
        let start = std::time::Instant::now();

        tracing::info!("→ {} {}", method, path);

        // 注意：当前管道模型中，此处无法获取 Handler 执行后的耗时
        // 可在后续洋葱模型中实现 post-process

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
        self.register(move |svc| {
            svc.add_middleware_instance(tenant_middleware(resolver))
        })
    }
}
```

这样消费方只需 `.use_tenant_resolver(resolver)` 一行启用。

## 工厂函数模式

JWT 中间件使用工厂函数返回 `Arc<dyn IMiddleware>`：

```rust
pub fn jwt_middleware(auth: Arc<JwtAuth>) -> Arc<dyn IMiddleware> {
    Arc::new(JwtAuthMiddleware { auth })
}

svc.add_middleware_instance(jwt_middleware(auth))
```

适用于需要构造参数的中间件。

## 小结

自定义中间件只需 `impl IMiddleware` + 注册。复杂场景用工厂函数或 Builder 扩展方法封装。

下一节：[中间件编排策略](ordering-strategy.md)
