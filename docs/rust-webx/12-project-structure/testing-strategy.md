# 测试策略

## 单元测试

直接测试 Handler，无需启动 HTTP 服务器：

```rust
#[tokio::test]
async fn test_create_user_success() {
    let handler = CreateUserHandler { repo: Arc::new(MockRepo::new()) };
    let result = handler.handle(CreateUserRequest {
        name: "Alice".into(),
        email: "alice@test.com".into(),
    }).await;
    assert!(result.is_ok());
}
```

## 集成测试

框架在 `crates/host/tests/` 提供完整集成测试范例：

```bash
cargo test -p rust-webapp-host
```

测试覆盖：路由匹配、中间件管道、认证授权、缓存、CORS 等。

### 应用级集成测试

```rust
// tests/api_test.rs
use rust_webapp::*;

#[tokio::test]
async fn test_hello_endpoint() {
    let host = Host::builder().build();
    // 使用 test client 发送请求并断言响应
}
```

## 测试金字塔

```
        /  E2E  \          少量：完整用户流程
       / 集成测试 \         中等：API 端点 + 中间件
      /  单元测试   \       大量：Handler + Service 逻辑
```

## Mock 策略

| 依赖 | Mock 方式 |
|------|----------|
| 数据库 | Mock Repository / 内存实现 |
| 外部 API | trait + Mock 实现 |
| IMediator | 直接调用 Handler 或 Mock Mediator |
| IDistributedCache | MemoryCache |

## 小结

Handler 的纯函数式设计使单元测试天然友好；框架集成测试可作为参考模板。

下一章：[扩展与自定义封装](../13-extensibility/INDEX.md)
