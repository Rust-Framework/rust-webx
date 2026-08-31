# 事件发布与订阅

## 定义事件

```rust
#[derive(Clone)]
pub struct UserCreatedEvent {
    pub user_id: String,
    pub email: String,
}

impl IEventRequest for UserCreatedEvent {}
```

## 定义事件处理器

```rust
pub struct SendWelcomeEmailHandler;

#[async_trait]
impl IEventHandler<UserCreatedEvent> for SendWelcomeEmailHandler {
    async fn handle(&self, event: UserCreatedEvent) -> Result<()> {
        tracing::info!("Sending welcome email to {}", event.email);
        Ok(())
    }
}
```

## 发布事件

```rust
async fn handle(&self, req: CreateUserRequest) -> Result<UserDto> {
    let user = self.repo.create(&req).await?;

    self.mediator.publish(UserCreatedEvent {
        user_id: user.id.clone(),
        email: user.email.clone(),
    }).await?;

    Ok(user)
}
```

## 多处理器

同一事件可有多个 `IEventHandler<T>` 实现，`publish()` 广播到所有已注册的处理器。

## 与直接调用的选择

| 场景 | 推荐 |
|------|------|
| 创建用户后发邮件、写日志、更新缓存 | `publish()` 事件 |
| 需要返回值 | `mediator.send()` 请求 |
| 同步强依赖 | 直接调用 Service |

## 小结

事件系统实现模块间松耦合通信，发布方不感知有多少订阅者。

下一章：[认证与授权](../09-auth-security/INDEX.md)
