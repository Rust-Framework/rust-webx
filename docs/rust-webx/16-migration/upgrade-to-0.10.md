# 升级到 0.10（破坏性变更）

0.10.0 的主题是**把扩展点从类型擦除升级为编译期类型安全**：管道行为（pipeline behavior）
从 `Box<dyn Any + Send>` 改为 `IPipelineBehavior<Req, Rsp>`，短路返回值在编译期与 Handler
的响应类型对齐；同一批还删掉了 0.6 起就不再生效的 no-op 全局 API，并新增
`HostBuilder::try_build()`。

> 本文对应 **rust-webx 0.10.0 已发布 API**（`crates/core/src/pipeline.rs`、
> `crates/core/src/route/ext.rs`、`crates/host/src/server.rs`、`crates/core/src/mediator.rs`）。

## 必须改代码的 Breaking

### 1. `IPipelineBehavior` 改为泛型 typed trait

旧 trait 用 `Box<dyn Any + Send>` 传递请求/响应，短路返回值只能在运行期 downcast；
0.10 新增泛型 `IPipelineBehavior<Req, Rsp>`，旧 trait 改名为 `IErasedPipelineBehavior`
（`#[deprecated(since = "0.10.0")]`，保留一个版本，1.0 删除）。

```rust
// 0.9：类型擦除，请求/响应都是 Box<dyn Any + Send>
#[async_trait::async_trait]
impl IPipelineBehavior for TimingBehavior {
    async fn handle(
        &self,
        req: Box<dyn Any + Send>,
        next: BoxedNextFn,
    ) -> Result<Box<dyn Any + Send>> {
        let started = Instant::now();
        let response = next(req).await;
        metrics.observe("handler", started.elapsed());
        response
    }
}
```

纯横切逻辑（计时、日志、指标，不需要读请求字段）**只需改名**，行为仍原样执行：

```rust
// 0.10：旧 trait 改名，注册也改用 deprecated 的 add_pipeline（见下一节）
#[async_trait::async_trait]
impl IErasedPipelineBehavior for TimingBehavior {
    async fn handle(
        &self,
        req: Box<dyn Any + Send>,
        next: BoxedNextFn,
    ) -> Result<Box<dyn Any + Send>> {
        let started = Instant::now();
        let response = next(req).await;
        metrics.observe("handler", started.elapsed());
        response
    }
}
```

需要读/改 typed 请求、或按业务短路时，改成 typed trait：

```rust
// 0.10：typed，泛型参数就是请求/响应类型（不是 associated type）
#[async_trait::async_trait]
impl IPipelineBehavior<CreateOrderRequest, OrderView> for RequireNote {
    async fn handle(
        &self,
        req: CreateOrderRequest,
        next: Next<CreateOrderRequest, OrderView>,
    ) -> Result<OrderView> {
        if req.note.is_empty() {
            // 短路：直接返回 Rsp，类型由编译期检查（不再是 Box::new(()) + 运行期 mismatch）
            return Err(Error::Validation("note is required".into()));
        }
        next.run(req).await
    }
}
```

- `Next<Req, Rsp>` 由框架构造，行为只调用 `next.run(req)` 继续执行剩余链与 Handler；
- 短路返回 `Rsp`：返回错误的类型是**编译错误**，而不是运行期 500；
- 一个行为只对它声明的 `<Req, Rsp>` 生效；不同请求类型注册各自的实例；
- 链顺序：先注册的最外层（请求先看到、响应后看到）。

> 以上 trait 签名与改名即 0.10.0 发布形态（`crates/core/src/pipeline.rs`）。

### 2. 注册 API：`add_typed_pipeline*`

```rust
// 0.10：typed 行为（Default 构造）
svc.add_typed_pipeline::<RequireNote, CreateOrderRequest, OrderView>();

// 0.10：构造注入 —— 工厂拿到 DI resolver；handle 内没有 resolver
svc.add_typed_pipeline_with::<_, CreateOrderRequest, OrderView>(|resolver| {
    Arc::new(RequireNote { clock: resolve_from(resolver) })
});
```

- `add_typed_pipeline::<B, Req, Rsp>()` 要求 `B: IPipelineBehavior<Req, Rsp> + Default`；
- `add_typed_pipeline_with::<F, Req, Rsp>(factory)` 的 factory 是
  `Fn(&dyn IServiceResolver) -> Arc<dyn IPipelineBehavior<Req, Rsp>>`，在构造期执行一次；
- 旧 `add_pipeline` / `add_pipeline_with` 仍可编译（`deprecated`），注册的是
  `IErasedPipelineBehavior`；新代码不要再用。

若一个请求同时有旧（erased）与新（typed）行为，**erased 链在 typed 链外层**：

```text
请求 → erased 行为（0.9 的横切逻辑）→ typed 行为（新）→ Handler → 响应反向返回
```

也就是说 erased 行为看到的是 `Box<dyn Any + Send>` 原始请求/响应，typed 行为看到的是
`Req` / `Rsp`；一条请求只会执行“匹配自己 `<Req, Rsp>` 的 typed 行为 + 全部 erased 行为”。

迁移建议：先把旧 impl 机械改名（deprecated warning 可接受一个版本），需要 typed 请求的行为
再逐个换成 `IPipelineBehavior<Req, Rsp>` + `add_typed_pipeline*`；1.0 会删除 erased trait。

> 注册函数名与泛型顺序即 0.10.0 发布形态（`crates/core/src/route/ext.rs`）。

### 3. 删除 `HostBuilder::use_resource_authorization()`

0.6 起它就是 `#[deprecated]` 的 no-op（授权由端点元数据 + `#[authorize]` 决定），0.10 直接删除：

```rust
// 0.9
Host::builder().use_resource_authorization().build();

// 0.10
Host::builder().build();   // 授权规则来自 #[authorize] 与注册的动态授权器
```

### 4. 删除进程级全局 provider shim

`set_global_provider` / `try_global_provider`（以及依赖它们的全局 handler-cache 回退）删除；
`dispatch_provider()` 只认当前任务上的 `DispatchRuntime`：

```rust
// 0.9：进程级兜底（已 deprecated，仅迁移期/测试）
webx::set_global_provider(provider.clone());
let provider = webx::global_provider();

// 0.10：Host 实例状态是唯一来源
let provider = host.provider();

// 后台任务 / 托管服务里要先进入 dispatch runtime：
host.dispatch_runtime()
    .run(async {
        let provider = webx::dispatch_provider(); // 依赖当前 DispatchRuntime
        // ...
    })
    .await;
```

没有活动 `DispatchRuntime` 时 `dispatch_provider()` 直接 panic，并在提示里给出
`Host::dispatch_runtime().run(...)` 的写法；测试请构造 runtime 或直接用 `host.provider()`。

## 新增能力（可选）

- **`HostBuilder::try_build() -> Result<Host, Error>`**：生产守卫失败（弱/占位 JWT secret、
  CORS `*`）返回 `Err`，不再只能 panic；`build()` 保持 fail-fast，内部委托 `try_build()`
  后 unwrap：

  ```rust
  let host = match Host::builder()
      .mode(AppMode::Production)
      .try_build()
  {
      Ok(host) => host,
      Err(error) => {
          eprintln!("invalid production configuration: {error}");
          std::process::exit(1);
      }
  };
  ```

- **`IMediator` 支持 `Arc<dyn IMediator>`（0.10 已交付）**：新增
  `send_boxed(Box<dyn Any + Send>) -> Result<Box<dyn Any + Send>>`，请求按 `TypeId`
  （`#[handler]` 生成的 boxed dispatch）走与 `send` 相同的 pipeline + handler 路径，调用方
  downcast 响应。泛型 `send`/`publish` 加 `where Self: Sized` 后不再进入 vtable；`publish`
  仍保持泛型（事件处理器按具体 `IEventHandler<T>` 从 DI 解析），boxed publish 不在 0.10 范围。

- 行为内不再提供 DI resolver（行为是 singleton，构造期注入）——需要依赖时用
  `add_typed_pipeline_with` 的工厂参数。

## 升级检查清单

```text
grep -rn "impl IPipelineBehavior" src        # 旧 erased 实现改名 IErasedPipelineBehavior，或改 typed
grep -rn "add_pipeline" src                  # 旧注册 deprecated 仍可跑；新行为用 add_typed_pipeline*
grep -rn "BoxedNextFn" src                   # erased 链类型；typed 链用 Next<Req, Rsp> + next.run(req)
grep -rn "use_resource_authorization" src    # 删除调用（no-op 已移除）
grep -rn "set_global_provider\|try_global_provider\|global_provider" src
                                             # 删除；改用 Host::provider() / dispatch_provider()
```

部署与代码侧：

1. 生产启动建议改用 `try_build()`，把“弱 JWT secret / CORS `*`”变成启动错误而不是 panic
   堆栈（`APP_ENV=Production` 时守卫生效）；
2. 进程级 provider 兜底删除后，后台任务必须通过 `Host::dispatch_runtime().run(...)` 或
   注入的 scope 获取依赖，`set_global_provider` 时代“先设置再随便取”的写法不再可用；
3. deprecated 的 erased pipeline 只保留一个版本（计划 1.0 移除），本轮迁移可以避免下次断裂；
4. 若需要 `Arc<dyn IMediator>` 注入，改用 `send_boxed` 并 downcast 响应；泛型
   `send`/`publish` 只在具体 `Mediator` 上调用。
