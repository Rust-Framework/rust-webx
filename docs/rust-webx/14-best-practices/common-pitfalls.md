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

## 排查流程

```
遇到错误 → 读错误消息 → 查本表 → 查对应章节 → 参考 Docbit 源码
```

## 小结

90% 的问题出在 Handler 注册和类型绑定上，掌握规则 1-5 可避免大部分陷阱。

下一节：[性能优化技巧](performance-tips.md)
