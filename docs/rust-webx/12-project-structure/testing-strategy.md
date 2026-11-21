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
cargo test -p rust-webx-host
```

测试覆盖：路由匹配、中间件管道、认证授权、缓存、CORS 等。

### 应用级集成测试

```rust
// tests/api_test.rs
use webx::*;

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

## 进程内测试基座（2.4-5）

`webx::TestHost` 在**不绑定端口**的情况下构建真实 host（同一 DI 容器、HandlerCache、
Mediator 分发路径），直接分发 typed 请求；时钟用 `webx::TestClock` 控制，故障用
`ChaosControl` 注入：

```rust
let clock = Arc::new(TestClock::from_unix(1_700_000_000));
let host = TestHost::from_builder_with_clock(
    Host::builder().register(|svc| svc.add_mediator()),
    Arc::clone(&clock),
);
let reply = host.dispatch(MyRequest { .. }).await?;   // 无 socket、无 HTTP 编解码
clock.advance(Duration::from_secs(60));                // handler 注入的 IClock 同步前进
host.chaos().fail_next(1);                             // 下一次 dispatch typed 503
host.shutdown();
```

- Handler 用 `#[inject] clock: Arc<dyn IClock>` 读取时间，别直接 `SystemTime::now()`；
- 超时/退避等单调时间仍用 tokio 定时器，配 `#[tokio::test(start_paused = true)]`；
- 需要路由/绑定/状态码等字节级行为时用 `webx::spawn`（临时端口 + 健康探测）。

详见 [TestHost 设计](../../design/test-host.md)。
