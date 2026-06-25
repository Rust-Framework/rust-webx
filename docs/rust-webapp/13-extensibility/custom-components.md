# 自定义组件封装

## 封装中间件组件

将中间件 + 配置 + 注册封装为 Builder 扩展：

```rust
pub trait TenantBuilderExt {
    fn use_multi_tenant(self, config: TenantConfig) -> Self;
}

impl TenantBuilderExt for HostBuilder {
    fn use_multi_tenant(self, config: TenantConfig) -> Self {
        let resolver = Arc::new(TenantResolver::new(config));
        self.register(move |svc| {
            svc.singleton::<TenantResolver>(move |_| Arc::clone(&resolver));
            svc.add_middleware_instance(tenant_middleware(Arc::clone(&resolver)));
        })
    }
}
```

消费方：

```rust
Host::builder().use_multi_tenant(config).build()
```

## 封装服务组件

```rust
// 在你的 library crate 中
pub struct EmailModule;

impl EmailModule {
    pub fn register(svc: &mut ServiceCollection, config: SmtpConfig) {
        svc.singleton::<EmailService>(move |_| Arc::new(EmailService::new(config)));
    }
}

// main.rs
.register(|svc| EmailModule::register(svc, smtp_config))
```

## 封装 Handler 基类模式

Rust 无继承，用组合替代：

```rust
pub struct BaseHandler {
    pub cache: Arc<dyn IDistributedCache>,
    pub mediator: Arc<Mediator>,
}

impl BaseHandler {
    pub async fn get_cached_user(&self, id: &str) -> Result<UserDto> {
        self.cache.get_or_create(&format!("user:{}", id), async {
            self.mediator.send(GetUserRequest { id: id.into() }).await
        }).await
    }
}

pub struct GetUserProfileHandler {
    base: BaseHandler,
}
```

## 发布为独立 Crate

通用组件可发布为独立 crate，只依赖 `rust-webapp-core`：

```toml
[dependencies]
rust-webapp-core = "0.1"
```

## 小结

好的封装让消费方一行启用，内部隐藏注册细节和配置逻辑。

下一节：[自定义 Endpoint](custom-endpoints.md)
