# 代码审查清单

## 路由与契约

- [ ] `#[get]` 等标注在 `impl IRequest<T>` 块上
- [ ] `IRequest<T>` 的 T 与 Handler 返回类型一致
- [ ] 路径参数名与 Request 字段名匹配
- [ ] POST/PUT Request 实现了 `Deserialize`
- [ ] 响应类型实现了 `Serialize`
- [ ] Response DTO 定义在 `contracts/`，非 `domain/`

## Handler

- [ ] 注册为 `dyn IRequestHandler<T, R>`
- [ ] 有依赖时使用 `#[inject]` + `#[handler(inject)]`，无依赖使用 `#[handler]`
- [ ] Handler 内无 HTTP 直接操作（设置 header/status）
- [ ] 业务错误使用正确的 `Error` 变体
- [ ] Handler 不超过 80 行（过长则下沉到 Service 实现）
- [ ] 注入 `Arc<dyn I…Service>`，非具体实现类型

## 分层与依赖

- [ ] `contracts/` 仅依赖框架，无 `use crate::domain::*`
- [ ] `I…Service` trait 定义在 `contracts/`
- [ ] Service 实现在 `handlers/`，带 `#[inject] (implements I…Service)`
- [ ] 无独立 `services/` 目录（或仅为迁移中的临时结构）
- [ ] `domain/` 不依赖框架类型，可引用 `contracts` 枚举/model
- [ ] `main.rs` 不手动注册业务 Service

## 安全

- [ ] 敏感端点有 `#[authorize]`
- [ ] 密码不明文存储
- [ ] JWT Secret 不在代码中硬编码（使用 appsettings 或环境变量）
- [ ] 输入有校验

## 测试

- [ ] 关键 Handler 有单元测试
- [ ] 错误路径有测试覆盖
- [ ] Service 可通过 mock `I…Service` 替换测试

## 小结

将此清单加入 PR 模板，确保代码质量与分层规范一致。

下一章：[案例研究 Docbit](../15-case-study/INDEX.md)
