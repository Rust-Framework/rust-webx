# 运行时支付服务工厂与 API 收敛（架构评审）

> 状态：**已落地（2.7.0）**。本文记录 `add_payments()` 去留、静态/运行时模式统一、接口命名
> 的评审结论与代码审查发现；实现结果见
> [支付手册](../rust-webx/13-extensibility/payments.md) 与 [支付域设计](payments.md)。
> §2 行号与 §7 清单保留为评审时的快照，便于追溯“为什么这样改”。

## 落地状态

| 结论 | 状态 |
|---|---|
| 删除 `add_payments()`/`PaymentStartupCheck`，`add_*` 幂等装配 `Payments` | ✅ |
| `add_*()` 运行时 + `add_*_with(options)` 静态；删除 `_from_config`/`_with_client`/ServiceCollection ext | ✅ |
| `PaymentScope`（provider/tenant/account）+ `CreatePayment`/`RefundCommand` 移除 account | ✅ |
| `IPaymentServiceFactory`/`IPaymentProviderFactory`/`IPaymentProviderBuilder`/默认工厂 | ✅ |
| `Payments` TTL 缓存/单飞/失效/容量上限；`PaymentError::Unavailable` | ✅ |
| 单一 `PaymentWebhook`/`payment_webhook!`/`verify_webhook`；删除账户专用类型 | ✅ |
| 适配器 factory/builder 与健康检查 | ✅ |
| 自定义工厂用原生 DI 注册（`try_add` 默认 + 后注册覆盖），无 `use_*` API | ✅ |
| 文档/CHANGELOG/门禁同步 | ✅ |
| readiness 由各 provider 健康检查/自定义工厂 `probe` 承担；启动不再强校验支付配置 | ✅（权衡见 §8） |


## 0. 结论摘要

1. **`add_payments()` 是冗余设计，应删除**。它是“装配仪式”，不是业务概念；每个
   `add_stripe()` 已经表达了“启用 Stripe”，框架应自行保证 `Payments` 运行时、缓存、
   注册表只装配一次。仓库自己的 `add_redis` 就是一次调用同时注册 cache、publisher、
   consumer、health —— 支付不应破坏这个先例。
2. **静态与运行时不是两套设计，而是同一个 `IPaymentServiceFactory` 的两种来源**：
   - 静态：`add_stripe_with(options)` 注册一个“预构造 provider”的 provider factory；
   - 运行时：`add_stripe()` 注册一个“按作用域解析配置”的 provider factory；
   - 两者都进入同一个 hub、同一条缓存/错误/可观测路径。用户只看到“带配置/不带配置”的
     区别，而不是两个架构。
3. **接口收敛为四个概念**：`Payments`（门面）、`PaymentScope`（provider/租户/账户）、
   `IPaymentServiceFactory`（应用级运行时源）、`IPaymentProviderFactory`（适配器级
   provider 源，内部扩展点）。`IPaymentProviderBuilder` 只给自定义工厂复用协议实现，
   普通用户不接触。
4. **精确的 `.add_stripe()` 可行且已在 2.7 定稿**：支付 crate 是 2.7 新增；
   直接改为 `.add_stripe()`（运行时）＋ `.add_stripe_with(options)`（静态）。
   Rust 不支持同名方法按参数个数重载；两个扩展 trait 同时引入会 `E0034`（已用 rustc 验证）。
5. **账户选择只能有一个入口**：放进 `PaymentScope`，从 `CreatePayment`/`RefundCommand`
   和 `get/capture/cancel` 的独立参数中移除；tenant 同理，只来自可信身份。
6. **回调只保留一个请求类型/一个宏/一个 verify 方法**：account 作为可选路径参数解析，
   不再有 `PaymentAccountWebhook`/`verify_account_webhook`。
7. **启动校验让位于 readiness**：运行时配置在启动时不可知，`PaymentStartupCheck` 与
   `add_payments()` 一起删除；静态配置由 provider 构造/健康检查校验，失败 503。
8. **依赖方向保持正确**：业务自定义工厂只依赖 `webx_pay`（端口/注册表），不依赖任何
   适配器 crate；适配器 crate 仍只依赖 core/client/host，不反向依赖宿主应用。

---

## 1. 第一性原理：支付注册到底需要几个概念？

从“一次支付调用需要什么”倒推：

```text
调用：payments.create(scope, request)
  需要：1) 一个能执行该 provider 协议的实例
        2) 该实例的配置/凭证/回调密钥
        3) 一个稳定的缓存与失效边界
```

- 第 1 项是**适配器**（Stripe/Alipay/...），编译期确定。
- 第 2 项是**配置来源**：启动期 options 或运行时 DB/KMS。
- 第 3 项是**运行时装配**（facade + cache），由框架负责。

`add_payments()` 不属于以上任何一项：它只是在“已经注册了 provider 之后”再注册一个 hub。
如果把第 3 项做成隐式、幂等的自动装配，这个 API 就没有存在理由。

**判断标准**：删除一个 API 后，用户是否还能用一句话表达意图？

```rust
// 删除前：两句话、两个概念
.add_stripe(options)
.add_payments()

// 删除后：一句话
.add_stripe()
```

删除后表达更直接，且没有信息丢失（hub/缓存/注册表是框架内部实现）。因此应删除。

---

## 2. 对 2.7 现有代码的审查

> 结论：功能可用，但存在“装配仪式、重复入口、重复类型、账户入口分散”四类结构问题；
> 引入运行时工厂前必须先收敛，否则会把两套设计固化。

| # | 严重度 | 位置 | 问题 | 修复方向 |
|---|---|---|---|---|
| F1 | 高 | `crates/payment/core/src/register.rs:41/66/89/93` | `add_payments()` 冗余；忘记调用则 DI 报错；重复调用会注册两个 hosted startup check | 删除；每个 `add_*` 自动幂等装配 `Payments` |
| F2 | 高 | `register.rs:70` | `PaymentStartupCheck` 又构造了一个 `PaymentHub::from_resolver`，与 singleton hub 不是同一个实例；将来会复制缓存/工厂调用 | 删除 startup check；readiness/health 校验 |
| F3 | 高 | `register.rs:32/54` | `add_payment_provider_lazy` 同时注册具体类型和 `dyn IPaymentProvider` 两个 factory，各自调用一次构造闭包 → 两个 provider 实例（HTTP 连接池、证书缓存都翻倍） | 只注册一个实例/一个 factory，具体类型从同一 `Arc` 派生 |
| F4 | 高 | 六个适配器 `provider.rs` | 每个适配器三个 HostBuilder 方法（`add_*`、`_with_client`、`_from_config`）＋ ServiceCollection ext；44 处入口，命名/语义重复 | 收敛为 `add_*()`（运行时）＋ `add_*_with(options)`（静态）；测试用 `ServiceCollection`/provider 构造器 |
| F5 | 高 | `core/src/webhook.rs:46/130/281`、`hub.rs:198/212` | `PaymentWebhook` 与 `PaymentAccountWebhook` 两套类型/两条 verify 路径/两种宏参数；逻辑重复，易漂移 | 一个 `PaymentWebhook`，account 为可选路径参数；一个 `payment_webhook!`、一个 `verify_webhook` |
| F6 | 中 | `CreatePayment.account`、`RefundCommand.account`、`get/capture/cancel(account, ...)` | 账户选择分散在请求字段 + 方法参数；与未来的 `PaymentScope` 三处重复 | 账户只进 `PaymentScope`；请求/方法参数移除 account |
| F7 | 中 | `hub.rs:85/118` | `provider()` 同步、`create()` 异步；`get/capture/cancel` 参数形态不一致 | 统一 `Payments` 门面：`scope: impl Into<PaymentScope>` + 异步方法 |
| F8 | 中 | 全局 | 没有 `PaymentScope`/tenant 概念；运行时工厂无法安全地按租户/账户缓存 | 新增 `PaymentScope`；tenant 只来自 `IdentityUser` |
| F9 | 中 | `core/src/error.rs` | 没有“配置源暂时不可用”的 503 语义；DB 故障会被映射成 500 | 增加 `PaymentError::Unavailable`（503，可重试） |
| F10 | 中 | 文档/示例 | `add_payments()` 出现 41 次；`_from_config`/`_with_client` 44 处；用户侧概念过多 | 重构同批更新手册/设计/CHANGELOG |
| F11 | 低 | `core/Cargo.toml:20-23` | core 依赖 `rust-webx-host`（为 HostBuilder 扩展/健康检查）；纯端口消费者也被迫拉入 host/engine | 可接受（redis 先例）；如未来需要非 Web 消费者，再拆 `pay/host` |
| F12 | 低 | `PaymentHub` 命名 | “hub” 是技术词；用户注入后调用 `payments.create(...)` 更自然 | 建议改为 `Payments`（门面），`PaymentHub` 作为别名不做 |

> 说明：F3 是当前静态路径就可能出现的实例重复；F2 在只有静态 provider 时影响小，但一旦
> 接入运行时工厂，两个 hub 会各自缓存/各自调用工厂，必须消除。

---

## 3. 对抗辩论：`add_payments()` 去留

### 3.1 支持保留的论点

1. **显式组合根**：所有支付装配集中在一处，读者一眼看到 hub/校验/健康检查。
2. **启动校验**：可以在 host 启动时统一校验所有 provider，失败早、错误集中。
3. **避免隐式行为**：每个 `add_*` 自动注册 hub 被视为“魔法”，不利于排障。
4. **与部分框架风格一致**：有些库确实要求显式 `AddXxx()`。

### 3.2 反方（结论采用）

1. **仓库已有反例**：`add_redis` 一次调用同时注册 cache、publisher、consumer、health；
   `add_nats`/`add_rabbitmq`/`add_kafka` 同样。支付没有理由多一步。
2. **运行时配置使启动校验不再完整**：DB 配置在启动时可能不可用；`add_payments()` 的
   “统一启动校验”承诺本身会失效。readiness 才是正确落点。
3. **隐式装配可以做到确定**：`add_*` 只做幂等的 `try_add_singleton`，注册顺序无关；
   出错路径仍是明确的 DI/健康错误，不是随机魔法。
4. **用户侧认知负担**：两个动词、一个额外失败点（忘记调用）；删除后没有信息丢失。
5. **未来运行时工厂**：`add_payments()` 还要承担“默认工厂注册顺序”的说明成本，越早删越好。

### 3.3 决定

- 删除 `add_payments()`、`PaymentStartupCheck` 和 `add_payments` 相关文档。
- 每个 `add_*` 内部调用 `Payments::ensure_registered(services)`（`try_add_singleton`）。
- 启动校验改为：provider 构造/健康检查校验静态配置；运行时配置由 readiness 的
  `IPaymentServiceFactory::probe()` 负责。静态配置错误 → readiness 503，不再阻塞进程启动。
- 保留 `Host::try_build()` 对 DI/其它框架配置的 fail-fast；支付配置不再参与。

### 3.4 其它被辩论的取舍

| 论点 | 反方 | 结论 |
|---|---|---|
| 静态/动态各一套 trait | 用户需要学两套，缓存/错误重复 | 统一为 provider factory + service factory 两层，普通用户只感知一种 |
| 工厂返回配置，hub 构造 | 业务无法包装/自定义 provider | 工厂返回 `Arc<dyn IPaymentProvider>`；builder 注册表避免重写协议 |
| 不做缓存，工厂自己负责 | DB 每请求一次，p99/可用性不可控 | hub 统一 TTL 缓存 + 单飞 + 失效 |
| 保留启动强校验 | 运行时配置不可知 | readiness + health；静态由 provider 校验 |
| tenant 拼进 provider_id | 字符串键易碰撞、无法校验 | 显式 `PaymentScope` |
| 工厂顺带管回调存储 | 事务/幂等/outbox 边界混乱 | 存储归 inbox/业务，工厂只给配置与 provider |

---

## 4. 目标架构

### 4.1 概念与职责

```text
业务 Handler
  │  payments.create(scope, request) / verify_webhook(&req)
  ▼
Payments（门面，singleton）
  │  作用域缓存（TTL/单飞/失效/上限）、错误、指标、readiness
  ▼
IPaymentServiceFactory（应用级运行时源；可覆盖）
  │  默认：DefaultPaymentServiceFactory（路由到 provider factories）
  ▼
IPaymentProviderFactory（适配器级，每个 provider 一个）
  │  add_stripe()：从 appsettings/monitor 构造
  │  add_stripe_with(options)：返回预构造实例
  ▼
Arc<dyn IPaymentProvider>
```

- **`Payments`**：用户唯一注入的门面；不暴露工厂存在。
- **`IPaymentServiceFactory`**：应用实现它就能接管“所有 provider 的配置来源”；
  默认实现只是把请求路由给各 provider 的 factory。
- **`IPaymentProviderFactory`**：适配器 crate 实现；`add_stripe()`/`add_stripe_with()`
  注册它。普通用户不实现。
- **`IPaymentProviderBuilder`**：适配器暴露的“配置 JSON → provider”构造器，供自定义
  `IPaymentServiceFactory` 复用官方协议实现；普通用户不接触。

### 4.2 关键类型

```rust
/// 解析作用域。tenant 只来自可信身份；account 来自业务/回调路径。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PaymentScope {
    provider: ProviderId,
    tenant: Option<TenantId>,
    account: Option<AccountId>,
}

impl PaymentScope {
    pub fn new(provider: ProviderId) -> Self;
    pub fn tenant(self, tenant: impl Into<TenantId>) -> Self;
    pub fn account(self, account: impl Into<AccountId>) -> Self;
    pub fn provider(&self) -> &ProviderId;
    pub fn tenant_ref(&self) -> Option<&TenantId>;
    pub fn account_ref(&self) -> Option<&AccountId>;
}

impl From<ProviderId> for PaymentScope { /* 无租户/默认账户 */ }

/// 应用级运行时源。默认实现内部路由；业务可实现为 DB/KMS。
#[async_trait]
pub trait IPaymentServiceFactory: Send + Sync {
    async fn resolve(
        &self,
        scope: &PaymentScope,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError>;

    /// readiness 探针（DB/KMS 可达性）；默认 Ok。
    async fn probe(&self) -> Result<(), PaymentError> {
        Ok(())
    }

    /// 可选：通知工厂自身缓存失效；默认 no-op。
    fn invalidate(&self, _scope: &PaymentScope) {}
}

/// 适配器级 provider 源。`add_stripe()` 与 `add_stripe_with(options)` 都注册它。
#[async_trait]
pub trait IPaymentProviderFactory: Send + Sync {
    fn provider(&self) -> ProviderId;
    async fn resolve(
        &self,
        scope: &PaymentScope,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError>;
}

/// 供自定义工厂复用官方协议实现：配置 JSON → provider。
pub trait IPaymentProviderBuilder: Send + Sync {
    fn provider(&self) -> ProviderId;
    fn build(
        &self,
        config: &PaymentProviderConfig,
    ) -> Result<Arc<dyn IPaymentProvider>, PaymentError>;
}

pub struct PaymentProviderConfig {
    pub provider: ProviderId,
    pub account: Option<AccountId>,
    pub live: Option<bool>,
    /// 与 `XXOptions` 的 serde 结构一致；密钥由工厂解析后注入 JSON，再交给 builder。
    pub settings: serde_json::Value,
    pub webhook_url: Option<String>,
    pub version: Option<String>,
}
```

### 4.3 门面 API（用户侧）

```rust
#[derive(Inject)]
struct CheckoutHandler {
    #[inject] payments: Arc<Payments>,
}

let scope = PaymentScope::new(ProviderId::Stripe)
    .tenant(req.user.tenant_id.clone().ok_or(...)?)
    .account("acct_eu");

let intent = self.payments
    .create(scope, CreatePayment::new(order_id, amount, method, flow))
    .await?;

// 单租户/默认账户可简写：
self.payments.create(ProviderId::Stripe, request).await?;

// 回调
let event = self.payments.verify_webhook(&req).await?;
Ok(self.payments.ack(&req).await?)
```

方法面（全部 `scope: impl Into<PaymentScope>`）：

```rust
impl Payments {
    pub async fn create(&self, scope: impl Into<PaymentScope>, req: CreatePayment)
        -> Result<PaymentIntent, PaymentError>;
    pub async fn get(&self, scope: impl Into<PaymentScope>, order_id: &str)
        -> Result<PaymentIntent, PaymentError>;
    pub async fn capture(&self, scope: impl Into<PaymentScope>, order_id: &str, amount: Option<Money>)
        -> Result<PaymentIntent, PaymentError>;
    pub async fn cancel(&self, scope: impl Into<PaymentScope>, order_id: &str)
        -> Result<PaymentIntent, PaymentError>;
    pub async fn refund(&self, scope: impl Into<PaymentScope>, cmd: RefundCommand)
        -> Result<Refund, PaymentError>;
    pub async fn get_refund(&self, scope: impl Into<PaymentScope>, refund_id: &str)
        -> Result<Refund, PaymentError>;
    pub async fn verify_webhook(&self, req: &PaymentWebhook)
        -> Result<PaymentEvent, PaymentError>;
    pub async fn ack(&self, request: &PaymentWebhook) -> Result<Response, PaymentError>;
    pub async fn payment_options(&self, tenant: Option<&TenantId>, currency: Option<&Currency>)
        -> Result<Vec<PaymentOption>, PaymentError>;
    pub async fn payment_options_for(&self, user: &IdentityUser, currency: Option<&Money>)
        -> Result<Vec<PaymentOption>, PaymentError>;
    pub async fn resolve(&self, scope: impl Into<PaymentScope>)
        -> Result<Arc<dyn IPaymentProvider>, PaymentError>;
}
```

`PaymentOption` 是前端结算页的公共目录：`provider/account/method/flows/client_config`，
不含任何 secret。默认工厂遍历已注册 provider；自定义工厂覆写
`IPaymentServiceFactory::payment_options` 返回当前租户启用的配置。框架不隐式注册路由，
应用用普通 Handler 暴露（手册 §3.1 有完整示例）。

`CreatePayment`/`RefundCommand` **移除 `account` 字段**；账户只来自 `PaymentScope`。
`PaymentIntent.account`/`Refund.account` 保留为结果。

### 4.4 注册 API（唯一入口）

```rust
// 运行时配置（默认从 appsettings 的 "Stripe" 节读取，可被自定义工厂覆盖）
Host::builder()
    .add_stripe()
    .add_alipay()
    .build();

// 静态配置（显式 options；测试/脚本/单租户）
Host::builder()
    .add_stripe_with(StripeOptions::new(secret_key, webhook_secret))
    .build();

// 应用自定义工厂（原生 DI 单例，不依赖适配器 crate）
Host::builder()
    .add_stripe()
    .register(|services| {
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

- 每个 `add_*` 只注册一个 provider factory；首次注册时自动创建 `Payments`（幂等）。
- `add_*_with(options)` 与 `add_*()` 的差别仅在 provider factory 的实现：前者返回预构造
  实例，后者从 `IOptionsMonitor<XXOptions>`/运行期配置构造；对 `Payments` 完全透明。
- `add_*_with_client`、`add_*_from_config`、每适配器 `ServiceCollectionExt` 删除；
  测试需要自定义 client 时直接 `StripeProvider::with_client(...)` + core
  `add_payment_provider`。
- **`IPaymentServiceFactory` 没有专用注册 API**：框架用 `try_add_singleton` 注册默认工厂，
  应用用 normal DI `singleton`/`instance` 注册实现；dix 的 MEDI 语义保证后注册覆盖先注册，
  因此 `add_*` 之前或之后注册都可。`Payments` 用 `resolve_arc::<dyn IPaymentServiceFactory>()`
  解析最终实现，不需要 `get_all` 手工挑选。
- 缓存策略也是原生 DI：注册 `PaymentRuntimeOptions` 覆盖默认 TTL/容量；未注册用默认值。

### 4.5 回调 API（唯一入口）

```rust
// 无账户段
payment_webhook!(StripeWebhook, "/webhooks/payments/{provider}");

// 带账户段（同一个请求类型，account 自动从路径读取）
payment_webhook!(AccountWebhook, "/webhooks/payments/{provider}/{account}");
```

- 只有一个 `PaymentWebhook`，绑定 provider + 可选 account + 原始 body/头。
- 只有一个 `Payments::verify_webhook`；内部按 scope（provider, account）解析 provider。
- 删除 `PaymentAccountWebhook`、`verify_account_webhook`、三参数宏。
- `payment_webhook!` 生成应用本地类型（孤儿规则）；路由仍由应用声明。

### 4.6 默认工厂与自定义工厂

```rust
/// 默认实现：把 scope.provider 路由到已注册的 IPaymentProviderFactory。
pub struct DefaultPaymentServiceFactory { /* provider -> factory */ }

#[async_trait]
impl IPaymentServiceFactory for DefaultPaymentServiceFactory {
    async fn resolve(&self, scope: &PaymentScope) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        self.providers.get(&scope.provider)
            .ok_or_else(|| PaymentError::not_found(scope.provider.clone(), "provider not registered"))?
            .resolve(scope)
            .await
    }
    async fn probe(&self) -> Result<(), PaymentError> {
        // 检查每个已注册 provider factory 的静态配置/缓存密钥可用性
        Ok(())
    }
}
```

业务自定义工厂从 DB 读取配置后构建 provider：

```rust
#[async_trait]
impl IPaymentServiceFactory for DbPaymentServiceFactory {
    async fn resolve(&self, scope: &PaymentScope) -> Result<Arc<dyn IPaymentProvider>, PaymentError> {
        let scope = self.scopes.create_scope();
        let ctx: DbContext = scope.get_owned()?;
        let row = load_enabled(&ctx, scope).await?;
        let config = row.to_config()?;
        self.builders
            .get(scope.provider)?
            .build(&config) // 官方协议实现，不需要依赖适配器 crate
    }
    async fn probe(&self) -> Result<(), PaymentError> {
        // 轻量 DB/KMS 探针；readiness 使用
    }
}
```

依赖性质：`DbPaymentServiceFactory` 只依赖 `webx_pay`（`IPaymentServiceFactory`/
`IPaymentProviderBuilder` 注册表）和它自己的 DB/KMS 栈，**不依赖**
`rust-webx-pay-stripe` 等适配器 crate；适配器由宿主 crate 链接并注册 builder。

### 4.7 解析、缓存与失效

```text
resolve(scope):
  1. 静态 provider 表命中？返回（add_*_with 的预构造实例）
  2. 缓存命中且未过期？返回 Arc
  3. 单飞（每 scope 一个异步锁）：
        custom factory ?? default factory -> provider
        provider.validate()
        写入缓存（TTL/LRU）
  4. 释放单飞，返回 Arc
```

| 项 | 默认 | 说明 |
|---|---|---|
| TTL | 60s | 配置变更最大延迟；可配置 |
| 容量 | 1024 | 防租户/账户膨胀；LRU |
| 单飞 | 每 scope | 防击穿；DB 故障不放大 QPS |
| 负缓存 | 5s（可选） | 未知 provider/账户高频探测；默认关闭 |
| 过期错误 | fail-closed | 不用过期凭证扣款；显式 `serve_stale` 可选 |
| 失效 | `Payments::invalidate(scope)` / `invalidate_provider` / `clear` | 管理端更新配置后调用；跨实例用 TTL + 可选事件 |

- 失效只删缓存；已持有的 `Arc` 完成在途请求，新请求用新实例。
- 跨实例广播可选：`IEventPublisher` 发布 `payments.config.invalidated`，各实例的 Handler
  调 `invalidate`；TTL 保底。
- 自定义工厂若内部也缓存，必须实现 `invalidate`；默认工厂不缓存。

### 4.8 健康与启动

- **启动**：`add_*` 只做注册；静态 options 在 provider 构造时校验，运行时配置在首次
  resolve 校验。支付配置错误不再阻塞进程启动。
- **readiness**：适配器 `add_*` 注册 `payments.<provider>` 健康检查（静态 validate /
  运行时 monitor 当前值 validate）；自定义工厂由应用注册 `IHealthCheck` 调 `probe()`。
  也可由 `Payments` 提供一个 `health()` 辅助方法（不自动注册，避免重复）。
- **失败语义**：

| 场景 | HTTP | 说明 |
|---|---|---|
| 未知 provider/account | 404 | 永久错误 |
| 工厂/DB 暂时不可用 | 503 + `Retry-After` | 新增 `PaymentError::Unavailable` |
| 配置字段/密钥非法 | 500 | 运维错误，不回显密钥 |
| 验签失败 | 400 | 供应商决定重投 |
| 过期缓存 + 源不可用 | 503 | 默认不 stale |

### 4.9 安全

- tenant 只来自 `IdentityUser`；绝不从 body/query 进入 `PaymentScope`。
- 回调 account 是公开路径：统一 404/400、限流；验签失败不区分“账户不存在/密钥错误”。
- DB 只存 `secret_refs`；`ISecretStore`/KMS 解析；`PaymentAccount`/日志/Debug 不暴露密钥。
- 自定义工厂必须校验租户/账户启用状态与归属；框架只保证缓存键隔离，不能替代业务授权。
- 缓存键包含 tenant/account；跨租户串用是 P0 缺陷，测试必须覆盖。

---

## 5. 命名决策

| 现有 | 目标 | 理由 |
|---|---|---|
| `add_payments()` | 删除 | 冗余装配仪式 |
| `add_stripe(options)` | `add_stripe_with(options)` | 静态配置显式；零参 `add_stripe()` 留给运行时 |
| `add_stripe_from_config()` | `add_stripe()` | 运行时默认就是配置节；无需单独名字 |
| `add_stripe_with_client()` | 删除（用 provider 构造器 + core 注册） | 非应用主路径 |
| 每适配器 `ServiceCollectionExt` | 删除（用 core 注册/测试） | 一条注册路径 |
| `PaymentsHostBuilderExt` / `PaymentsServiceCollectionExt` | 删除 host 扩展；保留内部 `PaymentsServiceCollectionExt` 给适配器注册 | 自定义工厂用原生 DI `singleton`/`instance`，不另造入口 |
| `PaymentHub` | `Payments` | 用户注入后 `payments.create(...)` 直接 |
| `PaymentWebhook` + `PaymentAccountWebhook` | `PaymentWebhook` | account 可选，一个类型 |
| `verify_webhook` + `verify_account_webhook` | `verify_webhook` | 一个方法 |
| `payment_webhook_route!` 2/3 参数 | `payment_webhook!` 2 参数 | account 自动从路径读取 |
| `CreatePayment.account` / `RefundCommand.account` / 方法参数 | `PaymentScope.account` | 账户只一个入口 |
| `add_*` 返回 `&ProviderId` 参数 | `scope: impl Into<PaymentScope>` | 支持 `ProviderId` 简写与 scope |
| 无 503 错误 | `PaymentError::Unavailable` | 配置源短暂不可用可重试 |

命名原则：用户侧只出现 **Payments / PaymentScope / IPaymentServiceFactory / add_* /
add_*_with / payment_webhook!**；`IPaymentProviderFactory`/`IPaymentProviderBuilder` 是扩展点，
文档标注“高级/自定义工厂”。

---

## 6. 依赖方向

```text
应用 Host
  ├── rust-webx（umbrella，可选）
  ├── rust-webx-pay（端口/门面/注册表）──► rust-webx-core + rust-webx-client + rust-webx-host
  ├── rust-webx-pay-stripe / -alipay / ...（适配器）──► rust-webx-pay + rust-webx-client
  ├── 回调 inbox 参考适配器（examples/adapters/payment/ef，可选）──► rust-webx-pay + rust-ef
  └── 业务 DbPaymentServiceFactory ──► rust-webx-pay（不依赖适配器 crate）
```

- 适配器不依赖 `rust-webx` umbrella；不引入第三方支付 SDK；唯一 HTTP 栈是
  `rust-webx-client`。
- `rust-webx-pay` 依赖 `rust-webx-host` 仅用于 `HostBuilder` 扩展/健康检查；与
  `rust-webx-redis` 先例一致。若未来需要非 Web 端口消费者，可拆 `pay/host`，当前不增加
  crate 数量。
- 业务工厂只依赖端口 crate，适配器由宿主链接并注册 builder；这是运行时工厂最重要的
  依赖解耦点。
- 回调 inbox 参考适配器不反向依赖任何支付适配器（它只依赖 `rust-webx-pay` 的类型与 `rust-ef`）。

---

## 7. 重构落地记录（R1–R3 已完成）

### R1 核心 API 收敛（✅）

- 新增 `PaymentScope`/`TenantId`；`CreatePayment`/`RefundCommand` 移除 `account`；
  `Payments` 门面替换 `PaymentHub`（无别名，直接删除旧类型）。
- 删除 `add_payments`、`PaymentStartupCheck`、`add_payment_provider_lazy` 的双实例注册。
- 新增 `IPaymentServiceFactory`/`IPaymentProviderFactory`/`IPaymentProviderBuilder`/
  `PaymentProviderBuilders`/`DefaultPaymentServiceFactory`。
- `Payments` 异步 `resolve` + TTL 缓存 + 单飞 + `invalidate`；新增
  `PaymentError::Unavailable`（503）。
- 合并 webhook 类型/方法/宏；更新 `webx_pay::testing`。

### R2 适配器收敛（✅）

- 六个适配器：`add_*()`（运行时）＋ `add_*_with(options)`（静态）；删除
  `_from_config`、`_with_client`、ServiceCollection ext。
- 注册 `IPaymentProviderFactory`（options monitor 或 static）＋ `IPaymentProviderBuilder`；
  provider 构造复用现有 options/serde。
- 静态路径保留 `payments.<provider>` 健康检查；运行时配置在解析/首用时校验。
- 文档示例改为无 `add_payments`。

### R3 测试与文档（✅）

- 手册、设计、CHANGELOG、lib.rs/adapters 文档同步；`add_payments` 只作为历史说明出现。
- 新增：scope 租户隔离、缓存 TTL/单飞/失效、工厂错误不进缓存、静态/运行时同路径、
  provider factory/builder 测试、回调 account 可选、自动运行时装配 e2e。
- 版本：2.7 定稿 API 改名。

### 验收

| 验收项 | 结果 |
|---|---|
| 无 `add_payments` | `.add_stripe().build()` 即可用；e2e 覆盖 |
| 静态/动态统一 | 同一 `Payments::create`、同一缓存、同一错误路径 |
| 运行时生效 | TTL / `invalidate` 后新请求重建；工厂失败不入缓存 |
| 租户隔离 | 单测覆盖 tenant/account 缓存键隔离 |
| 故障 | 工厂不可用 503；配置非法 500；未知 provider 404 |
| 门禁 | 支付测试、workspace check、fmt/clippy/docs/audit 全绿（见交付记录） |

---

## 8. 风险与已决事项

| 风险 | 影响 | 缓解 |
|---|---|---|
| 自动装配被误解为魔法 | 排障困难 | 文档明确“第一个 add_* 创建运行时”；错误信息指出注册入口 |
| 去掉启动强校验 | 坏配置上线 | readiness + provider 健康检查 + 可选预热服务 |
| 缓存/失效复杂 | 凭证轮换延迟 | 短 TTL + 显式失效 + 先加新后停旧；监控 cache age |
| 工厂异步/DB 依赖 | 热路径 p99 | 缓存/单飞；DB 只在 miss |
| 多实例不一致 | 部分节点旧配置 | TTL 保底；事件广播可选 |
| 默认工厂忽略 tenant 凭证 | 误以为租户自动隔离 | 文档明确：默认工厂 tenant 只参与缓存键；按租户切换凭证必须自定义工厂 |
| API 改名 | 2.7 发布前定稿 | 支付 crate 2.7 首发；同批文档/CHANGELOG |
| host 依赖 | 编译体积 | 接受（redis 先例）；需要时拆 `pay/host` |

已拍板：

1. 门面命名 `Payments`（不保留 `PaymentHub`）。
2. 接受“静态配置错误不再阻塞启动，改由 readiness/首次使用 503”。
3. 缓存 TTL 默认 60s、容量 1024（后续可按部署增加 options）。
4. 自定义工厂允许返回包装 provider；inbox 仍独立。
5. 跨实例失效首版不内置 `IEventPublisher`，TTL 保底；应用可自行广播并调用 `invalidate`。

---

## 9. 参考

- 当前实现与差异：[支付域架构级封装方案](payments.md)、[支付手册](../rust-webx/13-extensibility/payments.md)。
- 仓库先例：`add_redis`（一次注册 cache/publisher/consumer/health）、`add_nats` 等。
- 框架能力：`IOptionsMonitor`、`ISecretStore`/`SecretRef`、`IHealthCheck`、
  `Arc<dyn ScopeFactory>`、`IEventPublisher`。
- Rust 重载限制：同名方法跨 trait 会 `E0034`（已用 rustc 验证）。
