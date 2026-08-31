# 全局状态迁移（Phase 4）

`rust-webx` 0.2 将 HTTP 与托管服务调度从进程级 `global_provider()` 迁到实例级 [`DispatchRuntime`](04-architecture/request-lifecycle.md)。

## 变更摘要

| 旧方式 | 新方式 |
|--------|--------|
| `global_provider().get_owned()` | `dispatch_provider().get_owned()`（在 `DispatchRuntime` 作用域内） |
| `set_global_provider()` 在 `Host::build()` 自动调用 | **不再**自动设置；仅作已弃用 shim |
| `HandlerCache::init_global()` | `HandlerCache::build()` 挂在 `Host::dispatch_runtime()` 上 |

## HTTP 请求

无需修改。`Host` 在 `handle_request` 与托管服务 `start()` 中自动 `dispatch_runtime().run(...)`，宏生成的 `RouteDispatch` 通过 `dispatch_provider()` 解析 DI。

## 集成测试与多 Host

```rust
let host = Host::builder().register(...).build();

// 直接访问根容器
let provider = host.provider();

// 在实例运行时上下文中执行（等同 HTTP / IHostedService::start）
host.dispatch_runtime()
    .run(async {
        let mediator = Mediator::new(dispatch_provider());
        let _ = mediator.send(MyRequest::default()).await;
    })
    .await;
```

## IHostedService / DbInitService

在 `start()` 内使用 `dispatch_provider()` 解析 Scoped 依赖（如 `DbContext`），不要调用 `global_provider()`：

```rust
async fn start(&self) -> Result<()> {
    let mut ctx = dispatch_provider()
        .get_owned::<DbContext>()
        .map_err(|e| Error::Internal(e.to_string()))?;
    // ...
    Ok(())
}
```

`Host::run()` 已在启动托管服务时设置 `DispatchRuntime` 作用域。

## 仍使用 `global_provider()` 的代码

1. 改为 `host.provider()` 或 `dispatch_provider()`（在 `dispatch_runtime().run` 内）。
2. 过渡期可手动调用已弃用的 `set_global_provider(provider.clone())`；`dispatch_provider()` 会打警告并回退到 shim。

`global_provider()` 与 `set_global_provider()` 已标记 `deprecated(since = "0.2.0")`。

## OpenAPI 查询参数

GET 请求的 query 参数需在请求 DTO 上 `#[derive(WebxRequestMeta)]`，并对字段标注 `#[from_query]`（path 参数用 `#[from_route]`），否则 OpenAPI 不会列出 query 参数（路由绑定仍可用）。

## JWT 密钥（`jwt_secret()`）

Phase 4 废弃的是 DI 相关的 `global_provider()`，**不是** JWT 签名密钥。`jwt_secret()` 仍为进程级 `OnceLock`，由 `add_authentication()` 在 build 时初始化。登录 Handler 签发 token 时继续使用即可。详见 [安全最佳实践 — jwt_secret() shim](../09-auth-security/security-best-practices.md#jwt_secret-进程级-shim)。

## 相关文档

- [请求生命周期](04-architecture/request-lifecycle.md)
- [ARCHITECTURE_REMEDIATION.md](../../ARCHITECTURE_REMEDIATION.md) Phase 4
