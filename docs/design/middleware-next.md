# 中间件统一 `Next` 语义（D11，v2.4-4）

## 为什么

旧中间件模型只有 `invoke`（前置）+ `after`（后置）两段，能表达"短路"和"善后"，但
无法表达 **包裹** 语义：超时、重试、熔断、重排响应都需要"围绕剩余链路执行自己的逻辑"。
`Next` continuation 把剩余链路变成显式句柄，与 ASP.NET Core 的 `RequestDelegate` 同构。

## 两种中间件并存

```rust
#[async_trait::async_trait]
pub trait IMiddleware: Send + Sync {
    // 旧式：前置
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>>;
    // 旧式：后置（默认 no-op）
    async fn after(&self, _ctx: &mut dyn IHttpContext) -> Result<()> { Ok(()) }

    // 新式（2.4-4）：声明后引擎走 continuation 执行器
    fn continuation(&self) -> bool { false }
    // 默认实现 = 旧语义桥接：Break 短路；Continue 执行 next 再跑 after
    async fn invoke_next(&self, ctx: &mut dyn IHttpContext, next: NextStage) -> Result<()> { ... }
}
```

- **旧式中间件零改动**：默认 `invoke_next` 桥接 `invoke`/`after`，语义与 2.3 完全一致
  （含 `Break` 短路、错误时反向 `after` 仍然执行、首个错误优先）。
- **新式中间件**：`continuation() -> true` 并实现 `invoke_next`；`next.run(ctx).await`
  推进剩余链路，不调用即短路；`NextStage` 只能被推进一次（`run` 消费句柄）。
- **引擎按需选择执行器**：`Pipeline::new` 在构建期检测 `continuation()`。默认管道
  （所有内置/旧式中间件）仍走**零分配**的旧循环，分配预算不受影响；只要注册了一个
  新式中间件，整条管道改用 continuation 执行器（每阶段一个 boxed future，配置期已
  文档化）。这是有意的取舍：不为未使用者付费。

## 错误与响应顺序

错误在下游出现的**位置**立即经 `IErrorResponseWriter` 转成响应（而不是等到整条链
返回），因此外层旧式中间件的 `after` 看到真实状态码：

```text
continuation-before → legacy-before → handler Err(500) → [error → response]
                    → legacy-after（观察到 500）→ continuation-after
```

`after` 钩子失败不会掩盖已存在的错误（记录日志，保持首个错误）。短路（`Break` 或不
推进 `next`）时，已进入的旧式中间件仍会反向执行 `after`。

## 组合示例：超时（内置 `TimeoutMiddleware`）

```rust
let host = Host::builder()
    .use_middleware_with(|| Arc::new(TimeoutMiddleware::new(Duration::from_secs(2))))
    .build();
```

`TimeoutMiddleware` 是 continuation 风格：`tokio::time::timeout(2s, next.run(ctx))`，
超时则取消处理器并写 `504`。重试/熔断按同样形状实现——重试 = 循环调用携带状态；
熔断 = 在调用 `next` 前检查失败计数，拒绝时直接写 `503` 并返回。

按路由组合：把 continuation 中间件注册在目标路由组之前（管道顺序 = 中间件顺序），
或用路径判断在 `invoke_next` 里决定是否推进 `next`。路由级独立管道不在 2.4 范围内。

## 迁移指南

| 旧写法 | 新写法 | 说明 |
|---|---|---|
| 无需改动 | `invoke`/`after` | 默认桥接，零迁移 |
| 前置逻辑需包裹后置逻辑（如计时） | `invoke_next` + `next.run` | `after` 仍可用，但推荐新式 |
| 需要超时/重试/熔断 | `continuation()=true` + `invoke_next` | 见 `TimeoutMiddleware` |
| 写响应后需要停止 | `ctx.response_mut().set_status(...)`; 不推进 `next` | 等同旧式 `Break` |

## 实现位置与验收

- trait：`crates/core/src/middleware.rs`（`NextStage`、`BoxedNextFuture`、`TimeoutMiddleware`）
- 引擎：`crates/engine/src/pipeline.rs`（`has_continuation` 分派、`run_chain`）
- 契约测试：`crates/webx/tests/middleware_next_test.rs`（4 项：onion 顺序、短路、
  504 超时取消、错误响应先于 `after` 钩子）
- 默认管道分配预算不变（`alloc-probe compare 500 18`，见 `docs/design/alloc-budget.md`）

## 边界

- continuation 执行器需要引擎上下文（`IHttpContext::as_any_mut` 下转型）；替代宿主
  若不能提供具体上下文，终端返回 typed error（不会静默降级）。
- 新式中间件之间的错误转换发生在各自 `invoke_next` 边界；若自定义实现自行吞错，
  顺序由其负责（文档化契约）。
- `TimeoutMiddleware` 取消的是剩余链路 future；处理器须是取消安全的（无外部副作用
  的 async 代码）。取消安全边界同 tokio 语义。
