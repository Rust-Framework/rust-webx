# 代码审查清单

## 路由与契约

- [ ] `#[get]` 等标注在 `impl IRequest<T>` 块上
- [ ] `IRequest<T>` 的 T 与 Handler 返回类型一致
- [ ] 路径参数名与 Request 字段名匹配
- [ ] POST/PUT Request 实现了 `Deserialize`
- [ ] 响应类型实现了 `Serialize`

## Handler

- [ ] 注册为 `dyn IRequestHandler<T, R>`
- [ ] 有依赖时使用 `inject_attr`，无依赖使用 `#[handler]`
- [ ] Handler 内无 HTTP 直接操作（设置 header/status）
- [ ] 业务错误使用正确的 `Error` 变体
- [ ] Handler 不超过 80 行（过长则拆分）

## 安全

- [ ] 敏感端点有 `#[authorize]`
- [ ] 密码不明文存储
- [ ] JWT Secret 不在代码中硬编码
- [ ] 输入有校验

## 架构

- [ ] contracts 不含业务逻辑
- [ ] domain 不依赖框架类型
- [ ] 服务间通过 Mediator 或 Service 调用，不直接引用 Handler

## 测试

- [ ] 关键 Handler 有单元测试
- [ ] 错误路径有测试覆盖

## 小结

将此清单加入 PR 模板，确保代码质量一致性。

下一章：[案例研究 Docbit](../15-case-study/INDEX.md)
