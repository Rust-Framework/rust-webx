# 目录热替换（CatalogHandle）

模型/provider/别名变更不应要求重启网关。`CatalogHandle` 用 `ArcSwap` 持有不可变
`ModelCatalog` 快照；router 与 executor 共享同一个 handle。

```rust
use std::sync::Arc;
use webx_ai::{CatalogHandle, CatalogRouter, GatewayExecutor};

let handle = CatalogHandle::new(initial_catalog);
let router = Arc::new(CatalogRouter::from_handle(handle.clone()));
let executor = GatewayExecutor::new_with_handle(handle.clone(), router)
    .with_quota(quota)
    .with_usage(usage);

// 配置 watcher / 管理端加载新目录后：
let new_catalog = Arc::new(build_catalog(version + 1));
assert!(handle.reload(new_catalog));
```

## 语义

- `handle.current()` 无锁返回 `Arc<ModelCatalog>`；热路径不查库、不持锁。
- `reload` 只接受 `version >= 当前`；旧版本返回 `false`，当前快照不变。
- `CatalogRouter::plan` 与 `GatewayExecutor` 的 provider 解析读同一 handle，因此别名、provider、
  能力矩阵同时生效。
- 正在执行的请求使用它开始时读到的快照；新请求看到新快照。
- `HealthRegistry` 不随目录替换重置；同一 `ModelKey` 的健康/冷却延续。
- 版本号是单调诊断值；配置来源应保证每次成功构建都 +1。

## 装配顺序

1. 构建 `ModelCatalog`（version N）。
2. `CatalogHandle::new(Arc::new(catalog))`。
3. `CatalogRouter::from_handle(handle.clone())`。
4. `GatewayExecutor::new_with_handle(handle.clone(), router)`。
5. 注册 executor 到 DI；把 handle 交给配置 watcher。
6. watcher 构建 N+1 → `handle.reload`。

## 文件热加载（CatalogFileWatcher）

`rust-webx-ai` 提供 `CatalogFileWatcher`：轮询一个 JSON manifest，校验后构建新快照并
`reload`。provider 对象不能序列化，因此 manifest 只声明 **models / capabilities / aliases**，
provider 必须已在运行中的 catalog 里。

```json
{
  "version": 7,
  "models": [
    { "provider": "openai", "model": "gpt-4o-mini", "context_window": 128000,
      "capabilities": { "streaming": true, "tools": true } }
  ],
  "aliases": [
    { "name": "fast", "strategy": "weighted",
      "targets": [{ "provider": "openai", "model": "gpt-4o-mini", "weight": 3 }] }
  ]
}
```

```rust
use webx_ai::CatalogFileWatcher;

let watcher = CatalogFileWatcher::new(
    "config/catalog.json",
    handle.clone(),
    Duration::from_secs(5),
);
// 作为后台服务注册（PeriodicBackgroundService）：
// svc.add_background_service_with(move |_| Arc::new(watcher.clone()))
```

语义：

- `version` 必须大于当前版本；省略时 watcher 用 `current + 1`，保证每次文件变更只应用一次。
- 未知 provider、别名指向不存在的 model、重复 model/alias、空 alias 都在发布前拒绝。
- 文件内容 hash 去重；坏 JSON/校验错误记录后**保留旧快照**，修好文件后下一 tick 重试。
- 文件缺失不算错误（等待首次投放）。
- watcher 是 `PeriodicBackgroundService`，复用宿主循环/取消/join。

## 边界

- 已实现：目录快照热替换、版本防回退、router/executor 共享、manifest 文件 watcher。
- 通用配置热加载：`IOptionsMonitor<T>` + `HostBuilder::watch_options_file`（见
  [配置热更新](live-options.md)）。
- 未实现：多实例版本轮询、canary 校验、配额策略/价格/key 轮换（应用/控制面实现）。
- 失败处理：保留旧快照即可；不要清空 catalog 再加载，否则中间请求会 404/503。
- 不要在每个请求里重建 `CatalogHandle`；它应在应用生命周期内单例。
