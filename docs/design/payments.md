# 支付域架构

> 状态：静态 + 运行时配置路径均已落地。运行时按 `PaymentScope`
> （provider/tenant/account）解析 provider 的服务工厂见
> [运行时支付服务工厂](payment-runtime-factory.md)；实现与用法以
> [手册支付章](../rust-webx/13-extensibility/payments.md)为准；本文保留架构取舍与供应商协议依据。
> 结论先行：新增 `crates/payment/` 领域组，做**统一支付端口 + 逐供应商协议适配器**；
> 不引入第三方 Rust 支付 SDK，不把支付能力放进 core 或 umbrella。
> 本文只描述当前基线的做法，不记录版本演进。

---

## 0. 结论摘要

1. **没有可用的官方 Rust 支付 SDK**。Stripe / PayPal / Adyen / Square / 支付宝 / 微信支付的官方
   SDK 语言矩阵都不含 Rust（见 §4.1）；社区 crate 要么自带第二套 HTTP/TLS 栈，要么维护度不足。
2. 因此「开箱即用」的可行路径是**协议级适配器**：用官方文档 + 官方 SDK 源码作为行为基准，
   在 `rust-webx-client` 这一条 HTTP 栈上实现支付所需的最小操作面（下单/查询/撤销/退款/回调），
   并用官方测试向量与沙箱做验收。
3. 载体必须是**领域组 crate**（`crates/payment/...`），符合「core 不出现领域词（IM、媒体、AI、支付）」的
   宪法约束；框架/umbrella 不依赖任何 `pay` crate，应用显式 opt-in。
4. 统一模型只覆盖跨供应商真正同构的部分：金额、状态、支付意图、退款、回调事件、错误分类、
   幂等键。供应商专有字段保留在 `provider_status` / `raw` / `ClientAction` 中，**不做虚假统一**。
5. 回调入口**不隐式注册路由**：crate 提供 `PaymentWebhook`（原始 body + 头部）、
   `verify_webhook` 与 `ack`，应用用 3 行声明路由、10 行写 Handler。这样既开箱即用，
   又不违反「依赖库不得隐式注入路由」。
6. 范围：Stripe、支付宝、微信支付 v3、PayPal、Adyen、Square；银联全渠道暂不提供。

---

## 1. 目标与非目标

### 1.1 目标

- 同一段业务代码可在供应商之间切换：`create → 客户端动作 → 回调/查询 → 退款`。
- 宿主装配 3 行内完成；配置、密钥、健康检查、错误→HTTP 映射全部走框架原生端口。
- 国内（支付宝、微信支付）与国际（Stripe、PayPal）都覆盖；不要求业务方自己实现签名、
  验签、证书轮换、金额换算、回调去重。
- 每个签名/验签实现都有**官方文档出处 + 官方 SDK 行为对照 + 向量测试 + 沙箱验收**。
- 回调处理具备生产级默认：常量时间比较、重放窗口、去重键、乱序容忍、明确 ACK。

### 1.2 非目标（明确不做）

- 订阅/账单/发票/分账/清结算/税务/风控/催收。
- 资金托管、二清、跨境结汇合规（属于应用与持牌机构）。
- 卡号/有效期/CVV 的采集与存储（PCI 范围由供应商托管页或客户端 SDK 承担；服务端只见 token）。
- 供应商全量 API（例如 Stripe 的全部 100+ 资源）。只做支付主链路 + 按需扩展。
- 按租户动态创建账户/密钥托管/KMS per tenant（2.7 只做配置化多账户；动态凭证见 §11 后续）。

---

## 2. 本仓库约束

| 约束 | 出处 | 对支付方案的含义 |
|---|---|---|
| core 不出现领域词（IM、媒体、AI、支付） | [框架宪法](framework-constitution.md) §4 | 不能放 `webx-core::payment`；必须独立 crate |
| 框架 crate / umbrella 不依赖领域组 | [crate 分层](../rust-webx/04-architecture/crate-layout.md) | `rust-webx` 不 re-export 支付 |
| 只有一条出站 HTTP 栈 | [出站 HTTP](../rust-webx/13-extensibility/outbound-http.md) | 适配器基于 `rust-webx-client`，不引入 reqwest/hyper 0.14 |
| 依赖库不得隐式注入路由 | 宪法 §4 | 回调路由由应用声明，crate 只提供请求类型与校验 |
| 2.x 只做加性变更、MSRV 1.95、无 unsafe | 宪法 §5–§6 / 根 `Cargo.toml` | 新 crate 独立版本线随 workspace |
| 唯一事件通道 `IEventPublisher` | [端口与适配器归属](integrations.md) §6 | 支付事件不另起 pub/sub；需要持久化走 inbox/outbox |
| 默认安全、失败关闭 | 宪法 P3 | 配置非法启动失败；验签失败拒绝；不吞异常 |

新增 crate 还要同步：`scripts/check-crate-layout.py` 的 `GROUPS`、`Cargo.toml` 成员与
`workspace.dependencies`、`publish.sh` 与 `.github/workflows/publish.yml` 的发布顺序、
`CHANGELOG.md`、手册与 INDEX/SUMMARY（[稳定性门禁](stability-gates.md)）。

---

## 3. 现状盘点

已有、可直接复用的能力：

| 能力 | 位置 | 支付场景用途 |
|---|---|---|
| 连接池 / 超时 / SSRF 防护 / 有界响应体 | `rust-webx-client`（`IHttpClient`、`OutboundResponse::body.collect(limit)`） | 调用供应商 REST |
| 录制回放测试 | `rust-webx-client::testing`（`MockUpstream`、`ReplayFixture`） | 适配器协议测试，无需真实网络 |
| 配置绑定 | `HostBuilder::add_options::<T>("Section")`、`APP__Section__Key` | 供应商选项 |
| 密钥存储 | `webx_core::secrets`（`ISecretStore`、`SecretValue` 零化 + Debug 脱敏） | 私钥 / API Key / APIv3 Key |
| 常量时间比较、一次性令牌 | `webx_core::security`（`constant_time_eq`、`OneTimeSecret`） | 验签比较、回调重放窗口（可选） |
| 健康检查 | `HostBuilder::add_health_check[_async]` | 配置有效性、证书缓存新鲜度 |
| 后台任务 | `BackgroundService` / `PeriodicBackgroundService` | 证书刷新、对账、inbox 消费 |
| 事件总线 | `IEventPublisher` / `EventMessage` | 支付成功事件的跨进程投递（可选） |
| 响应控制 | `webx_core::action::{ContentResult, JsonResult, StatusCodeResult, Response}` | 各供应商不同的回调 ACK |
| 支付宝 RSA2 签名/验签 | `crates/auth/oauth2/src/alipay.rs`（OAuth 登录用） | 抽取为 `webx_core::security` 的通用 RSA/PEM helper，oauth2 与 pay 共用，避免两套实现 |

缺口：

1. 没有支付端口、统一模型、回调入口与 provider 注册。
2. `IHttpRequest` 提供对象安全的 `header_pairs()`，回调验签可读取全部请求头；
   `webx-engine` 用原始 `HeaderMap` 零拷贝实现，替代/test 主机保持默认空迭代器。
3. 没有 X.509 证书解析依赖（微信平台证书、支付宝证书模式需要从证书取公钥）。
4. 没有支付域的契约测试与沙箱 CI。

---

## 4. 外部方案调研

### 4.1 官方 SDK 语言矩阵（2026-11）

| 供应商 | 官方服务端 SDK 语言 | 官方 Rust SDK | 官方文档 / 参考实现 |
|---|---|---|---|
| Stripe | Ruby、Python、PHP、Java、Node.js、Go、.NET | **无** | `docs.stripe.com`；`stripe/stripe-{node,python,go,java,dotnet,php,ruby}` |
| PayPal | .NET、Java、PHP、Python、Ruby、TypeScript | **无** | `developer.paypal.com`；`paypal/paypal-server-sdks` |
| Adyen | Java、.NET、PHP、Python、Ruby、Node、Go | **无** | `docs.adyen.com`；`Adyen/adyen-*-api-library` |
| Square | Java、.NET、Node.js、PHP、Python、Ruby、Go | **无** | `developer.squareup.com`；`square/connect-*-sdk` |
| 支付宝 | Java、PHP、Python、NodeJS、.NET | **无** | `opendocs.alipay.com`；`alipay/alipay-sdk-*`、`alipay-easysdk` |
| 微信支付 | Java、PHP、Go（APIv3 官方库） | **无** | `pay.weixin.qq.com`；`wechatpay-apiv3/wechatpay-{java,go,php}` |

结论：「直接使用官方 Rust SDK」在本领域不存在。能对标的只有**官方文档 + 官方 SDK 源码**，
这也是本方案把「官方 SDK 行为对照 + 向量测试」写进验收标准的原因。

### 4.2 社区 Rust crate 现状

| crate | 观察 | 结论 |
|---|---|---|
| `async-stripe`（`stripe-rust`） | 由 Stripe OpenAPI 生成、更新频繁；但**自带 hyper 0.14 / surf / 自建 TLS 组合**，源码 4.6 MB，全量编译分钟级 | 不进入适配器依赖；需要全量 API 的应用可直接单独依赖 |
| `paypal-rs` / `paypal-rust` | 早期阶段、维护稀疏 | 不依赖 |
| `adyen` / `square-rs` / `autogen-squareup` | 社区实现，各自带 HTTP 栈 | 不依赖，仅作字段对照 |
| `alipay_sdk_rust` / `weapay` / `wechat_pay_sdk` / `wechatpay-rust` | 版本老、覆盖面窄、无证书轮换 | 不依赖，仅作行为对照 |
| `payment`（统一支付 crate 尝试） | 宣称统一 Stripe/Alipay/WeChat，覆盖面与维护度未验证 | 不依赖 |

它们共同的问题不是「不能跑」，而是与本仓库的三条硬约束冲突：**第二套 HTTP 栈**、
**没有 DI/健康检查/可观测性接入点**、**没有契约测试与发布纪律**。

### 4.3 协议要点（设计输入）

| 维度 | Stripe | PayPal | Adyen | Square | 支付宝 | 微信支付 v3 |
|---|---|---|---|---|---|---|
| 传输 | REST（v1 以 form-encoded 为主） | REST JSON | REST JSON | REST JSON | 表单 POST 到网关 | REST JSON |
| 鉴权 | `Authorization: Bearer sk_...` | OAuth2 client_credentials | `X-API-Key` | `Authorization: Bearer` | RSA2 参数签名 | 商户私钥 RSA 请求签名 |
| 金额 | 最小货币单位整数 | 十进制字符串 + 币种 | 最小单位整数 | 最小单位整数 | 元字符串（2 位） | 分整数 |
| 幂等 | `Idempotency-Key` | `PayPal-Request-Id` | `Idempotency-Key` | `Idempotency-Key` | `out_trade_no` | `out_trade_no` |
| 回调验签 | HMAC-SHA256（`t.body`） | 调用验签 API | HMAC-SHA256（字段拼接） | HMAC-SHA256（URL+body） | RSA2（参数排序） | RSA 验签 + AES-GCM 解密 |
| 重放窗口 | 300s（官方库默认） | 由验签 API 控制 | 无时间戳 | 无时间戳 | 无强制窗口 | 5 分钟 |
| 回调 ACK | 任意 2xx | 200 | 200 | 200 | 纯文本 `success` | 200 + `{"code":"SUCCESS"}` |
| 状态源 | PaymentIntent.status | Order/Capture.status | resultCode | Payment.status | trade_status | trade_state |

各供应商的关键实现细节：

- **Stripe**：`Stripe-Signature: t=<unix>,v1=<hex>`，签名串为 `"{t}.{raw_body}"`，
  可有多个 `v1`（密钥轮换期），未知条目必须忽略；必须使用**原始 body**。
  API 版本用 `Stripe-Version` 固定。参考 [Webhook 签名](https://docs.stripe.com/webhooks/signature)、
  [幂等请求](https://docs.stripe.com/api/idempotent_requests)。
- **支付宝**：RSA2（SHA256withRSA）。请求参数按 key 的 ASCII 升序、`k=v` 用 `&` 连接后签名
  （`sign` 不参与；官方 SDK 也不把 `sign_type` 放进待签 map）。**响应验签不是重序列化**：
  官方 SDK 取响应 JSON 中 `xxx_response` 节点的原始子串验签；回调是表单参数，验签时剔除
  `sign`/`sign_type`。回调成功必须返回字面量 `success`。参考
  [自行实现签名](https://opendocs.alipay.com/common/057k53)、
  [签名与验签](https://opendocs.alipay.com/pre-open/common/104741)。
- **微信支付 v3**：请求签名串为 5 行（`METHOD\nURL_PATH\nTIMESTAMP\nNONCE\nBODY\n`），
  `Authorization: WECHATPAY2-SHA256-RSA2048 ...`；应答/回调验签用 3 行
  （`TIMESTAMP\nNONCE\nBODY\n`）+ 平台证书或微信支付公钥；回调 `resource` 用
  APIv3 密钥做 `AEAD_AES_256_GCM` 解密；平台证书通过 `GET /v3/certificates` 下载，
  证书本身也加密，按 `serial_no` 缓存并在未知 serial 时刷新。参考
  [请求签名](https://pay.weixin.qq.com/doc/v3/merchant/4012365336)、
  [Authorization 示例](https://pay.weixin.qq.com/doc/v3/merchant/4012365344)、
  [应答验签](https://pay.weixin.qq.com/doc/global/v3/zh/4012354989)。
- **PayPal**：Webhook 验签是**服务端 API 调用**（`POST /v1/notifications/verify-webhook-signature`，
  需要 `webhook_id` + `PAYPAL-TRANSMISSION-*` 头 + `cert_url` + 原始事件），返回
  `verification_status`。因此验签端口必须是 `async`。参考
  [Verify webhook signature](https://developer.paypal.com/api/webhooks/v1/verify-webhook-signature-post)。
- **Adyen**：一个 POST 可含多个 `notificationItems`；签名在 `additionalData.hmacSignature`，
  按固定字段拼接并做 `\`/`:` 转义；应答 `[accepted]`。参考
  [Verify HMAC signatures](https://docs.adyen.com/development-resources/webhooks/secure-webhooks/verify-hmac-signatures)。
- **Square**：`x-square-hmacsha256-signature` = Base64(HMAC-SHA256(key, notification_url + raw_body))；
  无时间戳，重放防护依赖事件 ID 去重。参考
  [Validate an event notification](https://developer.squareup.com/docs/webhooks/step3validate)。
- **银联全渠道**：表单 + 证书签名、回调表单，协议细节与证书管理复杂度最高，列为第三批，
  实现前需要单独确认签名算法版本与证书链要求。

### 4.4 编排器路线（备选）

[Hyperswitch](https://github.com/juspay/hyperswitch) 是 Apache-2.0、Rust 编写的开源支付编排器，
支持大量国际 PSP 与部分本地支付方式。把它当作一个 provider（`payment/hyperswitch` 适配器）可以
用一份适配器覆盖长尾渠道。代价是引入一个需要部署与运维的独立服务（Postgres/Redis 等），
与「进程内开箱即用」的目标冲突。结论：**不作为 v1 方案**，作为长尾渠道的可选扩展记录在案。

---

## 5. 方案对比与选型

| 方案 | 做法 | 优点 | 致命问题 | 结论 |
|---|---|---|---|---|
| A. 包装社区/官方 SDK | 直接依赖 `async-stripe` 等 | API 覆盖全、少写协议代码 | 第二套 HTTP/TLS 栈；编译体积与时间；无国内供应商；与 DI/健康检查脱节 | 不采用 |
| B. 协议适配器 | 基于 `rust-webx-client` 实现最小操作面 | 单一 HTTP 栈；模型可控；国内外一致；可测可审计 | 需要自己维护协议与版本 | **采用** |
| C. 自托管编排器 | 接入 Hyperswitch | 渠道覆盖广 | 额外服务与运维；不符合进程内体验 | 后续可选 |
| D. OpenAPI 代码生成 | 从 Stripe/Adyen/PayPal 规范生成 | 覆盖全 | 生成器维护成本、编译膨胀；支付宝/微信无规范 | 不采用 |

选型结论：**B**。A 仅作为「需要全量 API 时的互操作说明」写进文档；C 预留为长尾扩展；
D 不做。已经用 `async-stripe` 的应用不需要迁移：实现一个 `IPaymentProvider` 把现有调用包起来
即可，协议适配器与 SDK 可以共存。

---

## 6. 架构设计

### 6.1 crate 布局

```text
crates/payment/
├── core/        rust-webx-pay        webx_pay        统一模型 + 端口 + 回调请求 + 错误
├── stripe/      rust-webx-pay-stripe webx_pay_stripe
├── alipay/      rust-webx-pay-alipay webx_pay_alipay
├── wechatpay/   rust-webx-pay-wechatpay webx_pay_wechatpay
├── paypal/      rust-webx-pay-paypal webx_pay_paypal
├── adyen/       rust-webx-pay-adyen  webx_pay_adyen
└── square/      rust-webx-pay-square webx_pay_square

examples/adapters/payment/ef/   rust-webx-pay-ef  webx_pay_ef  回调 inbox 参考实现（非发布，可复制）
```

依赖方向：

```text
应用 Handler
   │
   ├── webx_pay_*（适配器）──► webx_pay（核心）──► webx_core + webx_client
   │                                  │
   │                                  └──► webx_client（唯一 HTTP 栈）
   └── webx（umbrella，仅应用依赖）
```

- `payment/core` 依赖 `rust-webx-core`（请求模型、响应类型、错误）+ `rust-webx-client`（传输）+
  `rust-webx-host`（`HostBuilder` 扩展、启动校验/健康检查接入），不依赖 umbrella。
- 适配器依赖 core 与 `rust-webx-client`，通过 core 复用宿主扩展。
- `payment/ef` 依赖 `rust-ef` + core，不反向依赖任何适配器。
- `scripts/check-crate-layout.py` 增加 `"pay"` 组；发布顺序 `payment/core` 先于各适配器。

### 6.2 统一模型（`webx_pay`）

只统一真正同构的概念：

```rust
pub struct Money {
    pub currency: Currency,   // ISO 4217，含指数表（0/2/3 位小数）
    pub minor: i64,           // 最小货币单位；禁止 f64
}

pub enum PaymentStatus {
    Created, RequiresAction, Processing, Authorized,
    Succeeded, Canceled, Failed, Refunded, PartiallyRefunded, Unknown,
}

pub struct PaymentIntent {
    pub provider: ProviderId,
    pub order_id: String,              // 商户订单号（幂等锚点）
    pub provider_payment_id: Option<String>,
    pub status: PaymentStatus,
    pub provider_status: String,       // 原样保留，不丢失信息
    pub amount: Money,
    pub amount_captured: Option<Money>,
    pub amount_refunded: Option<Money>,
    pub client_action: Option<ClientAction>,
    pub created_at: i64,
    pub metadata: BTreeMap<String, String>,
}

pub enum ClientAction {
    Redirect { url: String },                   // 托管页 / 支付链接
    QrCode { content: String },                 // 扫码（微信 Native、支付宝当面付）
    AppOrder { provider: ProviderId, payload: serde_json::Value }, // App SDK 参数
    ClientSdk { provider: ProviderId, payload: serde_json::Value }, // Stripe Elements 等
}

pub enum PaymentFlow {
    ClientSdk { return_url: Option<String> },   // 服务端建意图，客户端 SDK 确认
    Redirect { success_url: String, cancel_url: Option<String> }, // 跳转托管页
    QrCode,                                     // 展示二维码
    App { return_url: Option<String> },         // App SDK 拉起
}

pub struct CreatePayment {
    pub order_id: String,
    pub amount: Money,
    pub description: String,
    pub method: PaymentMethod,
    pub flow: PaymentFlow,
    pub metadata: BTreeMap<String, String>,
    pub idempotency_key: Option<String>,
    pub expires_at: Option<i64>,
}
```

设计要点：

- `ProviderId` 是封闭枚举（Stripe/Alipay/WechatPay/PayPal/Adyen/Square/`Custom(String)`），
  未知值不猜测；`PaymentMethod` 覆盖 Card、支付宝 page/wap/app/qr、微信 native/jsapi/app/h5、
  PayPal 与 `Other(String)`，适配器对不支持的组合返回 `Unsupported`。
- **金额**：对外只用 `Money`；内部一律最小单位整数。`Currency` 携带 ISO 4217 指数表，
  拒绝未知币种；CNY=2、JPY=0、KWD/BHD=3。解析供应商字符串（支付宝“元”、PayPal `value`）
  用严格十进制解析器，最多允许指数位小数，**不引入浮点**。
- **状态**：`PaymentStatus` 是规范化视图，`provider_status` 保留原文。映射表逐供应商固定，
  未知状态落 `Unknown` 并记录原文，绝不猜测为成功。
- **客户端动作**：统一到「客户端下一步要做什么」，供应商专有参数放 `payload`；
  避免把 Stripe 的 `client_secret`、微信的 `prepay_id`、支付宝的 `orderStr` 硬塞进同一结构。
- **退款**：`RefundRequest { order_id, refund_id, amount: Option<Money>, reason }` +
  `Refund { status, provider_refund_id, provider_status, amount }`；部分退款由能力位控制。
- **错误**：

```rust
pub enum PaymentError {
    Config { provider: ProviderId, detail: String },
    Validation { detail: String },
    Unsupported { provider: ProviderId, capability: &'static str },
    Provider { provider: ProviderId, code: Option<String>, message: String,
               http_status: Option<u16>, request_id: Option<String>, retryable: bool },
    SignatureInvalid { provider: ProviderId, reason: &'static str },
    Transport(webx_client::HttpClientError),
    Serialization(String),
    Internal(String),
}
```

提供 `retryable()`、`http_status()`、`public_detail()` 与 `Into<webx::Error>`（仅应用边界使用），
与 rust-ef 的「持久层错误在边界映射」原则一致。错误信息不回显密钥、证书、完整回调 body。

### 6.3 端口

单 trait + 能力位 + 默认 `Unsupported`，避免 trait 爆炸，也避免适配器写空实现：

```rust
#[async_trait]
pub trait IPaymentProvider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn capabilities(&self) -> ProviderCapabilities;

    /// 启动期静态校验：必填项、PEM 可解析、URL 为 https、币种/通道支持。
    fn validate(&self) -> Result<(), PaymentError>;

    async fn create_payment(&self, req: CreatePayment) -> Result<PaymentIntent, PaymentError>;
    async fn get_payment(&self, order_id: &str) -> Result<PaymentIntent, PaymentError>;

    async fn capture_payment(&self, _order_id: &str, _amount: Option<Money>)
        -> Result<PaymentIntent, PaymentError> {
        Err(PaymentError::unsupported(self.id(), "capture"))
    }
    async fn cancel_payment(&self, _order_id: &str) -> Result<PaymentIntent, PaymentError> {
        Err(PaymentError::unsupported(self.id(), "cancel"))
    }
    async fn refund(&self, _req: RefundRequest) -> Result<Refund, PaymentError> {
        Err(PaymentError::unsupported(self.id(), "refund"))
    }
    async fn get_refund(&self, _refund_id: &str) -> Result<Refund, PaymentError> {
        Err(PaymentError::unsupported(self.id(), "get_refund"))
    }

    fn webhook_verifier(&self) -> &dyn IPaymentWebhookVerifier;
}
```

`IPaymentWebhookVerifier` 单独成 trait，因为验签是「无业务副作用」的纯校验，且 PayPal 需要网络：

```rust
#[async_trait]
pub trait IPaymentWebhookVerifier: Send + Sync {
    fn provider(&self) -> ProviderId;
    async fn verify(
        &self,
        scope: &PaymentScope,
        payload: &WebhookPayload,
    ) -> Result<PaymentEvent, PaymentError>;
    /// 供应商要求的成功应答（Alipay=text "success"、WeChat=JSON、Stripe=200）。
    fn ack(&self) -> webx_core::action::Response;
}
```

`PaymentEvent` 含 `provider`、`event_type`、`dedupe_key`、`order_id`、`provider_payment_id`、
`amount`、`occurred_at`、`raw`（有界 JSON）。`dedupe_key` 的语义逐供应商固定：
Stripe=`evt.id`；微信=通知 `id`；PayPal=`event.id`；Square=`event_id`；支付宝无事件 ID，
用 `out_trade_no + trade_status + trade_no`；Adyen 用 `pspReference + eventCode + success`。
两个回退键都**不含时间成分**，保证供应商重投时稳定。**去重键是契约的一部分**，写进文档与契约测试。

为什么不做 `IPaymentGateway`/`IPaymentRefundGateway`/`IPaymentQuery` 多个 trait：绝大多数场景
是「拿到一个 provider 对象做完整生命周期」，多 trait 会让 DI 注入与能力探测变复杂；能力位 +
默认 `Unsupported` 已能表达差异，且错误可读。

供应商选择策略（按地区、币种、费率、成功率路由）属于业务决策，core 不内置路由器；
需要时由应用包一层服务，在 `Payments` 之上做选择。

### 6.4 注册、配置与 DI

适配器提供两个扩展方法（不引入 `add_payments()` 装配仪式）：

```rust
Host::builder()
    .add_stripe()                                                 // 运行时：读取 "Stripe" 节
    .add_alipay()                                                 // 运行时：读取 "Alipay" 节
    .add_square_with(SquareOptions::new(token, signature_key))    // 静态：预构造 provider
    .build();
```

- 每个 `add_*()` 注册：适配器级 `IPaymentProviderFactory`（从 `IOptionsMonitor<XxOptions>`
  构造）与 `IPaymentProviderBuilder`（供自定义工厂复用协议实现）；静态路径
  `add_*_with(options)` 还注册 `payments.<provider>` 健康检查。第一个
  `add_*` 幂等创建 `Payments` 运行时，注册顺序无关。没有 `add_payments()`。
- `add_*_with(options)` 注册预构造 provider 的静态工厂；它与运行时工厂对 `Payments`
  完全透明，走同一条缓存/错误/可观测路径。`add_*_from_config()` 与 `add_*_with_client()`
  已删除：前者等价于 `add_*()`，后者测试用 `Provider::with_client` + `add_payment_provider`。
- 运行时配置源由 `IPaymentServiceFactory` 决定；默认实现把 scope 路由到各
  `IPaymentProviderFactory`，应用可替换为 DB/KMS 实现。业务工厂只依赖 `webx_pay`，
  不依赖适配器 crate；通过 `PaymentProviderBuilders` 复用官方协议实现。
- `IPaymentServiceFactory` 是普通 DI 单例：应用用 `services.singleton::<dyn
  IPaymentServiceFactory>(…)`/`instance` 注册；框架默认工厂用 `try_add_singleton`，
  dix 后注册覆盖先注册，因此 `add_*` 前后注册均可，没有支付专用覆盖 API。
- `Payments` 通过 rust-dix 1.0 的 `GetServices<T>` 语义聚合工厂（`get_all_any` + downcast；
  rust-dix 对 `&dyn` 没有对象安全的 `get_all`，需要一行 helper）。
- 启动校验不再阻塞进程：静态 provider 在构造时校验，运行时配置在首次解析/健康检查校验；
  配置源不可用返回 503（`PaymentError::Unavailable`），由 readiness 暴露。原因是运行时
  DB/KMS 在启动时可能不可用，fail-fast 会误伤可用性。
- 配置：`add_*()` 自动 `add_options::<XxOptions>("Section")`（同时注册 `Arc<T>`、具体 monitor 与
  monitor 端口），支持 appsettings 与 `APP__Stripe__SecretKey` 覆盖；`bind_config` 只按顶层 key
  取子节点，因此每个供应商一个顶层节。密钥字段的 `Debug` 已脱敏。
- 传输：默认由适配器创建 `HttpClient`（base_url、超时、`ssrf_protection` 保持开启）；
  测试/共享连接池通过各 provider 的 `with_client` 构造器完成。
- 测试替身：`rust-webx-client::testing::MockUpstream` 录制/回放，适配器测试不触网。

### 6.5 回调入口（关键取舍）

约束是「依赖库不得隐式注入路由」，所以**不**在 crate 里写 `#[post("/webhooks/...")]`。
crate 提供：

```rust
// webx_pay：手写 RequestModel，读取原始 body + 回调头，不做 JSON 解析
pub struct PaymentWebhook {
    // provider/account 来自路径参数，body/headers 由绑定层填充
    // 对外提供 provider()/account()/provider_id()/account_id()/payload()/body()
}
```

应用侧（默认一条路由覆盖全部供应商）。Rust 的孤儿规则不允许应用为外部类型
`PaymentWebhook` 实现 `IRequest`，所以由 `payment_webhook!` 在应用 crate 内
生成一个本地包装类型（转发原始 body 绑定并声明路由），Handler 仍写在应用里：

```rust
use webx::*;
use webx_pay::{payment_webhook, Payments};

payment_webhook!(StripeWebhook, "/webhooks/payments/{provider}");

#[derive(Inject)]
pub struct PaymentWebhookHandler {
    #[inject] pub payments: Arc<Payments>,
    #[inject] pub orders: Arc<dyn IOrderService>, // 应用自己的端口
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<StripeWebhook, Response> for PaymentWebhookHandler {
    async fn handle(&mut self, req: StripeWebhook) -> ActionResult<Response> {
        let event = self.payments.verify_webhook(&req).await?; // 验签+规范化
        self.orders.apply_payment_event(event).await?;          // 业务事务
        Ok(self.payments.ack(&req).await?)                      // 供应商要求的 ACK
    }
}
```

包装类型 `Deref` 到原始请求，所以 `verify_webhook(&req)`、`req.provider()` 直接可用。
账户级回调仍是同一个宏与同一个请求类型，account 自动从路径绑定：

```rust
payment_webhook!(AccountWebhook, "/webhooks/payments/{provider}/{account}");
// handler 里仍然只有 payments.verify_webhook(&req)，没有第二个 verify 方法
```

`Payments::verify_webhook` 从路径解析 `ProviderId`，未知供应商返回 404，验签失败返回 400
（problem+json），配置源不可用返回 503 让供应商重投，绝不把未验签的数据交给业务。

**每条路由一个请求类型**：路由宏按请求类型名生成 `bind`/`dispatch`/`encode` 函数，同一类型在
同一模块只能声明一条路由。默认用一条 `{provider}` 路由；需要 `/webhooks/stripe`、
`/webhooks/alipay` 这类独立路径时，为每条路由调用一次 `payment_webhook!` 生成不同类型。
需要按供应商做 IP 白名单、独立限流或独立审计时，也使用独立路径。

回调路由不加 `#[authorize]`、不依赖 cookie 会话（签名即认证）；若全局限流开启，
需要确认回调路径的阈值不会误伤供应商重试流量。

**头部枚举**：`webx-core` 为 `IHttpRequest` 提供了对象安全的
`header_pairs()`；`webx-engine` 用现成的 `HeaderMap` 实现，零拷贝；默认空迭代器保证
其他/测试主机不破坏。`PaymentWebhook` 优先用它，并对内置供应商头做按名兜底，
兼容未实现枚举的替代主机。

**为什么不让 crate 自带通用 Handler 自动分发**：handler 可用 inventory 注册、路由仍需应用声明，
技术上可行；但会在库里埋下「事件如何落到业务事务」的策略（同步？发布事件？写 inbox？），
与「基础设施归框架、业务归应用」的边界冲突，也会让多供应商路由难以审计。保持显式
`verify → 业务 → ack` 十行代码，比隐式分发更符合本仓库风格。

### 6.6 可靠性

支付回调是**至少一次**投递，重复与乱序是常态：

| 环节 | 做法 |
|---|---|
| 创建幂等 | 以 `order_id` 为锚：Stripe/PayPal/Adyen/Square 映射到各自幂等头；支付宝/微信用 `out_trade_no` |
| 回调去重 | `dedupe_key` 唯一约束（`payment/ef` inbox）或应用表唯一索引；缓存只能做快路径，不能当唯一性保证（`IDistributedCache` 无原子 set-if-absent） |
| 乱序 | 只允许状态单调推进；`RefundSucceeded` 先于 `Succeeded` 到达时不回退订单；必要时用 `get_payment` 拉取权威状态再落库 |
| 网络超时 | 写操作不自动重试；调用方用同一幂等键重试。查询类可重试一次 |
| 对账 | `PeriodicBackgroundService` 对 `Processing/Created` 超时订单调用 `get_payment`；提供通用 `PaymentReconciler`，订单存储由应用实现 |
| 事件外发 | 应用内事务写 outbox 或 inbox，再由后台任务发布；不在回调线程里直接做重业务 |
| ACK 时机 | 验签 + 持久化（inbox 落库）后立即 ACK；业务处理异步。供应商超时时间普遍 5–30s |

`payment/ef` 提供通用 inbox 表：`provider + dedupe_key` 唯一、`payload`、`attempts`、
`locked_until`、`processed_at`；后台 worker 领取、调用应用 `IPaymentEventHandler`、失败退避、
超限进死信。订单/退款表属于应用领域，**不进框架**。

### 6.7 安全

- 验签：Stripe/Adyen/Square 用 HMAC-SHA256 + `constant_time_eq`；支付宝/微信用 RSA
  PKCS#1 v1.5（SHA-256）。只使用经过审计的 crate（`rsa`、`sha2`、`hmac`、`x509-cert`），
  不手写填充与 ASN.1。
- 重放：Stripe 默认 300s 容差；微信 5 分钟；PayPal 由验签 API 判定；Adyen/Square 依靠
  dedupe。窗口校验失败返回 400。
- 机密：密钥只从配置/环境/`ISecretStore` 进入；`Debug` 脱敏；日志不打印私钥、签名串、
  `client_secret`、`prepay_id`；回调 body 仅在排障级别按截断 + 脱敏白名单记录。
  对外错误只给通用文案与 `request_id`，不回显供应商原文。
- 传输：只接受 `https://` 端点（`allow_private_networks` 仅供本地测试）；SSRF 防护保持开启；
  不跟随重定向。
- PCI：服务端不接触卡号；卡数据只经供应商托管页/客户端 SDK。文档中明确写出这条边界，
  避免使用者误以为框架承担 PCI 范围。
- 证书：微信平台证书按 serial 缓存、未知 serial 触发一次刷新（带节流）、证书过期前告警；
  证书公钥只从解密后的证书 PEM 提取，不信任回调头里的任何证书内容。

### 6.8 可观测性与健康

- `tracing`：每次操作一个 span（provider、operation、order_id 哈希、http_status、耗时、
  供应商 request id），与 `RequestContext::trace_id()` 同源；不记录 openid、手机号等买家标识。
- 指标：`payments_operations_total{provider,operation,result}`、`payments_operation_seconds`
  尚未接入；需要时用 `IMeter`，不新建指标后端。
- 健康：`provider {id}` 检查「配置已校验 + 证书缓存非空且未过期 + 上次成功调用时间」；
  不做每次探测都打供应商 API。启动校验与 readiness 分离：配置错误启动失败，网络问题只降级。

### 6.9 版本与兼容

- 供应商 API 版本显式固定：Stripe `Stripe-Version`、Square `Square-Version`、
  Adyen 路径版本（`/v71`）；支付宝/微信按文档版本记录在适配器常量里。
- 新供应商只加 crate，不改 core 的既有类型；core 的新增字段用 `Option` + `#[non_exhaustive]`
  的策略与仓库既有 DTO 一致。
- 供应商协议变更走「适配器 minor + CHANGELOG + 契约测试向量更新」，不静默改语义。

---

## 7. 供应商适配矩阵

### 7.1 为什么是这几个

- **覆盖优先**：国内线上收款几乎只在支付宝/微信之间选择；国际卡收单以 Stripe 为开发者事实标准，
  PayPal 覆盖无卡钱包用户，Adyen 面向企业多渠道路由，Square 面向北美线下/线上小商户。
- **文档与参照优先**：六个供应商都有完整官方文档与官方 SDK 源码，协议边界清晰，
  可以被契约测试锁定；银联全渠道虽然也有文档，但证书/表单协议成本明显更高，放在第三批。
- **复杂度分层**：Stripe/Square（JSON + HMAC）→ PayPal（OAuth + API 验签）→
  支付宝（RSA2 + 响应子串）→ 微信（RSA + 证书 + AES-GCM），先用简单协议验证统一模型，
  再处理硬协议。
- **不做聚合支付**：Ping++/Airwallex 等属于应用层选型；框架只提供 `IPaymentProvider` 端口，
  应用可自行包一层。出海业务也可以先用 Stripe/Adyen/PayPal 的 Alipay/WeChat 支付方式，
  再按需接直连。

| 能力 | Stripe | 支付宝 | 微信支付 v3 | PayPal | Adyen | Square |
|---|---|---|---|---|---|---|
| 下单 | PaymentIntent / Checkout | page/wap/app/precreate | Native/JSAPI/App/H5 | Orders v2 | /payments、/sessions | /v2/payments |
| 查询 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| 手动捕获 | ✓ | — | — | ✓ | ✓ | ✓（延迟捕获） |
| 撤销/关闭 | cancel | close | close | void | cancel | cancel |
| 退款/部分退款 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| 托管页 | Checkout | page/wap | H5 | approve link | Sessions | Payment Links |
| 扫码 | — | precreate | Native | — | 视渠道 | — |
| App 参数 | — | orderStr | prepay_id + 签名参数 | — | — | — |
| 回调验签 | HMAC | RSA2 | RSA + AES-GCM | 验签 API | HMAC | HMAC |

Apple Pay / Google Pay 不是独立收单方：它们产出加密 token，由 Stripe/Adyen/PayPal 等渠道处理。
适配器只需把客户端 token 作为支付方式参数透传，v1 不为它们单独建 crate。

状态映射（节选，实现时以各自文档为准）：

| 规范状态 | Stripe | 支付宝 | 微信 | PayPal | Adyen |
|---|---|---|---|---|---|
| RequiresAction | requires_action | WAIT_BUYER_PAY | NOTPAY/USERPAYING | PAYER_ACTION_REQUIRED | RedirectShopper/ChallengeShopper |
| Authorized | requires_capture | — | — | APPROVED（未捕获） | Authorised（手动捕获模式） |
| Succeeded | succeeded | TRADE_SUCCESS/TRADE_FINISHED | SUCCESS | COMPLETED | Authorised（自动捕获）/ Settled |
| Canceled | canceled | TRADE_CLOSED | CLOSED/REVOKED | VOIDED | Cancelled |
| Failed | —（payment_failed 事件） | — | PAYERROR | DECLINED/FAILED | Refused/Error |

每供应商实现清单（同一套骨架）：

1. `options.rs`：`serde` 选项 + 脱敏 `Debug` + `validate()`。
2. `sign.rs`：请求签名 / 响应验签 / 回调验签（纯函数优先，便于向量测试）。
3. `client.rs`：基于 `IHttpClient` 的传输与错误解析（有界 body、供应商错误码映射）。
4. `mapping.rs`：DTO ↔ 统一模型（金额、状态、客户端动作）。
5. `provider.rs`：`IPaymentProvider` + `IPaymentWebhookVerifier` + `ack`。
6. `health.rs`：配置/证书健康检查。
7. `lib.rs`：`add_*` 扩展 trait + re-export。

---

## 8. 目标使用体验

```rust
// 1) 装配（一条 add_* 一个供应商；运行时配置来自配置节）
let host = Host::builder()
    .add_stripe()
    .add_alipay()
    .add_wechat_pay()
    .build();

// 2) 下单（业务 Handler 内；provider/tenant/account 由 PaymentScope 选择）
let scope = PaymentScope::new(ProviderId::Stripe).tenant(tenant_id);
let intent = payments
    .create(
        scope,
        CreatePayment::new(
            order.id.clone(),
            Money::new(Currency::CNY, 9900), // ¥99.00
            PaymentMethod::Card,
            PaymentFlow::ClientSdk { return_url: Some(success_url) },
        )
        .with_description("Pro plan")
        .with_metadata("user_id", user.id.clone()),
    )
    .await?;

// 3) 把 intent.client_action 返回给前端（Redirect / QrCode / AppOrder / ClientSdk）
// 4) 回调：应用声明路由，10 行 Handler（见 §6.5）
```

配置示例（每个适配器一个顶层节；`accounts` 支持多账户）：

```json
{
  "Stripe": {
    "accounts": [
      { "id": "default", "secret_key": "", "webhook_secret": "" }
    ],
    "default_account": "default"
  },
  "Alipay": { "accounts": [] },
  "WeChatPay": { "accounts": [] }
}
```

环境覆盖：`APP__Stripe__Accounts__0__SecretKey`、`APP__WeChatPay__Accounts__0__ApiV3Key`；生产建议配合
`ISecretStore`（文件/环境/KMS），不把私钥写进镜像。

---

## 9. 测试与验收策略

1. **纯函数向量测试**：签名串构造（微信 5 行/3 行、支付宝排序、Stripe `t.body`、Adyen 转义、
   Square URL+body）、金额解析、状态映射。向量来源：官方文档示例 + 官方 SDK 测试代码
   （注明出处与许可证）。
2. **传输层回放**：`MockUpstream` 录制真实响应，断言路径、头、body、签名；错误响应
   （4xx/5xx、超时、超大 body）逐一覆盖。
3. **契约测试**：`webx_pay::testing::payment_provider_contract(&dyn IPaymentProvider)`，
   用内存假 provider 验证「账户/能力一致性、配置可校验、不支持能力返回 Unsupported」等语义；
   每个适配器在自身测试里调用它，协议签名/状态映射由适配器自己的向量测试覆盖。
4. **沙箱/夜间**：Stripe 测试模式、支付宝沙箱、PayPal sandbox、Adyen test；微信 v3 没有公开
   沙箱环境（以官方当前说明为准），用官方验收用例/回放；无 secrets 时 job 跳过并保持绿色
   （与 broker-integration 同模式）。
5. **安全用例**：Debug 不泄漏密钥；验签失败/过期时间戳/重复事件/未知 serial/错误 ACK；
   回调 body 被中间件改写时验签失败（防止「先解析 JSON 再验签」的错误用法）。
6. **模糊测试（lite）**：随机 header/body 喂给验签与解析，不 panic、不越界；纳入
   `fuzz-lite` 风格用例。

---

## 10. 风险与决策记录

### 已拍板决策

1. `IHttpRequest::header_pairs()` 进入 `webx-core`（已实现 + 测试）。
2. RSA/证书工具：先在支付宝/微信适配器内封装（接口一致：PEM 规范化、SHA256withRSA
   签/验、证书公钥提取）；`oauth2` 的支付宝登录保持原实现，统一到 `webx_core::security`
   留到后续 minor（core 避免新增重量级 RSA 依赖）。
3. `payment_webhook!` 路由宏随框架提供（孤儿规则要求应用侧本地包装类型）；
   不提供通用 Handler 自动分发，业务分发仍在应用 Handler。
4. `payment/ef` inbox 已交付（生产强建议）。
5. 多账户/服务商模式已交付（`AccountId` + `AccountSet` + `PaymentScope`；账户段
   回调复用同一个 `PaymentWebhook`）。
6. **API 收敛**：`add_payments()` 删除，由 `add_*` 自动幂等装配 `Payments`；静态配置统一为
   `add_*_with(options)`，运行时配置为 `add_*()`；`add_*_from_config`/`add_*_with_client`
   删除。`PaymentHub` 更名为 `Payments`，`PaymentAccountWebhook`/`verify_account_webhook`
   合入单一类型/方法。
7. **运行时配置**：`IPaymentServiceFactory` + `IPaymentProviderFactory` + `PaymentScope`
   + TTL 缓存/单飞/失效落地，默认工厂沿用各配置节；业务可替换为 DB/KMS 工厂。
8. **前端支付目录**：`PaymentOption` + `PaymentFlowKind` + `Payments::payment_options(_for)`
   暴露已启用 provider/account/method/flow 与公开 client_config；适配器不得输出密钥；
   多租户目录由自定义 `IPaymentServiceFactory::payment_options` 提供。
9. 银联全渠道暂不做（协议/证书成本最高，按需求评估）。
10. 保持 2.x 加性策略。

实现与原设计的差异（均已写入适配器文档）：

- 支付宝证书序列号按官方 SDK 的 `issuer + 十进制 serial` MD5 算法实现，而不是简化的
  `MD5(DER)`；证书模式使用配置的支付宝公钥证书。
- Adyen Checkout 没有文档化的支付状态 GET；通用 `get_payment` 返回 `Unsupported`，
  另提供需要前端 details 的 `get_payment_with_details`。
- PayPal / Square 的查询、捕获、退款以供应商对象 id 为键（没有按商户订单号反查的 API）；
  商户订单号保留在 metadata/custom_id 并通过回调返回。
- 微信平台证书首次下载按官方规定豁免应答验签；业务应答与回调全部验签失败关闭。

### 仍需关注的风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| 供应商协议漂移 | 线上失败 | 固定 API 版本；契约测试 + nightly 沙箱；变更走 minor |
| 自研签名/证书代码出错 | 资金/安全 | 只用审计过的 crate；向量 + 沙箱 + fuzz；常量时间比较 |
| 微信证书轮换/公钥模式 | 验签失败 | 缓存 + 未知 serial 刷新 + 过期告警；双模式支持 |
| 回调乱序/重复 | 订单状态错乱 | 单调状态机 + dedupe + 必要时回查权威状态 |
| 金额精度 | 对账差异 | 最小单位整数 + 指数表 + 严格解析；拒绝浮点 |
| 多商户/服务商模式 | 无法服务 SaaS | 已交付多账户：`AccountId` + `AccountSet` + 双段回调路由 |
| 自研维护成本 | 长期负担 | 操作面最小化；每供应商单一 owner；文档写清「不做全量 API」 |
| 体积/编译 | 应用变慢 | 适配器独立 crate、opt-in；不引入 async-stripe |

上述决策均已落地（银联全渠道除外，按需求评估）；后续 minor 只做适配器能力扩展
（订阅/分账/银联）与协议版本跟进。

**运行时配置已实现**：`IPaymentServiceFactory` 在运行时按 `PaymentScope`
（provider/tenant/account）解析 provider 实例；默认工厂沿用现有 appsettings，业务可从
数据库/KMS 提供配置。设计取舍见 [运行时支付服务工厂](payment-runtime-factory.md)。

---

## 11. 参考

- 框架内部：[框架宪法](framework-constitution.md)、[crate 分层](../rust-webx/04-architecture/crate-layout.md)、
  [出站 HTTP](../rust-webx/13-extensibility/outbound-http.md)、[配置](../rust-webx/10-configuration/appsettings.md)、
  [稳定性门禁](stability-gates.md)。
- Stripe：[SDK 列表](https://docs.stripe.com/sdks)、[Webhook 签名](https://docs.stripe.com/webhooks/signature)、
  [幂等请求](https://docs.stripe.com/api/idempotent_requests)。
- 支付宝：[自行实现签名](https://opendocs.alipay.com/common/057k53)、
  [签名与验签](https://opendocs.alipay.com/pre-open/common/104741)、[SDK 下载](https://opendocs.alipay.com/support/01rayh)。
- 微信支付：[请求签名](https://pay.weixin.qq.com/doc/v3/merchant/4012365336)、
  [Authorization 示例](https://pay.weixin.qq.com/doc/v3/merchant/4012365344)、
  [应答验签](https://pay.weixin.qq.com/doc/global/v3/zh/4012354989)、
  [官方 SDK 语言](https://pay.weixin.qq.com/doc/global/v3/zh/4012354960)。
- PayPal：[Verify webhook signature](https://developer.paypal.com/api/webhooks/v1/verify-webhook-signature-post)、
  [Server SDKs](https://github.com/paypal/paypal-server-sdks)。
- Adyen：[Verify HMAC signatures](https://docs.adyen.com/development-resources/webhooks/secure-webhooks/verify-hmac-signatures)。
- Square：[Validate an event notification](https://developer.squareup.com/docs/webhooks/step3validate)。
- 社区实现对照：[async-stripe](https://github.com/arlyon/async-stripe)。
- 编排器备选：[Hyperswitch](https://github.com/juspay/hyperswitch)。
