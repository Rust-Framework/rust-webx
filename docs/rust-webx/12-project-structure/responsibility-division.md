# 职责归属与边界

## 职责矩阵

| 层 | 负责 | 不负责 |
|----|------|--------|
| contracts | 路由声明、Request/Response DTO、enum、`I…Service` trait、授权元数据 | 业务实现、数据库访问 |
| handlers | `IRequestHandler` 实现、`I…Service` 实现、用例编排 | HTTP 细节、路由定义 |
| domain | 实体、值对象、迁移、EF 配置 | 框架依赖、Handler 逻辑 |
| main/startup | DI 组装（基础设施）、Host 配置、`IHostedService` | 业务逻辑、业务 Service 手动注册 |
| appsettings.json | 框架运行时配置 | 业务密钥（应使用环境变量） |

## 判断规则

**这段代码该放哪？**

```
是否定义 API 路由、DTO 或 I…Service trait？  → contracts
是否实现 IRequestHandler 或 I…Service？       → handlers
是否是持久化实体或数据库迁移？                  → domain
是否是应用启动/初始化？                        → startup
是否是 Host / 缓存 / 认证等框架配置？           → main.rs + appsettings.json
```

## 跨层规则

- contracts **仅依赖框架**，**禁止引用** domain、handlers
- domain **可以引用** contracts（复用枚举、共享 model）
- domain **禁止引用** handlers 或框架类型
- handlers **可以引用** contracts、domain、common 基础设施
- Handler **只依赖** `Arc<dyn I…Service>`，不依赖具体实现类型

## 面向接口开发

新增业务能力的标准顺序：

1. `contracts/` — 定义 DTO、enum、`I…Service` trait、`IRequest` 路由
2. `handlers/` — 实现 Service + Handler，`inject_attr(as = dyn I…Service)`
3. `domain/` — 如需持久化，添加实体与迁移
4. `main.rs` — 通常无需修改

## 违反边界的信号

- `contracts` 中有 `use crate::domain::*` → DTO 放错层
- `contracts` 中有 `async fn` 或数据库调用 → 逻辑应在 handlers
- 存在独立 `services/` 目录 → 接口应并入 contracts，实现并入 handlers
- Handler 超过 80 行 → 逻辑下沉到 Service 实现
- Handler 注入 `Arc<ConcreteService>` → 应改为 `Arc<dyn I…Service>`
- domain 中 `use rust_webx::*` → 领域被污染

## 小结

清晰的职责边界是大型项目可维护性的基础。契约层隔离对外承诺，handlers 专注履约，domain 专注数据。

下一节：[Contracts / Handlers / Domain 分层](contracts-handlers-domain.md)
