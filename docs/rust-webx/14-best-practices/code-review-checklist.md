# 代码审查清单

## 路由与契约

- [ ] `#[get]` 等标注在 `impl IRequest<T>` 块上
- [ ] `IRequest<T>` 的 T 与 Handler 返回类型一致
- [ ] 请求类型派生了 `WebxRequestMeta`
- [ ] 写方法的路径参数用 `#[frompath]` 显式声明
- [ ] 需要登录的端点用 `#[fromclaim] CurrentUser`，不在 Handler 里解析 claims
- [ ] 租户标识取自 `user.tenant_id`，不取自请求参数
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
- [ ] Service 实现在 `handlers/`，`#[inject]` 标注在 `impl Trait for Type` 上
- [ ] 无独立 `services/` 目录
- [ ] `domain/` 不依赖 host，可引用 `contracts` 枚举/model 与框架核心原语
- [ ] `main.rs` 不手动注册业务 Service

## 安全

- [ ] 敏感端点有 `#[authorize]`；多规则 AND 语义符合预期（any-of 用 `roles = [...]`）
- [ ] 密码不明文存储
- [ ] JWT Secret 不在代码中硬编码（使用 appsettings 或环境变量）
- [ ] 输入有校验

## 端点元数据与国际化

- [ ] 端点名称/描述用 `#[display]` / `#[description]`，分组用 `#[category]`；不在别处复制一份
- [ ] 权限同步/管理菜单通过 `IEndpointCatalog` 读取，不静态复制路由表
- [ ] 需要多语言时 `Culture.Default` 与 `locales/<culture>.json` 匹配，且资源键已补齐

## 测试

- [ ] 关键 Handler 有单元测试
- [ ] 错误路径有测试覆盖
- [ ] Service 可通过 mock `I…Service` 替换测试

## 小结

将此清单加入 PR 模板，确保代码质量与分层规范一致。

下一章：[案例研究 Docbit](../15-case-study/INDEX.md)
