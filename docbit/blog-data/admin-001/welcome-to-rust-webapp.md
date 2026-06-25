## 为什么选择 rust-webapp？

在传统 Rust Web 生态中，开发者往往需要在路由、DI、中间件之间手动拼装大量样板代码。rust-webapp 借鉴 ASP.NET Core 的设计理念，将 **请求即端点** 的模式带入 Rust：

- `IRequest<T>` + `#[get("/path")]` 一行定义路由
- `#[handler]` 编译时自动注册到 DI 容器
- `IMediator` 统一调度请求与事件

## 快速体验

```bash
cargo run -p docbit
```

访问作品集首页，点击 **rust-webapp** 卡片即可查看完整文档。

## 下一步

阅读 [rust-webapp 文档](/works/rust-webapp/docs) 了解框架核心概念与 API 设计。
