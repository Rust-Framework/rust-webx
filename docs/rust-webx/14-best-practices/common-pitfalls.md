# 常见陷阱与排查

## 1. No handler registered for request

**原因**：Handler 未注册到 DI 容器。

**排查**：
- 是否使用了 `#[handler]` 或 `#[handler(inject)]`？
- 是否手动注册为 `dyn IRequestHandler<T, R>`？
- Handler 模块是否被 `mod handlers;` 引用？

## 2. route not found (404)

**原因**：路由未正确收集。

**排查**：
- `#[get]` 是否标注在 `impl IRequest<T>` 上（不是 struct）？
- 路径是否与实际请求匹配？
- 模块是否参与编译链接（inventory 需要）？

## 3. #[handler] 编译失败

**原因**：Handler 未实现 `Default`。

**解决**：有依赖的 Handler 使用 `#[inject]` + `#[handler(inject)]`。

## 4. 类型不匹配

**原因**：`IRequest<T>` 的 T 与 `IRequestHandler<T, R>` 的 R 不一致。

```rust
// ❌
impl IRequest<String> for GetUserRequest {}
impl IRequestHandler<GetUserRequest, UserDto> for GetUserHandler {}

// ✅
impl IRequest<UserDto> for GetUserRequest {}
impl IRequestHandler<GetUserRequest, UserDto> for GetUserHandler {}
```

## 5. 注册为具体类型

```rust
// ❌
svc.singleton(|_| Arc::new(HelloHandler::default()))

// ✅
svc.singleton::<dyn IRequestHandler<HelloRequest, String>>(
    |_| Arc::new(HelloHandler::default())
)
```

## 6. 401/403 授权问题

- 是否启用了 `add_authentication()`？
- JWT Secret 是否一致？
- `#[authorize(role = "...")]` 的 role 是否与 token 中的 roles 匹配？

## 7. 多个 `#[authorize]` 不是 OR

多个 `#[authorize]` 是 **AND**：每个规则都必须满足。要表达「任一角色即可」：

```rust
#[authorize(roles = ["admin", "auditor"])]
```

`roles` / `permissions` 数组内部是 any-of；同时写 role 和 permission 时两者是 AND。

## 8. 端点文本没有生效

- 是否调用了 `.add_localization()`（约定读取 `locales/<culture>.json`）或
  `.add_localization_with(...)`？
- `Culture.Default` 是否与资源文件名匹配？
- 资源键是否在 JSON 中？缺失时按 `text` → key 回退；
- `#[display]` / `#[description]` / `#[category]` 写在路由宏前后都可以；写在 struct 上无效。

## 9. 找不到「所有路由」的 API

`webx::endpoints()` 已移除。使用：

```rust
let catalog = host.endpoints();                 // &dyn IEndpointCatalog
let all = catalog.all();                        // &[EndpointDescriptor]
```

或在 Handler / hosted service 中注入 `Arc<dyn IEndpointCatalog>`。`all()` 是语言中立元数据；
需要解析后的名称/描述/分类时用 `catalog.localized(&*localizer)`。

## 10. OpenAPI 名称/分组不对

- 名称/描述来自 `#[display]` / `#[description]`，其次 `///`，最后类型名；
- 语言来自全局 `Culture.Default`，不跟随请求头；
- `#[category]` 会成为 OpenAPI `tag`；未设置时按路径推导 tag。

## 11. 身份与租户处理

| 反模式 | 后果 | 正确写法 |
|--------|------|----------|
| 每个 Handler 里 `ctx.claims()` 再解析 `sub`/`tenant` | 样板重复，容易漏租户条件 | `#[fromclaim] user: IdentityUser` |
| 租户取自 query/body | 客户端可自选租户，数据越权 | `user.tenant_id` |
| 匿名接口用 `IdentityUser` | 未登录直接 401 | `#[fromclaim(optional)] Option<IdentityUser>` |

完整的登录、匿名、角色、多租户场景见[用户身份与多租户](../05-request-pattern/user-identity.md)。

## 排查流程

```
遇到错误 → 读错误消息 → 查本表 → 查对应章节 → 参考 Docbit 源码
```

## 小结

90% 的问题出在 Handler 注册和类型绑定上，掌握规则 1-5 可避免大部分陷阱。

下一节：[性能优化技巧](performance-tips.md)
