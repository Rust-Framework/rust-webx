# 模块系统与 inject 宏

## rust-dicore 模块

大型项目可用 `#[module]` 宏组织 DI 注册：

```rust
use rust_dicore_macros::module;

#[module]
pub mod UserModule {
    pub use handlers::*;
    pub use services::*;
}
```

模块自动收集 `inject_attr` 标注的服务并注册到容器。

## inject 宏

```rust
use rust_dicore::inject;

#[inject(singleton)]
pub struct EmailService {
    smtp: SmtpConfig,
}
```

等价于手写 `svc.singleton::<EmailService>(...)` 。

## Inject derive

```rust
#[derive(Inject)]
#[inject(singleton)]
pub struct NotificationService {
    email: Arc<EmailService>,
}
```

自动为 struct 生成 DI 注册代码。

## 与 rust-webx 集成

伞 Crate 重新导出 DI 工具：

```rust
use rust_webx::{inject, inject_attr, module, Inject};
```

在 Handler 中：

```rust
#[inject_attr(singleton, as = dyn IRequestHandler<...>)]
pub struct MyHandler {
    service: Arc<MyService>,
}
```

## 组合根原则

无论使用哪种注入方式，**只有一个组合根**（`main.rs` 的 `register()` 或 `#[module]` 入口）。业务代码不应自行创建服务实例。

## 小结

`inject_attr` + `#[handler(inject)]` 是 Docbit 验证的生产模式，兼顾简洁与可测试性。

下一章：[中间件管道](../07-middleware/INDEX.md)
