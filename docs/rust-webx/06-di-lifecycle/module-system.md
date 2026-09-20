# 模块系统与 inject 宏

## rust-dix 模块

大型项目可用 `#[module]` 宏组织 DI 注册（伞 crate 重新导出了该宏）：

```rust
use webx::{module, rust_dix};

#[module]
pub mod user_module {
    rust_dix::register!(singleton: dyn IUserService => UserService);
    rust_dix::register!(factory singleton: Clock => SystemClock::system());
}
```

`#[module]` 只做一件事：把块内的 `register!(...)` 声明展开成一个
`__rdi_build_provider_<模块名>()` 构建函数。它**不会**收集 `#[inject]` 标注的类型——
`#[inject]` 的注册独立于 `#[module]`（见下）。

`register!` 的常用形式：

| 形式 | 含义 |
|------|------|
| `singleton: Type` | 注册 `Type`（要求 `Default`） |
| `singleton: dyn Trait => Impl` | 以 trait object 注册实现 |
| `factory singleton: Type => expr` | 用表达式构造值 |
| `keyed "name": singleton: Type` | 注册带 key 的服务 |

## inject 宏（属性宏）

```rust
use webx::inject;

#[inject(singleton)]
pub struct EmailService {
    #[inject]
    smtp: Arc<SmtpConfig>,
}
```

`#[inject]` 是**属性宏**：生成构造函数，**并**向 inventory 提交 `ServiceRegistration`。
`ServiceCollection::from_injected()` 在 `Host::build()` 时收集这些注册。

字段只有在标注 `#[inject]` 时才从容器解析（`Arc<T>` / `Option<Arc<T>>` / `Vec<Arc<T>>`，
或用 `#[inject(owned)]` 解析 bare `T`）；未标注的字段一律取 `Default::default()`。

## Inject derive

`#[derive(Inject)]` 只生成构造函数（`__rdi_construct_*` / `__rdi_try_construct_*`），
**不**提交 DI 注册：

```rust
#[derive(Inject)]
pub struct NotificationService {
    #[inject]
    email: Arc<EmailService>,
}
```

它常与 `#[handler(inject)]` 或 `impl` 块上的 `#[inject]` 搭配：由后者提交注册，复用 derive
生成的构造函数。**不要在同一类型上同时使用 `#[derive(Inject)]` 和 `#[inject]`**——两者都会生成
同名构造函数，导致重复定义编译错误。

## 与 rust-webx 集成

伞 Crate 重新导出 DI 工具：

```rust
use webx::{inject, module, Inject};
```

在 Handler 中：

```rust
#[derive(Inject)]
pub struct MyHandler {
    #[inject]
    service: Arc<dyn IMyService>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<MyRequest, MyResponse> for MyHandler { ... }
```

完整说明见 [依赖注入模式](injection-patterns.md)。

## 组合根原则

无论使用哪种注入方式，**只有一个组合根**（`main.rs` 的 `register()` 或 `#[module]` 入口）。业务代码不应自行创建服务实例。

## 小结

`#[derive(Inject)]` + `#[handler(inject)]` 是 Docbit 验证的生产模式，兼顾简洁与可测试性。

下一章：[中间件管道](../07-middleware/INDEX.md)
