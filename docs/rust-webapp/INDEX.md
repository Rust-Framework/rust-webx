# rust-webapp 开发者手册 · 目录

> 面向使用 rust-webapp 框架的开发者 · 渐进式披露 · 深入浅出

---

## 开篇

| 文档 | 说明 |
|------|------|
| [前言](FOREWORD.md) | 本书定位、读者画像、阅读路径 |

---

## 第一部分 · 入门与认知

### [第一章 认识 rust-webapp](01-introduction/INDEX.md)

- [什么是 rust-webapp](01-introduction/what-is-rust-webapp.md)
- [适用场景与边界](01-introduction/who-should-use.md)
- [生态与 Crate 全景](01-introduction/ecosystem-overview.md)

### [第二章 快速上手](02-quickstart/INDEX.md)

- [创建项目与依赖](02-quickstart/create-project.md)
- [Hello World 详解](02-quickstart/hello-world.md)
- [第一个 CRUD API](02-quickstart/first-crud.md)
- [运行、调试与验证](02-quickstart/run-and-debug.md)

---

## 第二部分 · 设计思想与架构

### [第三章 设计理念与哲学](03-philosophy/INDEX.md)

- [核心设计原则](03-philosophy/design-principles.md)
- [ASP.NET Core 的启发](03-philosophy/aspnet-inspiration.md)
- [Rust 惯用法与类型安全](03-philosophy/rust-idioms.md)
- [渐进式披露与框架边界](03-philosophy/progressive-disclosure.md)

### [第四章 架构全景](04-architecture/INDEX.md)

- [Crate 分层结构](04-architecture/crate-layout.md)
- [请求生命周期](04-architecture/request-lifecycle.md)
- [分层模型与依赖方向](04-architecture/layering-model.md)
- [编译时扫描机制](04-architecture/compile-time-scan.md)

---

## 第三部分 · 核心开发模式

### [第五章 请求即端点](05-request-pattern/INDEX.md)

- [IRequest 与 IRequestHandler](05-request-pattern/irequest-irequesthandler.md)
- [路由宏详解](05-request-pattern/route-macros.md)
- [Handler 注册策略](05-request-pattern/handler-registration.md)
- [参数绑定与序列化](05-request-pattern/parameter-binding.md)
- [错误处理与 ProblemDetails](05-request-pattern/error-handling.md)

### [第六章 DI 与生命周期](06-di-lifecycle/INDEX.md)

- [ServiceCollection 与服务注册](06-di-lifecycle/service-collection.md)
- [依赖注入模式](06-di-lifecycle/injection-patterns.md)
- [IHostedService 后台服务](06-di-lifecycle/hosted-services.md)
- [模块系统与 inject 宏](06-di-lifecycle/module-system.md)

### [第七章 中间件管道](07-middleware/INDEX.md)

- [管道模型与执行顺序](07-middleware/pipeline-model.md)
- [内置中间件一览](07-middleware/built-in-middleware.md)
- [自定义中间件](07-middleware/custom-middleware.md)
- [中间件编排策略](07-middleware/ordering-strategy.md)

### [第八章 中介者与事件](08-mediator-events/INDEX.md)

- [IMediator 请求调度](08-mediator-events/mediator-pattern.md)
- [IPipelineBehavior 拦截链](08-mediator-events/pipeline-behaviors.md)
- [事件发布与订阅](08-mediator-events/event-pub-sub.md)

---

## 第四部分 · 安全、配置与生产

### [第九章 认证与授权](09-auth-security/INDEX.md)

- [JWT Bearer 认证](09-auth-security/jwt-authentication.md)
- [基于资源的授权](09-auth-security/resource-authorization.md)
- [authorize 宏与声明式授权](09-auth-security/authorize-macro.md)
- [安全最佳实践](09-auth-security/security-best-practices.md)

### [第十章 配置与环境](10-configuration/INDEX.md)

- [appsettings.json 配置体系](10-configuration/appsettings.md)
- [AppMode 与环境切换](10-configuration/app-modes.md)
- [自定义配置节](10-configuration/custom-options.md)

### [第十一章 生产级能力](11-production/INDEX.md)

- [CORS、TLS 与健康检查](11-production/cors-tls-health.md)
- [缓存与速率限制](11-production/caching-rate-limit.md)
- [OpenAPI 与 SPA 托管](11-production/openapi-spa.md)
- [优雅关闭与可观测性](11-production/graceful-shutdown.md)

---

## 第五部分 · 工程化与进阶

### [第十二章 项目组织与职责划分](12-project-structure/INDEX.md)

- [推荐目录结构](12-project-structure/directory-layout.md)
- [职责归属与边界](12-project-structure/responsibility-division.md)
- [Contracts / Handlers / Domain 分层](12-project-structure/contracts-handlers-domain.md)
- [测试策略](12-project-structure/testing-strategy.md)

### [第十三章 扩展与自定义封装](13-extensibility/INDEX.md)

- [自定义组件封装](13-extensibility/custom-components.md)
- [自定义 Endpoint](13-extensibility/custom-endpoints.md)
- [样式与约定封装](13-extensibility/style-patterns.md)
- [第三方库集成](13-extensibility/third-party-integration.md)

### [第十四章 最佳实践](14-best-practices/INDEX.md)

- [常见陷阱与排查](14-best-practices/common-pitfalls.md)
- [性能优化技巧](14-best-practices/performance-tips.md)
- [AI 友好开发模式](14-best-practices/ai-friendly-development.md)
- [代码审查清单](14-best-practices/code-review-checklist.md)

---

## 第六部分 · 案例与迁移

### [第十五章 案例研究：Docbit](15-case-study/INDEX.md)

- [Docbit 项目概览](15-case-study/docbit-overview.md)
- [架构与模块划分](15-case-study/docbit-architecture.md)
- [可复用的模式提炼](15-case-study/docbit-patterns.md)

### [第十六章 迁移指南](16-migration/INDEX.md)

- [从 ASP.NET Core 迁移](16-migration/from-aspnet-core.md)
- [从 Axum / Actix 迁移](16-migration/from-axum-actix.md)
- [概念对照表](16-migration/concept-mapping.md)
