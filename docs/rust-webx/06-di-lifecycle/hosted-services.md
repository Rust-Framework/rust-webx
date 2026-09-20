# IHostedService 后台服务

## 接口定义

```rust
#[async_trait]
pub trait IHostedService: Send + Sync {
    async fn start(&self) -> Result<()>;
    async fn stop(&self) -> Result<()> { Ok(()) }  // 默认空实现
}
```

## 生命周期时机

```mermaid
graph LR
    A[Host::run] --> B[start × N 按注册顺序]
    B --> C[HTTP 监听启动]
    C --> D[服务运行中]
    D --> E[收到 shutdown 信号]
    E --> F[stop × N 按注册顺序]
    F --> G[连接排空]
```

- `start()` 在 HTTP 监听器启动**之前**执行
- `stop()` 在优雅关闭时按**与启动相同的顺序**执行
- `stop()` 有默认空实现，不需要关闭逻辑可省略

## 典型用途

| 用途 | 示例 |
|------|------|
| 数据库初始化 | `ctx.ensure_created()` 建表 |
| 种子数据 | 默认管理员账户 |
| 索引生成 | DocService 扫描 docs/ |
| 连接池预热 | 预建立数据库连接 |
| 后台消费者 | 消息队列轮询 |

## Docbit 实例

`#[derive(Inject)]` 标注 struct 生成构造器，`#[inject]` 标注 `impl IHostedService` 块把实现注册为
`dyn IHostedService`：

```rust
#[derive(Inject)]
pub struct DbInitService {
    #[inject]
    docs: Arc<dyn IDocumentService>,
}

#[inject]
#[async_trait]
impl IHostedService for DbInitService {
    async fn start(&self) -> Result<()> {
        // 请求作用域由框架建立，Scoped 依赖在此解析
        let mut ctx: DbContext = dispatch_provider()
            .get_owned()
            .map_err(|e| Error::Internal(format!("DbContext resolution failed: {}", e)))?;

        ensure_schema(&mut ctx).await?;             // 建表
        admin_user::ensure_admin_user(&mut ctx).await?;  // 种子数据
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        tracing::info!("Shutting down.");
        Ok(())
    }
}
```

替代在 `main()` 中显式调用初始化函数——生命周期由框架统一管理。

## 注册

```rust
// 在 impl IHostedService 上标注 #[inject]，通过 inventory 自动注册
#[inject]
#[async_trait]
impl IHostedService for DbInitService { ... }

// 或手动注册（要求类型实现 Default）
Host::builder()
    .register(|svc| svc.add_hosted_service::<CacheWarmupService>())
```

## 多个 HostedService

按注册顺序启动，也按注册顺序停止：

```
注册顺序: DbInit → CacheWarmup → QueueConsumer
启动顺序: DbInit → CacheWarmup → QueueConsumer
停止顺序: DbInit → CacheWarmup → QueueConsumer
```

## 小结

`IHostedService` 是应用初始化和清理的标准钩子，等价于 ASP.NET Core 的同名接口。

下一节：[模块系统与 inject 宏](module-system.md)
