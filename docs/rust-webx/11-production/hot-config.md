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

## 边界

- 已实现：目录快照热替换、版本防回退、router/executor 共享。
- 通用配置热加载：`IOptionsMonitor<T>` + `HostBuilder::watch_options_file`（见
  [配置热更新](live-options.md)）；目录 watcher 可以用它承载，但"构建下一个 catalog"仍是应用逻辑。
- 未实现：目录文件的自动 watcher、配额策略/价格/key 轮换、多实例版本轮询、canary 校验。
- 失败处理：保留旧快照即可；不要清空 catalog 再加载，否则中间请求会 404/503。
- 不要在每个请求里重建 `CatalogHandle`；它应在应用生命周期内单例。
