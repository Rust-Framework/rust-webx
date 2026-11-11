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
    E --> F[连接排空]
    F --> G[stop × N 逆序]
```

- `start()` 在 HTTP 监听器启动**之前**执行；任一服务 `start()` 失败时，已启动的服务按**逆序**回滚 `stop()`，然后 `run()` 返回错误
- 1.0 起没有 ambient DI：服务若需要 Scoped 依赖（`DbContext`、事务、连接），注入 rust-dix 内建的
  `#[inject] services: Arc<dyn ScopeFactory>`，在 `start()`/后台循环里显式
  `let scope = services.create_scope(); let mut ctx: DbContext = scope.get_owned()?;`，scope 释放即释放依赖
- Host 停机顺序：连接排空 → `stop()` 逆序 → **`provider.dispose()`**（rust-dix 逆序释放 singleton 并标记 disposed）
- 监听器**先绑定后启动**：端口占用等绑定错误让 `run()`/`run_at()` 直接返回 `Err`（进程非 0 退出），不会静默"启动成功"
- `stop()` 在优雅关闭、连接排空之后按**启动的逆序**执行（最后启动的最先停止），每个服务最多等待 30s，超时只记日志、不阻塞进程
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
    /// rust-dix 内建：后台任务自己开 scope
    #[inject]
    services: Arc<dyn ScopeFactory>,
}

#[inject]
#[async_trait]
impl IHostedService for DbInitService {
    async fn start(&self) -> Result<()> {
        // 显式 DI scope：释放 scope 即释放 Scoped 依赖（不落 root）
        let scope = self.services.create_scope();
        let mut ctx: DbContext = scope
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

按注册顺序启动，按**逆序**停止（依赖方向：先启动的最晚停止）：

```
注册顺序: DbInit → CacheWarmup → QueueConsumer
启动顺序: DbInit → CacheWarmup → QueueConsumer
停止顺序: QueueConsumer → CacheWarmup → DbInit
```

## 小结

`IHostedService` 是应用初始化和清理的标准钩子，等价于 ASP.NET Core 的同名接口。

下一节：[模块系统与 inject 宏](module-system.md)
