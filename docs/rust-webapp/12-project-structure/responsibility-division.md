# 职责归属与边界

## 职责矩阵

| 层 | 负责 | 不负责 |
|----|------|--------|
| contracts | 路由声明、DTO 定义、授权元数据 | 业务逻辑、数据库访问 |
| handlers | 用例编排、调用服务、返回 Result | HTTP 细节、路由定义 |
| services | 领域规则、跨实体逻辑 | HTTP、路由 |
| domain | 实体、值对象、迁移 | 框架依赖 |
| main/startup | DI 组装、Host 配置、初始化 | 业务逻辑 |

## 判断规则

**这段代码该放哪？**

```
是否定义 API 路由或 DTO？        → contracts
是否响应一个 IRequest？          → handlers
是否包含可复用的业务规则？        → services
是否是持久化实体或领域概念？      → domain
是否是应用启动/初始化？          → startup
```

## 跨层规则

- contracts **不引用** handlers
- domain **不引用** 任何上层
- handlers **可以引用** contracts、services、domain
- services **可以引用** domain

## 违反边界的信号

- Handler 超过 80 行 → 逻辑下沉到 service
- contracts 中有 `async fn` → 逻辑放错了层
- domain 中 `use rust_webapp::*` → 领域被污染

## 小结

清晰的职责边界是大型项目可维护性的基础。

下一节：[Contracts / Handlers / Domain 分层](contracts-handlers-domain.md)
