# 配置热更新（IOptionsMonitor）

`HostBuilder::add_options::<T>` 只在启动时绑定一次 appsettings。需要"改配置不重启"时，用
`IOptionsMonitor<T>`：当前快照放在 `ArcSwap` 里，替换时通过 `tokio::sync::watch` 通知订阅者。
热路径只做一次无锁 `Arc` 读取，不查库、不持全局锁。

## 契约

| 项 | 值 |
|----|----|
| 读取 | `IOptionsMonitor::current() -> Arc<T>`（无锁，热路径可用） |
| 订阅 | `subscribe() -> watch::Receiver<Arc<T>>`，每次替换后 `changed()` 就绪 |
| 替换 | 整个不可变快照替换，不原地修改；读方只会看到旧值或新值 |
| 注册 | `add_options_monitor::<T>("Section")`（启动快照）/ `watch_options_file`（文件轮询） |
| 注入 | `Arc<dyn IOptionsMonitor<T>>` 或 `Arc<OptionsMonitor<T>>` |

## 启动快照 + 文件热加载

```rust
use std::time::Duration;
use webx::*;

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct GatewayConfig {
    #[serde(default)]
    pub rpm: u64,
    #[serde(default)]
    pub model: String,
}

Host::builder()
    .watch_options_file::<GatewayConfig>(
        "Gateway",                 // appsettings 节名 / 文件内的节名
        "config/gateway.json",     // 允许在运行时修改的文件
        Duration::from_secs(5),    // 轮询间隔
    )
    .build();
```

Handler/service 注入端口：

```rust
#[derive(Inject)]
pub struct ChatHandler {
    #[inject]
    config: Arc<dyn IOptionsMonitor<GatewayConfig>>,
}

impl ChatHandler {
    fn rpm(&self) -> u64 {
        self.config.current().rpm // 热路径：一次 Arc clone，无 I/O
    }
}
```

只想要启动快照、由应用自己推送新值时，用 `add_options_monitor`；任何持有
`Arc<OptionsMonitor<T>>` 的地方（后台任务、管理端点）都可以替换快照：

```rust
// 注入 Arc<OptionsMonitor<GatewayConfig>> 后
monitor.set(GatewayConfig { rpm: 1_000, model: "fast".into() });
```

> 两个方法都会注册 monitor，选一个即可。热加载文件的根节点与 appsettings 同形，watcher 取其中的
> `Gateway` 节；`"Gateway"` 参数与 `add_options` 的节名一致。`"."` / 空串表示绑定整个文件。

## 文件 watcher 语义

| 情况 | 行为 |
|------|------|
| 首次 tick | 在**一个 interval 之后**执行；之前使用 appsettings 启动快照 |
| 文件不存在 | 不算错误，保留当前快照（允许先启动、后投放文件） |
| 内容未变 | 按字节 hash 跳过，不发布、不通知 |
| JSON 非法 / 节绑定失败 | 返回 typed `Error::Validation`，worker 记录日志；**旧快照继续生效** |
| 修好文件 | 内容 hash 变化，下一 tick 重新尝试 |
| 读取失败（权限/IO） | 返回 `Error::Internal`，旧快照继续生效 |

"坏配置不生效、不宕机"是刻意的：热更新失败应退化为"继续用上一版"，而不是把运行中的服务打挂。

## 订阅变更

```rust
let mut updates = self.config.subscribe();
tokio::spawn(async move {
    while updates.changed().await.is_ok() {
        let next = updates.borrow().clone();
        tracing::info!(rpm = next.rpm, "gateway config reloaded");
    }
});
```

- 订阅者总是从**当前**快照开始；晚订阅不会重放历史。
- 消费慢时 `watch` 只保留最新值：读 `borrow()` 即最新，不会积压。
- 没有订阅者时替换仍然生效（`send_replace`），之后的订阅者拿到新值。

## 生效范围与边界

- 已实现：通用 options 快照、文件轮询、变更通知、typed 失败、`add_options_monitor` / `watch_options_file`。
- 配置所有权在应用：watcher 只负责"读文件 → 绑定 → 发布"，不校验业务约束；需要校验时订阅变更并
  自行拒绝（保留旧快照）。
- 不监听环境变量：`APP__*` 只在启动时合并；运行期改环境变量没有意义。
- 目录/价格/key 的 watcher 可用同一套 `IOptionsMonitor` 承载；`CatalogHandle` 仍是模型目录的
  无锁快照（见 [目录热替换](hot-config.md)）。
- 多实例部署时，每个实例各自轮询文件（或由控制面广播后调用 `monitor.set`），最终一致。

## 测试

`crates/host/src/options_watch.rs` 覆盖：节绑定、内容未变不发布、文件缺失保留旧值、坏 JSON 拒绝且
不丢旧快照、周期 tick。`crates/core/src/options_monitor.rs` 覆盖快照读取、订阅通知、晚订阅与无订阅者
替换。
