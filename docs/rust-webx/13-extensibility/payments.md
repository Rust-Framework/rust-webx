# 支付：统一运行时、官方协议适配器与多租户配置

> 适用：需要接入 Stripe、支付宝、微信支付、PayPal、Adyen、Square 的业务服务。
> 目标：同一段业务代码完成下单、查询、撤销、退款与回调处理；供应商差异收敛在适配器内部；
> 配置既可以来自启动期 options，也可以由运行时 `IPaymentServiceFactory` 从 DB/KMS 提供。

支付是**领域能力**，不在 `rust-webx` 汇总 crate 里：应用显式依赖 `rust-webx-pay*` 才引入。
所有适配器共用一条出站 HTTP 栈（`rust-webx-client`），不引入第二套 `reqwest`/`hyper`。

## 1. Crate 一览

| crate | 导入名 | 职责 |
|---|---|---|
| `rust-webx-pay` | `webx_pay` | 统一模型、端口、`Payments` 运行时、`IPaymentServiceFactory`、回调请求类型 |
| `rust-webx-pay-stripe` | `webx_pay_stripe` | PaymentIntents / Checkout、退款、`Stripe-Signature` 验签 |
| `rust-webx-pay-alipay` | `webx_pay_alipay` | 电脑/手机网站、App、当面付，RSA2 签名与异步通知（公钥/证书模式） |
| `rust-webx-pay-wechatpay` | `webx_pay_wechatpay` | APIv3 Native/JSAPI/App/H5，请求签名、平台证书/公钥验签、AEAD 解密 |
| `rust-webx-pay-paypal` | `webx_pay_paypal` | Orders v2、OAuth2 令牌缓存、capture/void/refund、验签 API |
| `rust-webx-pay-adyen` | `webx_pay_adyen` | Checkout `/payments`、capture/cancel/refund、HMAC 回调 |
| `rust-webx-pay-square` | `webx_pay_square` | Payments/Refunds、Payment Links、HMAC 回调 |
| `rust-webx-pay-ef` | `webx_pay_ef` | 回调 inbox 的**参考适配器**：基于 rust-ef 的唯一去重、重试、死信。位于 `examples/adapters/payment/ef`，`publish = false`，复制进业务仓使用（不随框架发布） |

## 2. 快速开始（运行时配置）

每个适配器一个 `add_*()`：只声明启用哪个供应商，凭据/商户账户从对应配置节读取。

```toml
[dependencies]
rust-webx = "2.9"
rust-webx-pay = "2.9"
rust-webx-pay-stripe = "2.9"
```

```rust
use webx::Host;
use webx_pay_stripe::StripeHostBuilderExt;

let host = Host::builder()
    .add_stripe()   // 读取 "Stripe" 配置节，文件热更由 IOptionsMonitor 承载
    .build();
```

对应的 `appsettings.json`：

```json
{
  "Stripe": {
    "accounts": [
      { "id": "default", "secret_key": "", "webhook_secret": "" }
    ],
    "default_account": "default",
    "timeout_seconds": 30
  }
}
```

环境覆盖：`APP__Stripe__Accounts__0__SecretKey`。没有 `add_payments()`：第一个 `add_*`
会幂等创建支付运行时，注册顺序无关。

需要代码内静态配置（测试、脚本、单租户）时用 `add_*_with(options)`：

```rust
use webx_pay_stripe::{StripeHostBuilderExt, StripeOptions};

let host = Host::builder()
    .add_stripe_with(StripeOptions::new("<secret-key>", "<webhook-secret>"))
    .build();
```

两种方式对业务代码完全透明：都进入同一个 `Payments`、同一条缓存与错误路径。

## 3. 业务 Handler

```rust
use std::sync::Arc;
use webx::Inject;
use webx_pay::{CreatePayment, Currency, Money, PaymentFlow, PaymentMethod, PaymentScope, Payments, ProviderId};

#[derive(Inject)]
pub struct CheckoutHandler {
    #[inject]
    pub payments: Arc<Payments>,
}

let scope = PaymentScope::from_identity(ProviderId::Stripe, &req.user)?
    .account("acct_eu"); // 可选；不传用默认账户

let intent = self
    .payments
    .create(
        scope,
        CreatePayment::new(
            order_id,
            Money::new(Currency::CNY, 9900), // ¥99.00
            PaymentMethod::Card,
            PaymentFlow::ClientSdk { return_url: None },
        )
        .with_description("Pro plan")
        .with_metadata("user_id", user_id),
    )
    .await?;
```

- 单租户/默认账户可简写：`payments.create(ProviderId::Stripe, request)`。
- `PaymentScope::from_identity(provider, &req.user)` 从可信身份取 tenant；`PaymentScope` 是
  provider + tenant + account 的唯一载体，tenant **绝不**从 body/query 进入。
- `intent.client_action` 表达下一步：`Redirect { url }`、`QrCode { content }`、
  `AppOrder { payload }`、`ClientSdk { payload }`。
- 查询/捕获/撤销/退款同样传 scope：`payments.get(scope, order_id)`、
  `payments.capture(scope, order_id, Some(amount))`、`payments.refund(scope, command)`。

### 3.1 前端支付方式目录

结算页不需要硬编码“支持哪些支付方式”：`Payments` 从当前运行时配置/自定义工厂实时返回
公共目录（`PaymentOption`），包含 provider、account、method、可用 flow 和 **非敏感** 的
`client_config`（Stripe publishable key、Adyen client key、PayPal client id、Square
application id、Alipay/WeChat app id 等）。框架不隐式注册路由，应用用普通 Handler 暴露：

```rust
use webx::*;
use webx_pay::{PaymentOption, Payments};

#[derive(Debug, WebxRequestMeta)]
struct PaymentOptionsRequest {
    #[fromclaim] user: IdentityUser, // 目录按可信 tenant 过滤；未登录直接 401
}

#[get("/api/payments/options")]
impl IRequest<Response> for PaymentOptionsRequest {}

#[derive(Inject)]
struct PaymentOptionsHandler {
    #[inject] payments: Arc<Payments>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<PaymentOptionsRequest, Vec<PaymentOption>> for PaymentOptionsHandler {
    async fn handle(&mut self, req: PaymentOptionsRequest) -> ActionResult<Vec<PaymentOption>> {
        Ok(self.payments.payment_options_for(&req.user, None).await?)
    }
}
```

- `PaymentOption` 字段：`provider`/`provider_name`、`account`、`method`、`flows`
  （`client_sdk`/`redirect`/`qr_code`/`app`）、`display_name`、`description`、`icon`、
  `brand_color`、`sort_order`、可选 `client_config`、可选 `label`；前端据此构建支付页。
- `icon` 是稳定 key（`card`/`alipay`/`wechatpay`/`paypal`/`apple_pay`/`google_pay`），
  框架不打包图片，前端按 key 映射自己的图标资源；`display_name`/`description` 可由前端
  本地化或用 `PaymentOption::with_display_name/with_description` 覆盖。
- `sort_order` 是展示顺序提示；同值时保留适配器/配置顺序。
- 请求可带金额/币种：`payment_options_for(&user, Some(&money))` 按 provider 声明的 currency
  能力过滤（例如 CNY-only 渠道不会出现在 USD 订单里）。
- 目录是**公共数据**：适配器只放入 publishable/app/client/location 等公开标识，绝不包含
  secret key、私钥、webhook secret。
- 默认工厂遍历已注册 provider，是**部署级目录**（没有租户归属信息）；带 tenant 的
  `payment_options_for` 默认工厂会 fail-closed（500），多租户必须注册自定义
  `IPaymentServiceFactory` 并在其中覆写 `payment_options`。
- 自定义工厂的 `payment_options` 应返回当前租户启用的 provider/account/method/flow；
  返回的数据同样只能是公开配置。
- 目录不做长缓存（每次请求实读），保证配置变更后前端立即看到；若 DB 压力大，应用可在
  自己的工厂里做短 TTL 缓存。
- 静态 `add_*_with(options)` 与运行时 `add_*()` 都会出现在同一目录中。

## 4. 统一模型

- **金额**永远是整数最小单位 + ISO 4217：`Money::new(Currency::CNY, 9900)`。
  `Currency` 自带指数表（CNY=2、JPY=0、KWD=3），适配器负责与供应商字符串/整数互转，没有浮点。
- **支付状态**：`Created / RequiresAction / Processing / Authorized / Succeeded / Canceled / Failed / Refunded / PartiallyRefunded / Unknown`。
  供应商原始状态保留在 `PaymentIntent::provider_status`，未知状态不会猜成成功。
- **客户端动作**`ClientAction` 只表达「下一步做什么」，供应商专有参数放 `payload`。
- **错误**`PaymentError` 带 `retryable()`、`http_status()` 与 `Into<webx::Error>`；
  `Unavailable` 映射 503（配置源暂不可用，可重试），Handler 里 `?` 直接得到 problem+json。
- **能力位**`provider.capabilities()` 声明 capture/cancel/refund/partial_refund/hosted_checkout/qr/app
  与回调签名方式；未实现的能力返回 `PaymentError::Unsupported`，不会静默成功。

## 5. 回调入口

回调签名是对**原始 body** 计算的，所以不能用普通 JSON 请求模型。crate 提供手写
`PaymentWebhook`（原始字节 + 全部请求头）。由于 Rust 的孤儿规则，应用不能直接为
这个外部类型实现路由，因此用 crate 提供的宏生成一个本地请求类型：

```rust
use webx::*;
use webx_pay::{payment_webhook, Payments};

payment_webhook!(StripeWebhook, "/webhooks/payments/{provider}");

#[derive(Inject)]
pub struct PaymentWebhookHandler {
    #[inject]
    pub payments: Arc<Payments>,
    #[inject]
    pub orders: Arc<dyn OrderService>, // 应用自己的端口
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<StripeWebhook, Response> for PaymentWebhookHandler {
    async fn handle(&mut self, req: StripeWebhook) -> ActionResult<Response> {
        let event = self.payments.verify_webhook(&req).await?; // 验签 + 规范化
        self.orders.apply_payment_event(event).await?;         // 业务事务
        Ok(self.payments.ack(&req).await?)           // 供应商要求的 ACK
    }
}
```

- 宏生成的类型 `Deref` 到 `PaymentWebhook`，`req.provider()` / `req.account()` 直接可用；
  宏展开需要 `IMediator` 在作用域内（`use webx::*` 即可）。
- 回调路由**不要**加 `#[authorize]`：签名即认证。
- 未知供应商 404、验签失败 400、配置源不可用 503（让供应商重投）；未验签数据绝不进入业务。
- ACK 由适配器决定：Stripe 200、支付宝字面量 `success`、微信 `{"code":"SUCCESS"}`、Adyen `[accepted]`。
- 同一条路由覆盖全部供应商；需要按供应商做独立 IP 白名单/限流时，为每个供应商单独声明
  路由与生成类型。
- 账户段也是同一个宏：`payment_webhook!(AccountWebhook, "/webhooks/payments/{provider}/{account}")`，
  account 自动从路径绑定，无需第二个请求类型或第二个 verify 方法。

## 6. 多账户 / 服务商模式

一个适配器实例可以持有多个商户账户（不同 mch_id / app_id / 密钥 / 回调密钥）。
账户在 `PaymentScope` 中选择，不放在 `CreatePayment` 里：

```rust
let stripe = StripeOptions::new("<default-key>", "<default-webhook-secret>")
    .with_account("acct_eu", "<eu-key>", Some("<eu-webhook-secret>".into()))
    .with_default_account("acct_eu");

Host::builder().add_stripe_with(stripe).build();

// 业务侧：
let intent = payments
    .create(PaymentScope::new(ProviderId::Stripe).account("acct_eu"), request)
    .await?;
```

运行时配置下账户来自 `accounts[]`，选择方式相同。多账户下没有账户段的回调会被拒绝
（无法确定验签密钥），这是失败关闭而不是猜测默认账户。

## 7. 运行时配置：自定义 `IPaymentServiceFactory`

默认工厂从各供应商配置节读取。需要从数据库/配置中心/KMS 动态提供配置时，实现
`IPaymentServiceFactory`，然后**像普通 DI 单例一样注册**；业务代码不变：

```rust
use std::sync::Arc;
use async_trait::async_trait;
use rust_dix::{IServiceResolverExt, ScopeFactory};
use webx::Host;
use webx_pay::{
    IPaymentProvider, IPaymentServiceFactory, Currency, PaymentError, PaymentOption,
    PaymentProviderBuilders, PaymentScope, TenantId,
};

struct DbPaymentServiceFactory {
    scopes: Arc<dyn ScopeFactory>,
    builders: Arc<PaymentProviderBuilders>,
}

#[async_trait]
impl IPaymentServiceFactory for DbPaymentServiceFactory {
    async fn resolve(&self, scope: &PaymentScope)
        -> Result<Arc<dyn IPaymentProvider>, PaymentError>
    {
        let db = self.scopes.create_scope();
        let ctx: DbContext = db.get_owned()?;
        let row = load_enabled_payment_config(&ctx, scope).await?; // 应用自己的表
        let config = row.to_provider_config()?;                    // settings JSON + version
        self.builders.build(scope.provider(), &config)             // 复用官方协议实现
    }

    async fn probe(&self) -> Result<(), PaymentError> {
        // 轻量 DB 探针；readiness 使用
        Ok(())
    }

    async fn payment_options(
        &self,
        tenant: Option<&TenantId>,
        currency: Option<&Currency>,
    ) -> Result<Vec<PaymentOption>, PaymentError> {
        // 返回当前租户启用的目录；只放公开 client_config
        let db = self.scopes.create_scope();
        let ctx: DbContext = db.get_owned()?;
        load_enabled_payment_options(&ctx, tenant, currency).await
    }
}

Host::builder()
    .add_stripe()
    .register(|services| {
        // 原生 DI 注册；框架默认工厂是 try_add，后注册即覆盖（MEDI last wins）
        services.singleton::<dyn IPaymentServiceFactory>(|resolver| {
            Arc::new(DbPaymentServiceFactory {
                scopes: resolver.resolve_arc::<dyn ScopeFactory>().expect("scopes"),
                builders: resolver
                    .resolve_arc::<PaymentProviderBuilders>()
                    .expect("builders"),
            })
        })
    })
    .build();
```

- `IPaymentServiceFactory` 是**普通 DI 单例**：不需要 `use_payment_service_factory*` 之类的
  支付专用入口。框架用 `try_add_singleton` 注册默认工厂；应用在 `add_*` 之前或之后用
  `singleton`/`instance` 注册自己的实现，都按 dix 的“后注册覆盖”语义生效。
- 自定义工厂若要支持前端目录，覆写 `payment_options(&self, tenant, currency)`，返回当前租户启用的
  `PaymentOption` 列表；默认实现返回空列表（结算页只看到内置工厂的目录）。
- 工厂依赖（`ScopeFactory`、`PaymentProviderBuilders`、`ISecretStore` 等）由 DI 闭包
  `resolve_arc` 注入；工厂本身无需关心容器。
- 缓存策略同样是原生 DI：注册 `PaymentRuntimeOptions` 即可覆盖默认的 60s/1024，
  ```rust
  services.instance::<PaymentRuntimeOptions>(Arc::new(
      PaymentRuntimeOptions::default().with_cache_ttl(Duration::from_secs(15)),
  ))
  ```
  不注册就用默认值，不需要额外 API。
- `DbPaymentServiceFactory` 只依赖 `webx_pay`（端口/注册表）和它自己的 DB/KMS 栈，
  **不依赖** Stripe/支付宝等适配器 crate；适配器由宿主链接并注册 builder。
- `settings` 使用与 `XxOptions` 相同的 serde 结构；密钥引用在工厂内解析/注入，
  不要把明文密钥写进配置展示或日志。
- `Payments` 按 `(provider, tenant, account)` 缓存解析结果（默认 TTL 60s、容量 1024、
  并发单飞）。配置更新后调用 `payments.invalidate(scope)` 立即生效；多实例可用
  `IEventPublisher` 广播失效，TTL 保底。
- 默认工厂把 `tenant` 当作缓存隔离键，但配置仍来自同一部署级配置节：**需要按租户使用
  不同凭证时，必须注册自定义 `IPaymentServiceFactory`**，默认工厂不会因为 tenant 不同而
  切换商户账户。
- 工厂故障返回 `PaymentError::Unavailable`（503），且失败不进缓存；回调场景让供应商重投。

## 8. 可靠性

支付回调是**至少一次**投递，重复与乱序是常态：

| 环节 | 做法 |
|---|---|
| 创建幂等 | `order_id` 是幂等锚点；适配器映射到各供应商幂等键（Stripe `Idempotency-Key`、PayPal `PayPal-Request-Id`、支付宝/微信 `out_trade_no`） |
| 回调去重 | 每个事件有稳定 `dedupe_key`；用唯一索引落库，缓存只能做快路径 |
| 乱序 | 只允许状态单调推进；退款事件先到不会把订单回退 |
| 查证 | 状态不确定时用 `payments.get(scope, order_id)` 拉权威状态 |
| 持久化 | 生产环境把 `examples/adapters/payment/ef` 的 inbox 复制进业务仓：先落库再 ACK，后台 worker 调业务处理器，失败退避/死信 |
| 对账 | 用框架 `PeriodicBackgroundService` 对超时未终态订单调用 `get`，不要自己 spawn 轮询线程 |

没有 inbox 时，回调 Handler 应在**同一事务**内完成验签后的订单状态写入；不要先 ACK 再异步做事。

### 8.1 持久化 inbox（生产推荐，参考适配器）

`examples/adapters/payment/ef` 提供「先落库、再 ACK、后台处理」的通用 inbox 参考实现
（`publish = false`：复制进业务仓，改成自己的包名与实体）：
唯一 `(provider, dedupe_key)`、租约领取、指数退避、死信；表结构由 rust-ef 官方迁移生成，
运行期不建表。下面代码里的 `webx_pay_ef` 是参考实现的导入名，复制后换成你自己的。

```rust
use std::sync::Arc;
use webx::Host;
use webx_pay::PaymentEvent;
use webx_pay_ef::{IPaymentEventHandler, PaymentInboxHostBuilderExt, PaymentInboxOptions};

struct OrderEvents; // 应用自己的幂等业务处理

#[async_trait::async_trait]
impl IPaymentEventHandler for OrderEvents {
    async fn handle(&self, event: PaymentEvent) -> Result<(), webx_pay::PaymentError> {
        // 按 event.order_id / event.dedupe_key 幂等落库
        Ok(())
    }
}

let host = Host::builder()
    .register(|services| services.add_dbcontext(|options| { /* ... */ }))
    .add_payment_inbox_with_handler(PaymentInboxOptions::default(), Arc::new(OrderEvents))
    .build();
```

回调 Handler 只做「验签 → 落 inbox → ACK」：

```rust
let event = self.payments.verify_webhook(&req).await?;
self.inbox.record(event).await?; // 重复投递返回 Duplicate，不重复入队
Ok(self.payments.ack(&req).await?)
```

`PaymentInboxOptions` 控制 `enabled`、`interval_seconds`、`batch_size`、`lease_seconds`、
`max_attempts`、`retention_days`；`enabled=false` 只关闭 drain worker，端口仍可落库。

## 9. 配置参考

每个适配器一个顶层配置节：`Stripe`、`Alipay`、`WeChatPay`、`PayPal`、`Adyen`、`Square`。
`add_*()` 自动绑定并监听该节，不需要手动 `add_options`。

```json
{
  "Stripe": {
    "accounts": [
      {
        "id": "default",
        "secret_key": "",
        "webhook_secret": "",
        "publishable_key": "pk_...",
        "api_version": "2025-xx-xx"
      }
    ],
    "default_account": "default",
    "timeout_seconds": 30
  },
  "Alipay": { "accounts": [] },
  "WeChatPay": { "accounts": [] },
  "PayPal": { "accounts": [] },
  "Adyen": { "accounts": [] },
  "Square": { "accounts": [{ "application_id": "sq0idp-..." }] }
}
```

生产环境的私钥/API Key 建议从 `ISecretStore`（文件/环境/KMS）读取后构造 options，不写进镜像；
所有 options 的 `Debug` 都已脱敏。运行时工厂从 DB 读取时，DB 只存 secret ref，不存明文。

## 10. 供应商能力矩阵

| 能力 | Stripe | 支付宝 | 微信支付 v3 | PayPal | Adyen | Square |
|---|---|---|---|---|---|---|
| 下单 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| 查询 | ✓ | ✓ | ✓ | ✓ | ✓（details） | ✓（按支付 ID） |
| 手动捕获 | ✓ | — | — | ✓ | ✓ | ✓ |
| 撤销/关闭 | ✓ | ✓ | ✓ | ✓（void） | ✓ | ✓ |
| 退款/部分退款 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| 托管页 | ✓ | ✓ | ✓（H5） | ✓ | ✓ | ✓（Payment Links） |
| 扫码 | — | ✓ | ✓ | — | 视渠道 | — |
| App 参数 | — | ✓ | ✓ | — | — | — |
| 回调验签 | HMAC | RSA2 | RSA + AES-GCM | 验签 API | HMAC | HMAC |

以各 crate 的 `ProviderCapabilities` 为准；调用前可用 `capabilities()` 做能力探测。

## 11. 安全边界

- 服务端**不接触卡号**：卡数据只经过供应商托管页或客户端 SDK；服务端只处理 token。
- 私钥/API Key 只从配置、环境、`ISecretStore` 或运行时工厂进入；`Debug` 脱敏，日志不打印
  签名串与密钥。
- tenant 只来自 `IdentityUser`；account 来自业务/回调路径；缓存键包含 tenant/account，
  跨租户不会共享 provider 实例。
- 回调验签使用常量时间比较；Stripe 300s、微信 5 分钟重放窗口；Adyen/Square 依赖事件去重。
- 微信平台证书按 serial 缓存，未知 serial 触发限流刷新；证书过期/缺失会让 readiness 失败。
- 出站只接受 `https://`（测试可用 `allow_private_networks` 连本地 mock）；SSRF 防护保持开启。

## 12. 测试与沙箱

- 适配器自带离线测试：签名串构造、RSA/HMAC/AES-GCM 向量、回调解析、状态与金额映射。
- `rust-webx-client` 的 `MockUpstream` 可回放供应商响应，不触网即可测请求签名与错误映射。
- 真实环境：Stripe 测试模式、支付宝沙箱、PayPal sandbox、Adyen test、Square sandbox；
  微信支付 v3 没有公开沙箱（以官方说明为准），用官方验收用例 + 回放覆盖。
- 回调幂等/乱序/重试用 inbox 的测试覆盖（`cargo test --manifest-path examples/adapters/Cargo.toml`）。
- 运行时缓存/单飞/失效/租户隔离的测试在 `rust-webx-pay` 的 `payments` 模块内。

## 13. 与第三方 SDK / 旧代码共存

已经使用 `async-stripe` 等服务商 SDK 的项目不需要迁移：实现一个 `IPaymentProvider` 把现有调用
包起来，通过 `add_payment_provider` 注册即可；统一模型之外的全量 API 继续用原 SDK。

旧版 2.7 支付代码的 `add_*(options)`（静态配置）等价于新的 `add_*_with(options)`；
`add_*_from_config()` 等价于新的 `add_*()`；`add_payments()` 已删除，由 `add_*` 自动装配。

## 14. 已知差异与操作约定

统一模型不掩盖供应商 API 的真实差异；下单返回的 `PaymentIntent::provider_payment_id` 通常就是
后续查询/退款要传的值。各适配器的约定如下：

| 供应商 | 查询/退款参数 | 说明 |
|---|---|---|
| Stripe | `pi_...` / `cs_...` / `re_...`，或订单号 | 订单号查询走 PaymentIntents Search（有索引延迟）；退款先解析 PaymentIntent |
| 支付宝 | 订单号；退款查询为 `out_trade_no:out_request_no` | 证书模式使用配置的支付宝公钥证书；证书序列号按官方 SDK 算法（issuer + 十进制 serial 的 MD5） |
| 微信支付 | `out_trade_no` / `out_refund_no` | 只支持 CNY；平台证书首次下载按官方规定豁免应答验签；可用 `refresh_platform_certificates()` 做定时轮换 |
| PayPal | PayPal order / capture / refund id | Orders v2 没有按商户订单号查询；部分捕获只在手动捕获（AUTHORIZE）下支持 |
| Adyen | pspReference；capture/refund 需显式金额 | Checkout 没有文档化的支付状态 GET；用 `get_payment_with_details`（需要前端 details）；批量通知用 `verify_all` |
| Square | Square payment / refund id | 没有按商户参考号查询；下单需要客户端 token（metadata `square_source_id`）；幂等键在 JSON body |

其他约定：

- **状态语义**：`PaymentStatus::Authorized` 只在支持手动捕获的适配器出现；`Refunded` 与
  `PartiallyRefunded` 以供应商原因为准，原始状态始终保留在 `provider_status`。
- **金额**：适配器只发送最小单位整数或按币种指数格式化后的十进制字符串；不接受浮点输入。
- **多账户回调**：没有 `{account}` 路径段且配置了多个账户时，验签失败关闭（无法确定密钥）。
- **错误**：供应商 401/403 视为本端配置/凭证问题（502），429/5xx 标记可重试（503/502），
  其余按 4xx 校验错误返回；响应不回显密钥或完整供应商原文。

## 15. 发布检查清单

支付上线前逐项确认（前六项是框架/业务边界）：

1. **装配**：生产使用 `add_*()`（配置节）或 `add_*_with(options)`；没有 `add_payments()`，
   所有支付 provider 都由 `add_*` 自动创建 `Payments`。
2. **配置与密钥**：`Jwt.Secret`/`SecretFile`、数据库、支付密钥都来自环境/KMS，不进镜像；
   Stripe 的 `publishable_key`、Square 的 `application_id`、Adyen 的 `client_key` 等公开
   配置可进配置节，但 secret 字段不能出现在日志/前端响应里。
3. **前端目录**：用带认证的 Handler 调 `Payments::payment_options_for(&user, Some(&money))`；
   默认工厂是部署级，多租户必须用自定义 `IPaymentServiceFactory` 并覆写
   `payment_options(tenant, currency)`；目录响应中不得含密钥。
4. **回调**：`payment_webhook!` 声明路由；签名失败 400、配置源不可用 503、未知 provider 404；
   生产建议“验签 → 落 inbox（复制自 `examples/adapters/payment/ef`）→ ACK”，业务 worker 再处理。
5. **身份与作用域**：`PaymentScope` 的 tenant 只来自 `IdentityUser`/`#[fromclaim]`；account 只来自
   业务或回调路径；不要信任 body/query 的 tenant/account。
6. **运行时变更**：自定义工厂更新 DB 后调用 `payments.invalidate(scope)`；跨实例用短 TTL
   （`PaymentRuntimeOptions`）或事件广播；前端目录每次请求实读，保证配置变更可见。
7. **readiness**：静态 provider 自带 `payments.<provider>` 健康检查；自定义工厂实现
   `IPaymentServiceFactory::probe()`（`Payments::probe()` 暴露给应用健康组件）。
8. **能力边界**：本域只覆盖下单/查询/捕获/撤销/退款/回调；订阅、分账、payout、dispute、
   客户/支付方式管理、供应商全量 API 不在范围内，需要时实现 `IPaymentProvider` 扩展。
9. **沙箱/回放**：Stripe test、支付宝沙箱、PayPal sandbox、Adyen test、Square sandbox 至少
   跑一次端到端；微信 v3 用官方验收用例 + 离线向量；回调重试/乱序用 inbox 测试覆盖。
10. **可观测性**：日志只打 provider/account/version 与 `trace_id`，不打 body/签名/密钥；
    payment 失败按 `PaymentError::http_status()` 映射；回调 5xx 应触发供应商重投。
11. **容量**：确认 `PaymentRuntimeOptions` 的 TTL/容量适合商户数量；多实例下缓存键包含
    tenant/account；回调入口限流阈值覆盖供应商重试流量。
12. **发布内容**：`CHANGELOG`、手册、`docs/design/payments.md`、适配器文档与
    `check-docs`/`check-crate-layout`/`check-publish-order` 同批更新。
