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
    E --> F[stop × N 逆序]
    F --> G[连接排空]
```

- `start()` 在 HTTP 监听器启动**之前**执行
- `stop()` 在优雅关闭时**逆序**执行
- `stop()` 有默认空实现，不需要关闭逻辑可省略

## 典型用途

| 用途 | 示例 |
|------|------|
| 数据库迁移 | `m001_initial::up()` |
| 种子数据 | 默认管理员账户 |
| 索引生成 | DocService 扫描 docs/ |
| 连接池预热 | 预建立数据库连接 |
| 后台消费者 | 消息队列轮询 |

## Docbit 实例

```rust
#[inject_attr(singleton, as = dyn IHostedService)]
pub struct DbInitService {
    ctx: Arc<Mutex<DbContext>>,
    docs: Arc<DocService>,
}

#[async_trait]
impl IHostedService for DbInitService {
    async fn start(&self) -> Result<()> {
        // 1. 运行迁移
        // 2. 种子数据
        // 3. 生成文档索引
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
// 通过 inject_attr 自动注册
#[inject_attr(singleton, as = dyn IHostedService)]
pub struct DbInitService { ... }

// 或手动
Host::builder()
    .register(|svc| svc.add_hosted_service::<DbInitService>())
```

## 多个 HostedService

按注册顺序启动，逆序停止：

```
注册顺序: DbInit → CacheWarmup → QueueConsumer
启动顺序: DbInit → CacheWarmup → QueueConsumer
停止顺序: QueueConsumer → CacheWarmup → DbInit
```

## 小结

`IHostedService` 是应用初始化和清理的标准钩子，等价于 ASP.NET Core 的同名接口。

下一节：[模块系统与 inject 宏](module-system.md)
