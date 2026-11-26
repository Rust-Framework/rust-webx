# 移动推送（IPushSender）

IM 的消息是**按设备令牌寻址**的：收件人没有在线 WebSocket 时，消息仍要经 APNs/FCM 到达设备。
框架只拥有端口与结果模型（`IPushSender` / `PushMessage` / `PushOutcome`），不链接任何供应商 SDK；
APNs/FCM 适配器在独立 crate 里，用 `HostBuilder::use_push` 注入。离线消息的持久化、设备令牌表和
重试编排属于业务数据模型。

> 设计权威：[IM 实时硬化](../../design/im-hardening.md)（D3 移动推送）。
> 运行期边界台账：[known-boundaries.md](../../design/known-boundaries.md)（C2-06 WebPush）。

## 1. 端口与三态

```rust
pub enum PushPlatform { Apns, Fcm, WebPush }
pub enum PushPriority { Normal, High }            // Normal 默认：可被供应商聚合/延后
pub struct PushNotification { title, body, badge: Option<u32>, sound: Option<String> }

pub struct PushMessage {
    pub platform: PushPlatform,
    pub token: String,                            // 设备令牌（APNs hex / FCM registration token）
    pub notification: Option<PushNotification>,   // None = 静默数据推送
    pub data: serde_json::Value,                  // 必须是 JSON object
    pub collapse_key: Option<String>,
    pub ttl: Option<Duration>,
    pub priority: PushPriority,
}

/// 供应商裁决，不是传输错误。
pub enum PushOutcome {
    Accepted,
    TokenInvalid,
    Retryable { retry_after: Option<Duration> },
}

#[async_trait::async_trait]
pub trait IPushSender: Send + Sync {
    async fn send(&self, message: PushMessage) -> Result<PushOutcome>;
}
```

**三态与 `Err` 的分工**是这张页最重要的约定：

| 结果 | 含义 | 应用动作 |
|---|---|---|
| `PushOutcome::Accepted` | 供应商已受理（不保证设备已展示） | 记录/清空待发标记即可 |
| `PushOutcome::TokenInvalid` | 令牌已失效（APNs `BadDeviceToken`/`Unregistered`，FCM `UNREGISTERED`） | **删除设备注册**、停止重试；这是常规事件，不是故障 |
| `PushOutcome::Retryable { retry_after }` | 限流或临时不可用（429/5xx） | 按 `retry_after` 或退避预算安排重试 |
| `Err(Error)` | 传输/配置失败（连接失败、凭据被拒、请求构造失败） | 告警；可重试错误按应用策略处理 |

把 `TokenInvalid` 映射成 500 会让所有过期设备看起来像服务端事故；端口用三态把“死令牌”和
“基础设施故障”分开。适配器在触网前会先调用 `PushMessage::validate()`，所以空令牌、空
title/body 通知、零 TTL、非 object 的 `data` 在任何供应商下都是同一个 typed `validation_failed`。

## 2. 装配

端口**没有默认实现**：未注册时 Handler 解析 `Arc<dyn IPushSender>` 失败，而不是静默丢消息。
测试与本地开发用捕获实现：

```rust
use std::sync::Arc;
use webx::*;

let push = Arc::new(InMemoryPushSender::new());
Host::builder().use_push(push.clone()).build();

// InMemoryPushSender：保留最近 100 条（默认），可换结果用于测试失效分支
let push = Arc::new(
    InMemoryPushSender::new()
        .with_max_messages(10)
        .with_outcome(PushOutcome::TokenInvalid),
);
assert_eq!(push.len(), 0);
```

Handler 只依赖端口：

```rust
#[derive(Inject)]
struct SendChatPushHandler {
    #[inject]
    push: Arc<dyn IPushSender>,
    #[inject]
    devices: Arc<dyn IDeviceTokenStore>,   // 业务自己的设备表/token 仓库
}

let message = PushMessage::alert(PushPlatform::Fcm, device.token.clone(), "Alice", "hi")
    .with_data(serde_json::json!({ "chat_id": chat.id }))
    .with_collapse_key(format!("chat:{}", chat.id))
    .with_priority(PushPriority::High);

match self.push.send(message).await? {
    PushOutcome::Accepted => {}
    PushOutcome::TokenInvalid => self.devices.delete(&device.token).await?,
    PushOutcome::Retryable { retry_after } => {
        // 交给后台任务/outbox 按退避预算重试；不要把等待塞进请求路径
        self.retry_queue.enqueue(device.token.clone(), retry_after).await?;
    }
}
```

- 一次 `send` 只投递**一个设备令牌**；“给一个用户的全部设备推送”是应用遍历设备表的循环；
- 静默推送用 `PushMessage::silent(platform, token)`（无可见通知），可见通知用 `PushMessage::alert`
  或 `with_notification(PushNotification::new(..).with_badge(..).with_sound(..))`；
- `collapse_key` 让新消息替换未送达的旧消息（聊天列表常用）；`ttl` 是供应商保留上界，
  省略即用供应商默认；
- `WebPush` 平台**仅端口支持**（`PushPlatform::WebPush`）；适配器未交付（**C2**，见
  [known-boundaries.md](../../design/known-boundaries.md) C2-06）——不要假装可用。

## 3. APNs / FCM 适配器

命名约定：provider 适配器 crate 使用短名 `rust-webx-<provider>`（import 名 `webx_<provider>`），统一放在 `crates/push/` 领域组；不再使用 `rust-webx-push-*` 前缀。

两个适配器 crate 随框架发布，但不进入 core/host/umbrella 依赖（`P10 小核心`）：

| crate | 导入名 | 协议 | 凭据 |
|---|---|---|---|
| `rust-webx-apns` | `webx_apns` | APNs HTTP/2 + **token 鉴权**（ES256） | team id + key id + `.p8` 私钥；`apns-topic` 由配置给出 |
| `rust-webx-fcm` | `webx_fcm` | FCM HTTP v1 `messages:send` | service account JSON（RS256 → OAuth2 access token 缓存） |

适配器把请求头/字段按供应商协议映射：APNs 的 `apns-topic`/`apns-push-type`/`apns-priority`/
`apns-expiration`/`apns-collapse-id` 由 `PushPlatform`、`PushPriority`、`ttl`、`collapse_key` 推导；
FCM 的 `message.notification`/`message.data`/`android.collapse_key`/`ttl` 同理。传输复用
`rust-webx-client::IHttpClient`：测试注入 mock HTTP 客户端即可覆盖“正确 header/body + 状态码映射”，
不需要真实推送通道。

### APNs

```rust
use std::sync::Arc;
use std::time::Duration;
use webx::*;
use webx_client::{HttpClient, HttpClientOptions};
use webx_apns::{ApnsConfig, ApnsEnvironment, ApnsPushSender};

// 凭据来自环境变量或密钥存储，不来自 appsettings.json / 源码
let config = ApnsConfig::new(
    std::env::var("APNS_TEAM_ID")?,
    std::env::var("APNS_KEY_ID")?,
    std::env::var("APNS_PRIVATE_KEY")?, // AuthKey_<key_id>.p8 的内容
    std::env::var("APNS_TOPIC")?,      // 通常是 app bundle id
    ApnsEnvironment::Production,       // 或 Sandbox
);
// APNs 只讲 HTTP/2：客户端必须 http2_only
let client = HttpClient::new(
    "apns",
    HttpClientOptions {
        http2_only: true,
        request_timeout: Some(Duration::from_secs(10)),
        ..HttpClientOptions::new(ApnsEnvironment::Production.base_url())
    },
)?;
Host::builder()
    .use_push(Arc::new(ApnsPushSender::new(config, Arc::new(client))?))
    .build();
```

| APNs 响应 | 端口结果 |
|---|---|
| `200` | `Accepted` |
| `400 BadDeviceToken` / `DeviceTokenNotForTopic`、`410 Unregistered` | `TokenInvalid` |
| `400`/`403 ExpiredProviderToken` | 清 provider token 缓存后 `Retryable` |
| `429`、`5xx` | `Retryable`（带 `Retry-After` 时透传） |
| `401`/`403`（凭据被拒） | `Err`（配置缺陷，固定文案） |
| 其他 `4xx` / 未知原因 | `Err`（请求缺陷，固定文案） |

### FCM

```rust
use std::sync::Arc;
use webx::*;
use webx_client::{HttpClient, HttpClientOptions};
use webx_fcm::{FcmPushSender, FcmServiceAccount};

// service-account JSON 从环境变量/密钥存储读取；未知字段忽略，可整份反序列化
let json = std::env::var("FCM_SERVICE_ACCOUNT_JSON")?;
let account = FcmServiceAccount::from_json(&json)?;
let client = HttpClient::new("fcm", HttpClientOptions::new("https://fcm.googleapis.com"))?;
Host::builder()
    .use_push(Arc::new(FcmPushSender::new(account, Arc::new(client))?))
    .build();
```

| FCM 响应 | 端口结果 |
|---|---|
| `200` | `Accepted` |
| `400`/`404` 且原因为 `INVALID_ARGUMENT`/`NOT_FOUND`/`UNREGISTERED` | `TokenInvalid` |
| `429`、`5xx` | `Retryable`（`Retry-After` 或 RetryInfo `retryDelay`） |
| `401`/`403` | 清 access token 缓存后 `Err`（固定文案） |
| 其他 `4xx` | `Err`（请求缺陷） |

两个适配器都是 **Singleton**（缓存 provider token / access token，单飞刷新），在组合根构造一次。
多个平台共存时有两种做法：只注册一个按 `PushMessage::platform` 分派的组合实现（业务 crate 里
写几十行），或每个平台各注册一个并以应用自己的路由表选择——**端口仍只有一个**。

## 4. TokenInvalid 处理

死令牌是推送的常态（用户卸载、换机、清数据），处理不当会持续浪费配额并放大供应商错误率：

1. **立即删除**：收到 `TokenInvalid` 后删除/禁用该设备注册，下次消息不再发给它；
2. **不重试**：它不会因为重试变有效；只有 `Retryable` 与 `Err` 才进入重试预算；
3. **去重与防抖**：同一令牌在并发消息里可能多次返回失效，删除操作要幂等（唯一键/条件更新）；
4. **不要回滚业务**：推送失败不回滚已持久化的消息；业务真相是消息表，推送只是触达手段；
5. **指标**：按平台统计 `Accepted`/`TokenInvalid`/`Retryable` 计数（应用 `IMeter`），
   `TokenInvalid` 比例突然升高通常意味着令牌表过期或平台切换。

## 5. 安全边界

- **令牌与私钥都不落日志**：设备令牌是可关联到个人的标识，APNs `.p8` 与 FCM service account JSON
  是长期凭据；适配器错误只用固定、token-free 文案，`ApnsConfig` / `FcmServiceAccount` 的
  `Debug` 输出会 redact 私钥，provider token（缓存 45 分钟）与 FCM access token（缓存到过期前
  60 秒）只在内存中单飞刷新、绝不打印。业务日志只记录**脱敏后**的尾部或哈希。
- **凭据进密钥存储，不进代码/appsettings 明文**：用 `ISecretStore`（env/file/chain）或部署侧
  secret 注入；`use_secret_protector` 保护需要落库的敏感配置（见[密钥存储](../09-auth-security/secret-store.md)）。
- **不要在数据库事务或锁内发送**：推送是外部 I/O，失败与延迟不可控；请求内只持久化，
  “必须送达”走 outbox + 后台任务（见[事务性 outbox](outbox.md) 与
  [后台服务](../06-di-lifecycle/hosted-services.md)）。
- **不要自己写重试循环**：端口不承诺重试；重试预算、退避与死信由 outbox/后台任务统一承担，
  供应商的 `retry_after` 只是输入之一。
- **data 必须是 JSON object**：两个平台都按 object 传递自定义数据；数组/标量在校验期拒绝，
  避免到供应商才失败。
- 适配器在触网前调用 `validate()`；空的平台凭据/不可用的 HTTP 客户端是启动或首个请求期的
  配置错误（`Err`），不是静默丢弃。

## 6. 验收与相关

测试关注点：端口三态映射（mock HTTP 的 200/410/429/503）、请求形状（header/body 与
`PushMessage` 字段一致）、令牌缓存与 401 刷新、`TokenInvalid` 不重试、日志中不出现令牌/私钥、
`InMemoryPushSender` 捕获与结果覆盖。契约测试归 `testing/contract-tests`（不发布）。

相关：

- [端口与适配器（下游接入）](integrations.md)：端口清单与实现归属。
- [第三方库集成](third-party-integration.md)：邮件/短信通知端口的同构写法。
- [IM 实时硬化设计](../../design/im-hardening.md)：D3 决策、适配器矩阵与验收。
