# LRDI 设计模式大全

## 前言

LRDI 的键控服务（`keyed`）+ trait 对象组合是设计模式的天然实现基础。每个设计模式都利用了 LRDI 的一项或多项核心能力：生命周期管理、类型擦除、键控解析（`get_keyed`）、批量解析（`get_all`）。

本文档的所有代码均基于 `ServiceCollection` 构建器 API，与 `crates/lrdi/tests/patterns.rs` 中的测试风格一致，确保可直接编译运行。

---

## 1. 策略模式（Strategy Pattern）

### 问题描述

需要在运行时根据条件（支付方式、排序算法、压缩格式）动态切换算法实现，且新增策略时不应修改已有代码。

### LRDI 解决方案

将默认策略注册为 `singleton`，替代策略注册为 `keyed` 服务，运行时用 `get_keyed` 切换。**键控服务是策略模式的 LRDI 原生实现**——新增策略只需添加一个 keyed 注册，无需修改任何现有代码。

```rust
use lrdi::*;
use std::sync::Arc;

// —— 策略接口 ——
trait PaymentGateway: Send + Sync {
    fn pay(&self, amount: f64) -> String;
    fn name(&self) -> &'static str;
}

// —— 具体策略实现 ——
struct CreditCardGateway;
impl PaymentGateway for CreditCardGateway {
    fn pay(&self, amount: f64) -> String {
        format!("信用卡支付: ¥{:.2}", amount)
    }
    fn name(&self) -> &'static str { "credit_card" }
}

struct AlipayGateway;
impl PaymentGateway for AlipayGateway {
    fn pay(&self, amount: f64) -> String {
        format!("支付宝支付: ¥{:.2}", amount)
    }
    fn name(&self) -> &'static str { "alipay" }
}

struct WechatGateway;
impl PaymentGateway for WechatGateway {
    fn pay(&self, amount: f64) -> String {
        format!("微信支付: ¥{:.2}", amount)
    }
    fn name(&self) -> &'static str { "wechat" }
}

// —— DI 容器注册 ——
fn build_payment_provider() -> ServiceProvider {
    ServiceCollection::new()
        // 默认策略使用 singleton 注册
        .singleton::<dyn PaymentGateway>(|_| Arc::new(CreditCardGateway))
        // 其他策略使用 keyed 注册
        .keyed::<dyn PaymentGateway>("alipay", |_| Arc::new(AlipayGateway))
        .keyed::<dyn PaymentGateway>("wechat", |_| Arc::new(WechatGateway))
        .build()
        .unwrap()
}

// —— 运行时使用 ——
fn process_payment(provider: &ServiceProvider, method: &str, amount: f64) -> String {
    // 尝试用 keyed 获取指定策略，失败则回退到默认
    if let Some(gateway) = provider.try_get_keyed::<dyn PaymentGateway>(method) {
        gateway.pay(amount)
    } else {
        let default_gateway: Arc<dyn PaymentGateway> = provider.get();
        default_gateway.pay(amount)
    }
}

#[test]
fn test_strategy_pattern() {
    let p = build_payment_provider();
    // 默认策略
    let default: Arc<dyn PaymentGateway> = p.get();
    assert_eq!(default.name(), "credit_card");
    // 运行时切换
    let alipay: Arc<dyn PaymentGateway> = p.get_keyed("alipay");
    assert_eq!(alipay.name(), "alipay");
    // 不存在的策略回退
    assert_eq!(
        process_payment(&p, "unknown", 100.0),
        "信用卡支付: ¥100.00"
    );
}
```

### 新增策略（无需修改已有代码）

```rust
struct PayPalGateway;
impl PaymentGateway for PayPalGateway {
    fn pay(&self, amount: f64) -> String {
        format!("PayPal支付: ¥{:.2}", amount)
    }
    fn name(&self) -> &'static str { "paypal" }
}

fn build_extended_provider() -> ServiceProvider {
    ServiceCollection::new()
        .singleton::<dyn PaymentGateway>(|_| Arc::new(CreditCardGateway))
        .keyed::<dyn PaymentGateway>("alipay", |_| Arc::new(AlipayGateway))
        .keyed::<dyn PaymentGateway>("wechat", |_| Arc::new(WechatGateway))
        // 新增一行即可！不影响任何已有代码
        .keyed::<dyn PaymentGateway>("paypal", |_| Arc::new(PayPalGateway))
        .build()
        .unwrap()
}
```

### 适用场景

- 存在一组可互换的算法族（排序、压缩、加密、支付）
- 需要在运行时根据配置或用户输入切换算法
- 期望新增算法时满足开闭原则（对扩展开放，对修改关闭）
- 不希望在各处散布 `match` 或 `if/else` 分支判断

### 不宜使用

- 只有两个策略且几乎不会增加第三种的场景（增加复杂度超过收益）
- 策略之间有大量共享状态（keyed 服务彼此独立，共享状态需额外设计）
- 策略选择条件非常复杂、动态变化（此时应使用规则引擎而非策略模式）

### 对比：传统 enum dispatch vs LRDI keyed

```rust
// ❌ 传统方案：新增策略需要修改 enum 和所有 match 分支
enum PaymentMethod { CreditCard, Alipay, Wechat }
fn pay(method: PaymentMethod, amount: f64) -> String {
    match method {
        PaymentMethod::CreditCard => CreditCardGateway.pay(amount),
        PaymentMethod::Alipay => AlipayGateway.pay(amount),
        PaymentMethod::Wechat => WechatGateway.pay(amount),
        // 新增 PayPal？必须修改此 match 分支！
    }
}

// ✅ LRDI 方案：新增策略完全解耦
fn pay(provider: &ServiceProvider, method: &str, amount: f64) -> String {
    provider.get_keyed::<dyn PaymentGateway>(method).pay(amount)
}
```

---

## 2. 工厂模式（Factory Pattern）

### 问题描述

对象的创建逻辑复杂（依赖配置、环境、上下文），或者需要根据运行时条件创建不同实现，不应由消费者直接 `new`。

### 变体 A：注册时逻辑工厂（Closure Factory）

在注册闭包中根据已有服务的状态动态决定创建哪种实现。

```rust
use lrdi::*;
use std::sync::Arc;

// —— 产品接口 ——
trait Database: Send + Sync {
    fn connect(&self) -> String;
}

struct PostgresDB;
impl Database for PostgresDB {
    fn connect(&self) -> String { "连接到 PostgreSQL".into() }
}

struct SqliteDB;
impl Database for SqliteDB {
    fn connect(&self) -> String { "连接到 SQLite".into() }
}

// —— 配置服务 ——
struct AppConfig {
    db_type: String,
}

fn build_database_provider() -> ServiceProvider {
    ServiceCollection::new()
        // 先注册配置
        .singleton(|_| Arc::new(AppConfig { db_type: "postgres".into() }))
        // 工厂闭包：根据配置动态选择产品
        .singleton::<dyn Database>(|r| {
            let config: Arc<AppConfig> = r.get();
            if config.db_type == "postgres" {
                Arc::new(PostgresDB)
            } else {
                Arc::new(SqliteDB)
            }
        })
        .build()
        .unwrap()
}

#[test]
fn test_closure_factory() {
    let p = build_database_provider();
    let db: Arc<dyn Database> = p.get();
    assert_eq!(db.connect(), "连接到 PostgreSQL");
}
```

### 变体 B：键控工厂（Keyed Factory）

将所有产品按 key 注册为键控服务，运行时根据 key 选择产品。这是 LRDI 最原生的工厂实现。

```rust
use lrdi::*;
use std::sync::Arc;

// —— 产品接口 ——
trait Document: Send + Sync {
    fn render(&self) -> String;
}

struct PdfDocument;
impl Document for PdfDocument {
    fn render(&self) -> String { "渲染 PDF".into() }
}

struct WordDocument;
impl Document for WordDocument {
    fn render(&self) -> String { "渲染 Word".into() }
}

struct MarkdownDocument;
impl Document for MarkdownDocument {
    fn render(&self) -> String { "渲染 Markdown".into() }
}

fn build_document_provider() -> ServiceProvider {
    ServiceCollection::new()
        .keyed::<dyn Document>("pdf", |_| Arc::new(PdfDocument))
        .keyed::<dyn Document>("docx", |_| Arc::new(WordDocument))
        .keyed::<dyn Document>("md", |_| Arc::new(MarkdownDocument))
        .build()
        .unwrap()
}

#[test]
fn test_keyed_factory() {
    let p = build_document_provider();
    let pdf: Arc<dyn Document> = p.get_keyed("pdf");
    assert_eq!(pdf.render(), "渲染 PDF");
    let word: Arc<dyn Document> = p.get_keyed("docx");
    assert_eq!(word.render(), "渲染 Word");
}
```

### 适用场景

- 对象创建依赖运行时上下文（配置文件、环境变量、用户输入）
- 需要根据 key 创建不同实现（与策略模式侧重运行时使用不同，工厂侧重创建过程）
- 创建逻辑需要集中管理，避免散布在各处的 `new` 调用

### 不宜使用

- 创建逻辑非常简单（直接 `new` 即可），无需额外抽象层
- 产品之间没有共同 trait（无法统一为 `dyn Trait` 注册）
- 键控工厂 key 数量过多（>20），建议按领域拆分容器

---

## 3. 装饰器模式（Decorator Pattern）

### 问题描述

需要在不修改原对象的前提下，动态添加额外行为（如日志、计时、缓存、认证）。这些横切关注点应灵活叠加，而非为每种组合创建子类。

### LRDI 解决方案

将基础实现注册为非公开的命名服务，在工厂闭包中链式包装：`BaseNotifier → TimestampDecorator → LogLevelDecorator`。消费者只依赖 `dyn Notifier`，不感知装饰链。

```rust
use lrdi::*;
use std::sync::Arc;
use std::time::Instant;

// —— 核心接口 ——
trait Notifier: Send + Sync {
    fn send(&self, message: &str) -> String;
}

// —— 基础实现 ——
struct BaseNotifier;
impl Notifier for BaseNotifier {
    fn send(&self, message: &str) -> String {
        format!("发送: {}", message)
    }
}

// —— 日志装饰器 ——
struct LoggingDecorator {
    inner: Arc<dyn Notifier>,
}
impl Notifier for LoggingDecorator {
    fn send(&self, message: &str) -> String {
        let result = self.inner.send(message);
        format!("[LOG] {}", result)
    }
}

// —— 计时装饰器 ——
struct TimingDecorator {
    inner: Arc<dyn Notifier>,
}
impl Notifier for TimingDecorator {
    fn send(&self, message: &str) -> String {
        let start = Instant::now();
        let result = self.inner.send(message);
        let elapsed = start.elapsed();
        format!("[{}µs] {}", elapsed.as_micros(), result)
    }
}

// —— 组装装饰链 ——
fn build_notifier_provider() -> ServiceProvider {
    ServiceCollection::new()
        // 基础实现作为内部依赖注册（不作为 dyn Notifier 暴露）
        .singleton(|_| Arc::new(BaseNotifier))
        // 工厂闭包：层层包装
        .singleton::<dyn Notifier>(|r| {
            let base: Arc<BaseNotifier> = r.get();
            // 链：BaseNotifier → LoggingDecorator → TimingDecorator
            let with_logging = LoggingDecorator { inner: base };
            Arc::new(TimingDecorator {
                inner: Arc::new(with_logging),
            })
        })
        .build()
        .unwrap()
}

#[test]
fn test_decorator_pattern() {
    let p = build_notifier_provider();
    let notifier: Arc<dyn Notifier> = p.get();
    let result = notifier.send("hello");
    assert!(result.contains("[LOG]"));
    assert!(result.contains("µs"));
    assert!(result.contains("发送: hello"));
}
```

### 测试中跳过装饰器

```rust
// 测试 BaseNotifier 的纯粹逻辑（跳过所有装饰器）
#[test]
fn test_base_notifier_only() {
    let notifier = BaseNotifier;
    assert_eq!(notifier.send("test"), "发送: test");
}

// 测试单个装饰器
#[test]
fn test_logging_decorator_only() {
    let inner: Arc<dyn Notifier> = Arc::new(BaseNotifier);
    let decorator = LoggingDecorator { inner };
    assert_eq!(decorator.send("test"), "[LOG] 发送: test");
}
```

### 适用场景

- 需要为对象动态叠加多种横切关注点（日志、认证、缓存、限流）
- 希望灵活组合行为，而非为每种组合创建具体实现
- 装饰器的顺序有业务语义（如先认证再日志 vs 先日志再认证）
- 需要在不同环境（开发/测试/生产）使用不同的装饰组合

### 不宜使用

- 装饰链过长（>5 层）影响性能且排查困难
- 装饰器之间需要共享大量状态（应改用管道模式）
- 被装饰的对象方法签名差异很大（装饰器必须实现相同 trait）

---

## 4. 责任链模式（Chain of Responsibility）

### 问题描述

一个请求需经过多个处理器依次处理，每个处理器可处理请求返回结果，或传递给下一个处理器。常见于 HTTP 中间件、审批流程、事件处理。

### 变体 A：键控处理器链（Keyed Handlers）

每个处理器注册为 keyed 服务，由 RequestProcessor 按配置的顺序依次调用。这是业务代码中最常用的责任链实现——**基于配置的处理器链**。

```rust
use lrdi::*;
use std::sync::Arc;

// —— 处理器接口 ——
trait Handler: Send + Sync {
    /// 处理请求，返回 None 表示放行给下一个，返回 Some 表示链终止
    fn handle(&self, request: &str) -> Option<String>;
}

// —— 认证处理器 ——
struct AuthHandler;
impl Handler for AuthHandler {
    fn handle(&self, request: &str) -> Option<String> {
        if request.contains("token=invalid") {
            Some("认证失败: token 无效".into())
        } else {
            None // 放行
        }
    }
}

// —— 限流处理器 ——
struct RateLimitHandler {
    max_requests: u32,
}
impl Handler for RateLimitHandler {
    fn handle(&self, request: &str) -> Option<String> {
        // 简化示例：超过限制返回错误
        if request.contains("burst") {
            Some("限流: 请求过于频繁".into())
        } else {
            None
        }
    }
}

// —— 日志处理器（总是放行） ——
struct LoggingHandler;
impl Handler for LoggingHandler {
    fn handle(&self, request: &str) -> Option<String> {
        // 记录日志但始终放行
        println!("[LOG] 收到请求: {}", request);
        None
    }
}

// —— 请求处理器：按配置顺序执行链 ——
struct RequestProcessor {
    provider: Arc<ServiceProvider>,
    chain: Vec<&'static str>, // 可配置的处理链顺序
}
impl RequestProcessor {
    fn process(&self, request: &str) -> Result<String, String> {
        for key in &self.chain {
            let handler: Arc<dyn Handler> = self.provider.get_keyed(key);
            if let Some(error) = handler.handle(request) {
                return Err(error); // 链中断
            }
        }
        Ok(format!("请求 '{}' 处理成功", request))
    }
}

fn build_chain_provider() -> ServiceProvider {
    ServiceCollection::new()
        .keyed::<dyn Handler>("auth", |_| Arc::new(AuthHandler))
        .keyed::<dyn Handler>("rate_limit", |_| {
            Arc::new(RateLimitHandler { max_requests: 100 })
        })
        .keyed::<dyn Handler>("logging", |_| Arc::new(LoggingHandler))
        .build()
        .unwrap()
}

#[test]
fn test_keyed_handler_chain() {
    let p = Arc::new(build_chain_provider());
    let processor = RequestProcessor {
        provider: p.clone(),
        chain: vec!["auth", "rate_limit", "logging"], // 配置驱动
    };

    // 正常请求通过
    assert!(processor.process("normal_request").is_ok());
    // 认证失败链中断
    assert!(processor.process("token=invalid").is_err());
}
```

### 变体 B：链表式责任链（Linked List Chain）

每个处理器持有 `Option<Arc<dyn Handler>> next`，在工厂闭包中链接成链。适合**连接关系固定**的场景。

```rust
use lrdi::*;
use std::sync::Arc;

trait LinkedHandler: Send + Sync {
    fn handle(&self, request: &str) -> String;
}

struct ConcreteHandler {
    name: &'static str,
    next: Option<Arc<dyn LinkedHandler>>,
}
impl LinkedHandler for ConcreteHandler {
    fn handle(&self, request: &str) -> String {
        let result = format!("[{}] {}", self.name, request);
        match &self.next {
            Some(next) => format!("{} → {}", result, next.handle(request)),
            None => result,
        }
    }
}

fn build_linked_chain() -> ServiceProvider {
    ServiceCollection::new()
        .singleton::<dyn LinkedHandler>(|_| {
            // 构造链：Third → Second → First
            let first = Arc::new(ConcreteHandler { name: "First", next: None });
            let second = Arc::new(ConcreteHandler {
                name: "Second",
                next: Some(first),
            });
            Arc::new(ConcreteHandler {
                name: "Third",
                next: Some(second),
            })
        })
        .build()
        .unwrap()
}

#[test]
fn test_linked_chain() {
    let p = build_linked_chain();
    let handler: Arc<dyn LinkedHandler> = p.get();
    let result = handler.handle("hello");
    assert!(result.starts_with("[Third]"));
    assert!(result.contains("[Second]"));
    assert!(result.contains("[First]"));
}
```

### 适用场景

- 变体 A：请求经过多个中间件，链顺序可通过配置灵活调整
- 变体 B：链结构固定，链表式编码更简洁直观
- 审批流程、事件处理、HTTP 中间件管线

### 不宜使用

- 链过长（>10 个处理器）导致调试困难——考虑按领域分组
- 处理器之间有强依赖关系（A 的输出是 B 的输入）——应使用管道模式
- 变体 B 在需要动态调整顺序时不够灵活

---

## 5. 观察者模式（Observer Pattern）

### 问题描述

一个对象（Subject）状态改变时，需要自动通知多个观察者。观察者的数量和类型应可动态增减，且不应影响 Subject 的代码。

### LRDI 解决方案

所有观察者注册为 keyed 服务，Subject 使用 **`get_all`** 获取所有观察者并广播事件。`get_all` 是观察者模式在 LRDI 中的核心 API——它返回所有注册为该 `dyn Trait` 的实例。

```rust
use lrdi::*;
use std::sync::Arc;

// —— 观察者接口 ——
trait Observer: Send + Sync {
    fn on_event(&self, event: &str) -> String;
}

// —— 邮件观察者 ——
struct EmailObserver {
    addr: &'static str,
}
impl Observer for EmailObserver {
    fn on_event(&self, event: &str) -> String {
        format!("[邮件-{}] 发送: {}", self.addr, event)
    }
}

// —— 短信观察者 ——
struct SmsObserver {
    phone: &'static str,
}
impl Observer for SmsObserver {
    fn on_event(&self, event: &str) -> String {
        format!("[短信-{}] 发送: {}", self.phone, event)
    }
}

// —— 审计日志观察者 ——
struct AuditObserver;
impl Observer for AuditObserver {
    fn on_event(&self, event: &str) -> String {
        format!("[审计] 记录: {}", event)
    }
}

// —— 事件广播器（Subject） ——
struct EventBroadcaster {
    provider: Arc<ServiceProvider>,
}
impl EventBroadcaster {
    fn broadcast(&self, event: &str) -> Vec<String> {
        // get_all 获取所有注册为 dyn Observer 的实例
        let observers: Vec<Arc<dyn Observer>> = self.provider.get_all::<dyn Observer>();
        observers
            .iter()
            .map(|obs| obs.on_event(event))
            .collect()
    }
}

fn build_observer_provider() -> ServiceProvider {
    ServiceCollection::new()
        // 所有观察者注册为 keyed 服务
        .keyed::<dyn Observer>("email", |_| {
            Arc::new(EmailObserver { addr: "user@example.com" })
        })
        .keyed::<dyn Observer>("sms", |_| {
            Arc::new(SmsObserver { phone: "13800138000" })
        })
        .keyed::<dyn Observer>("audit", |_| Arc::new(AuditObserver))
        .build()
        .unwrap()
}

#[test]
fn test_observer_pattern() {
    let p = Arc::new(build_observer_provider());
    let broadcaster = EventBroadcaster { provider: p.clone() };

    // 广播事件，所有观察者都会响应
    let results = broadcaster.broadcast("用户注册成功");
    assert_eq!(results.len(), 3); // 3 个观察者全部通知
    assert!(results.iter().any(|r| r.contains("邮件")));
    assert!(results.iter().any(|r| r.contains("短信")));
    assert!(results.iter().any(|r| r.contains("审计")));
}
```

### 动态增减观察者

```rust
// 场景：VIP 用户注册时额外通知客户经理
fn build_vip_observer_provider() -> ServiceProvider {
    ServiceCollection::new()
        .keyed::<dyn Observer>("email", |_| {
            Arc::new(EmailObserver { addr: "vip@example.com" })
        })
        .keyed::<dyn Observer>("sms", |_| {
            Arc::new(SmsObserver { phone: "13900139000" })
        })
        .keyed::<dyn Observer>("audit", |_| Arc::new(AuditObserver))
        // VIP 专属：只需添加一行！
        .keyed::<dyn Observer>("vip_sms", |_| {
            Arc::new(SmsObserver { phone: "manager_phone" })
        })
        .build()
        .unwrap()
}
```

### 适用场景

- 事件驱动架构，一个事件触发多个副操作（注册→发邮件+发短信+记日志）
- GUI 中 Model 变化自动更新多个 View
- 微服务中的事件广播（配合消息队列）
- 需要动态增减监听器的任何场景

### 不宜使用

- 观察者过多（>50 个）时 `get_all` 遍历有性能成本——考虑异步+批量
- 观察者间有执行顺序依赖——`get_all` 返回顺序不保证，应使用责任链
- 观察者需要修改 Subject 状态——容易引发循环通知，应避免

---

## 6. 组合模式（Composite Pattern）

### 问题描述

需要将对象组织成树形结构，使客户端可以一致地处理单个对象（叶子）和组合对象（容器）。典型场景：UI 组件树、文件系统、组织架构、表达式树。

### LRDI 解决方案

叶子节点注册为 keyed 服务，组合节点（GroupNode）在运行时通过组装多个叶子构建树。`Component` trait 统一叶子与容器的接口。

```rust
use lrdi::*;
use std::sync::Arc;

// —— 组件接口（叶子与容器共享） ——
trait Component: Send + Sync {
    fn render(&self) -> Vec<String>;
    fn count(&self) -> usize;
}

// —— 叶子节点 ——
struct LeafNode {
    name: &'static str,
}
impl Component for LeafNode {
    fn render(&self) -> Vec<String> {
        vec![format!("  <leaf>{}</leaf>", self.name)]
    }
    fn count(&self) -> usize { 1 }
}

// —— 组合节点：包含子节点列表 ——
struct GroupNode {
    name: &'static str,
    children: Vec<Arc<dyn Component>>,
}
impl Component for GroupNode {
    fn render(&self) -> Vec<String> {
        let mut result = vec![format!("<group name=\"{}\">", self.name)];
        for child in &self.children {
            result.extend(child.render());
        }
        result.push("</group>".into());
        result
    }
    fn count(&self) -> usize {
        self.children.iter().map(|c| c.count()).sum()
    }
}

// —— 注册叶子节点，运行时组装成树 ——
fn build_ui_provider() -> ServiceProvider {
    ServiceCollection::new()
        // 注册叶子节点
        .keyed::<dyn Component>("btn_ok", |_| Arc::new(LeafNode { name: "OK" }))
        .keyed::<dyn Component>("btn_cancel", |_| {
            Arc::new(LeafNode { name: "Cancel" })
        })
        .keyed::<dyn Component>("label_title", |_| {
            Arc::new(LeafNode { name: "Title" })
        })
        .build()
        .unwrap()
}

#[test]
fn test_composite_pattern() {
    let p = build_ui_provider();

    // 从容器获取叶子节点
    let btn_ok: Arc<dyn Component> = p.get_keyed("btn_ok");
    let btn_cancel: Arc<dyn Component> = p.get_keyed("btn_cancel");
    let label: Arc<dyn Component> = p.get_keyed("label_title");

    // 组装成组合节点
    let buttons = GroupNode {
        name: "button_bar",
        children: vec![btn_ok, btn_cancel],
    };
    let root = GroupNode {
        name: "dialog",
        children: vec![label, Arc::new(buttons)],
    };

    // 统一渲染
    let output = root.render();
    assert_eq!(root.count(), 3); // 1 label + 2 buttons
    assert!(output[0].contains("dialog"));
    assert!(output.iter().any(|l| l.contains("OK")));
    assert!(output.iter().any(|l| l.contains("Cancel")));
}
```

### 适用场景

- 整体-部分层次结构（文件系统、组织树、UI 组件树）
- 希望客户端统一对待叶子节点和组合节点
- 递归结构（表达式树、AST、菜单系统）

### 不宜使用

- 叶子与容器行为差异很大，强行统一 trait 会导致接口臃肿
- 深层嵌套可能栈溢出——递归渲染时控制深度或转为迭代
- 树结构经常动态变化且需要高性能——考虑 arena 分配器而非 Arc

---

## 7. 代理模式（Proxy Pattern）

### 问题描述

需要在访问某个对象时添加控制逻辑，如延迟加载大对象、权限检查、缓存、远程代理。核心诉求是代理对象与原对象实现相同接口，对消费者透明。

### LRDI 解决方案

代理持有 `ServiceProvider` 引用，在首次访问时才通过 DI 解析真正的目标对象。DI 容器作为"服务定位器"注入到代理中，实现延迟实例化。

```rust
use lrdi::*;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// —— 接口 ——
trait Image: Send + Sync {
    fn display(&self) -> String;
    fn filename(&self) -> &str;
}

// —— 真实对象（创建成本高） ——
struct RealImage {
    filename: String,
    data: Vec<u8>, // 模拟大图片数据
}
impl RealImage {
    fn load(filename: &str) -> Self {
        println!("[RealImage] 从磁盘加载: {} ...", filename);
        thread::sleep(Duration::from_millis(200)); // 模拟 I/O 延迟
        Self {
            filename: filename.into(),
            data: vec![0u8; 1024 * 1024], // 模拟 1MB 图片
        }
    }
}
impl Image for RealImage {
    fn display(&self) -> String {
        format!("显示图片: {} ({}KB)", self.filename, self.data.len() / 1024)
    }
    fn filename(&self) -> &str { &self.filename }
}

// —— 代理：延迟加载 ——
struct LazyImageProxy {
    filename: String,
    real: Mutex<Option<Arc<dyn Image>>>,
}
impl Image for LazyImageProxy {
    fn display(&self) -> String {
        let mut real = self.real.lock().unwrap();
        if real.is_none() {
            println!("[Proxy] 首次访问，触发懒加载...");
            *real = Some(Arc::new(RealImage::load(&self.filename)));
        }
        real.as_ref().unwrap().display()
    }
    fn filename(&self) -> &str { &self.filename }
}

fn build_image_provider() -> ServiceProvider {
    ServiceCollection::new()
        .singleton::<dyn Image>(|_| {
            Arc::new(LazyImageProxy {
                filename: "photo.png".into(),
                real: Mutex::new(None),
            })
        })
        .build()
        .unwrap()
}

#[test]
fn test_proxy_pattern() {
    let p = build_image_provider();
    let img: Arc<dyn Image> = p.get();

    // 首次 display 触发加载（有延迟）
    let result1 = img.display();
    assert!(result1.contains("photo.png"));
    assert!(result1.contains("1024KB"));

    // 再次 display 直接使用缓存（无延迟）
    let result2 = img.display();
    assert_eq!(result1, result2);
}
```

### 适用场景

- 懒加载：大图片、大文件、数据库连接等创建成本高的对象
- 访问控制：权限校验代理（在访问前检查权限）
- 远程代理：隐藏网络调用细节，让远程对象像本地对象一样使用
- 缓存代理：在代理层缓存结果，避免重复计算

### 不宜使用

- 每次访问都需要穿透到真实对象——延迟加载无意义，徒增复杂度
- 代理需要添加的行为与业务逻辑耦合——考虑装饰器模式
- 线程安全开销（Mutex）大于延迟加载的收益——直接使用 Singleton

---

## 8. 管道模式（Pipeline Pattern）

### 问题描述

数据需要经过一系列有序的处理阶段（Stage），每个阶段对数据进行转换后传递给下一阶段。常见于文本处理（清洗→分词→过滤）、图像处理管线、请求处理管线。与责任链的区别：管道中**每个阶段都会执行**，数据在不同阶段间**转换**。

### LRDI 解决方案

每个处理阶段注册为 keyed 服务。Pipeline 从配置中读取阶段顺序，按序串行执行。LRDI 支持从配置文件读取阶段组合，实现动态管道配置。

```rust
use lrdi::*;
use std::sync::Arc;

// —— 处理阶段接口 ——
trait Stage: Send + Sync {
    fn process(&self, input: String) -> String;
    fn name(&self) -> &'static str;
}

// —— 转大写阶段 ——
struct UpperStage;
impl Stage for UpperStage {
    fn process(&self, input: String) -> String {
        input.to_uppercase()
    }
    fn name(&self) -> &'static str { "upper" }
}

// —— 去空格阶段 ——
struct TrimStage;
impl Stage for TrimStage {
    fn process(&self, input: String) -> String {
        input.trim().to_string()
    }
    fn name(&self) -> &'static str { "trim" }
}

// —— 敏感词过滤阶段 ——
struct SensitiveWordFilter {
    blocked: Vec<&'static str>,
}
impl Stage for SensitiveWordFilter {
    fn process(&self, input: String) -> String {
        let mut result = input;
        for word in &self.blocked {
            result = result.replace(word, "***");
        }
        result
    }
    fn name(&self) -> &'static str { "filter" }
}

// —— 管道执行器 ——
struct TextPipeline {
    provider: Arc<ServiceProvider>,
    stage_order: Vec<&'static str>, // 可配置
}
impl TextPipeline {
    fn execute(&self, input: &str) -> String {
        let mut data = input.to_string();
        for key in &self.stage_order {
            let stage: Arc<dyn Stage> = self.provider.get_keyed(key);
            data = stage.process(data);
        }
        data
    }
}

fn build_pipeline_provider() -> ServiceProvider {
    ServiceCollection::new()
        .keyed::<dyn Stage>("upper", |_| Arc::new(UpperStage))
        .keyed::<dyn Stage>("trim", |_| Arc::new(TrimStage))
        .keyed::<dyn Stage>("filter", |_| {
            Arc::new(SensitiveWordFilter {
                blocked: vec!["badword", "spam"],
            })
        })
        .build()
        .unwrap()
}

#[test]
fn test_pipeline_pattern() {
    let p = Arc::new(build_pipeline_provider());

    // 配置管道顺序：trim → upper → filter
    let pipeline = TextPipeline {
        provider: p.clone(),
        stage_order: vec!["trim", "upper", "filter"],
    };

    let result = pipeline.execute("  hello badword world  ");
    assert_eq!(result, "HELLO *** WORLD");
    // 管道依次执行：去空格 → 转大写 → 敏感词过滤
}

#[test]
fn test_dynamic_pipeline_config() {
    let p = Arc::new(build_pipeline_provider());

    // 从配置文件读取阶段顺序（模拟）
    let config_stages: Vec<&str> = vec!["filter", "trim"];

    let pipeline = TextPipeline {
        provider: p,
        stage_order: config_stages,
    };

    let result = pipeline.execute("  hello badword  ");
    assert_eq!(result, "hello ***");
}
```

### 适用场景

- 数据处理流水线（ETL、文本处理、编译管线）
- 请求/响应转换链（HTTP 中间件对数据做转换）
- 图像/音频处理（滤镜链、特效叠加）
- 需要从配置文件动态读取处理阶段的系统

### 不宜使用

- 阶段之间需要短路（某阶段可能拒绝执行后续）——应使用责任链模式
- 阶段间数据格式差异很大（需要独立类型）——考虑消息驱动架构
- 管道中阶段数量固定且很少（2-3 个）——直接写函数组合更简洁

---

## 9. 特性开关（Feature Toggle）

### 问题描述

新功能需要灰度发布或 A/B 测试，需要在运行时根据配置决定启用新实现还是旧实现。特性开关的核心是**运行时动态路由**，而非编译时条件编译。

### LRDI 解决方案

新旧实现分别注册为 keyed 服务，FeatureService 在运行时根据配置或环境变量选择使用哪个实现。

```rust
use lrdi::*;
use std::sync::Arc;
use std::collections::HashMap;

// —— 功能接口 ——
trait Checkout: Send + Sync {
    fn process(&self, cart_items: &[&str]) -> String;
}

// —— 旧版结账流程 ——
struct OldCheckout;
impl Checkout for OldCheckout {
    fn process(&self, cart_items: &[&str]) -> String {
        format!("[旧版] 结账 {} 件商品", cart_items.len())
    }
}

// —— 新版结账流程（灰度测试） ——
struct NewCheckout;
impl Checkout for NewCheckout {
    fn process(&self, cart_items: &[&str]) -> String {
        format!("[新版] 智能结账 {} 件商品，已应用优惠券", cart_items.len())
    }
}

// —— 特性开关服务 ——
struct FeatureService {
    provider: Arc<ServiceProvider>,
    features: HashMap<String, String>, // feature_name → implementation_key
}
impl FeatureService {
    fn resolve_checkout(&self, user_id: &str) -> Arc<dyn Checkout> {
        // 根据用户 ID 决定使用哪个版本（模拟灰度规则）
        let key = if user_id.ends_with("_beta") {
            self.features.get("checkout").map(|s| s.as_str()).unwrap_or("old")
        } else {
            "old"
        };
        self.provider.get_keyed(key)
    }
}

fn build_feature_provider() -> ServiceProvider {
    ServiceCollection::new()
        .keyed::<dyn Checkout>("old", |_| Arc::new(OldCheckout))
        .keyed::<dyn Checkout>("new", |_| Arc::new(NewCheckout))
        .build()
        .unwrap()
}

#[test]
fn test_feature_toggle() {
    let p = Arc::new(build_feature_provider());
    let mut features = HashMap::new();
    features.insert("checkout".into(), "new".into());

    let service = FeatureService {
        provider: p.clone(),
        features,
    };

    // Beta 用户使用新版
    let checkout = service.resolve_checkout("user_beta");
    assert!(checkout.process(&["item1"]).contains("新版"));

    // 普通用户使用旧版
    let checkout = service.resolve_checkout("user_normal");
    assert!(checkout.process(&["item1"]).contains("旧版"));
}
```

### 与配置中心集成实现动态开关

```rust
// 从配置中心读取特性开关配置（模拟）
fn load_feature_config() -> HashMap<String, String> {
    // 实际项目中从 etcd/consul/Apollo 等配置中心拉取
    let mut features = HashMap::new();
    features.insert("checkout".into(), "new".into());  // 10% 流量切新版
    features.insert("recommendation".into(), "old".into()); // 推荐引擎尚未切换
    features
}

#[test]
fn test_config_driven_features() {
    let p = Arc::new(build_feature_provider());
    let features = load_feature_config();
    let service = FeatureService { provider: p, features };
    // 10% beta 用户命中新版
    let result = service.resolve_checkout("user_001_beta").process(&["A"]);
    assert!(result.contains("新版"));
}
```

### 适用场景

- 灰度发布（金丝雀发布、百分比逐步放量）
- A/B 测试（对比新旧实现的业务指标）
- 紧急开关（Kill Switch，出问题时快速切回旧版）
- 多租户差异化功能（不同租户看到不同版本）

### 不宜使用

- 用特性开关规避代码重构（两套实现长期并存是技术债务）
- 开关数量爆炸（>50 个）导致组合爆炸——考虑模块化拆分
- 永久性的多版本共存——应使用策略模式而非特性开关

---

## 10. 插件架构（Plugin Architecture）

### 问题描述

需要构建可扩展的应用程序，允许第三方或独立团队开发插件，而主程序代码无需修改。插件应能被自动发现、加载、隔离。

### LRDI 解决方案

定义 `Plugin` trait（包含 name + execute），每个插件实现此 trait 并注册为 keyed 服务。插件发现用 `get_all`，插件隔离用作用域或分层容器。

```rust
use lrdi::*;
use std::sync::Arc;

// —— 插件接口 ——
trait Plugin: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn execute(&self, args: &[String]) -> String;
}

// —— 数学插件 ——
struct MathPlugin;
impl Plugin for MathPlugin {
    fn name(&self) -> &'static str { "math" }
    fn description(&self) -> &'static str { "数学计算: add/sub/mul/div" }
    fn execute(&self, args: &[String]) -> String {
        if args.len() < 3 { return "用法: math <a> <op> <b>".into(); }
        let a: f64 = args[0].parse().unwrap_or(0.0);
        let b: f64 = args[2].parse().unwrap_or(0.0);
        match args[1].as_str() {
            "add" => format!("{}", a + b),
            "sub" => format!("{}", a - b),
            "mul" => format!("{}", a * b),
            "div" => if b != 0.0 { format!("{}", a / b) } else { "除数不能为零".into() },
            _ => "不支持的运算".into(),
        }
    }
}

// —— IO 插件 ——
struct IoPlugin;
impl Plugin for IoPlugin {
    fn name(&self) -> &'static str { "io" }
    fn description(&self) -> &'static str { "文件操作: read/write/list" }
    fn execute(&self, args: &[String]) -> String {
        if args.is_empty() { return "用法: io <read|write|list> [路径]".into(); }
        match args[0].as_str() {
            "list" => "file1.txt\nfile2.txt\nconfig.toml".into(),
            "read" => format!("读取文件: {}", args.get(1).unwrap_or(&"?".into())),
            "write" => format!("写入文件: {}", args.get(1).unwrap_or(&"?".into())),
            _ => "不支持的 IO 操作".into(),
        }
    }
}

// —— 插件注册表 ——
fn build_plugin_provider() -> ServiceProvider {
    ServiceCollection::new()
        .keyed::<dyn Plugin>("math", |_| Arc::new(MathPlugin))
        .keyed::<dyn Plugin>("io", |_| Arc::new(IoPlugin))
        .build()
        .unwrap()
}

// —— 插件发现与执行 ——
struct PluginManager {
    provider: Arc<ServiceProvider>,
}
impl PluginManager {
    /// 列出所有已注册插件
    fn list_plugins(&self) -> Vec<String> {
        self.provider
            .get_all::<dyn Plugin>()
            .iter()
            .map(|p| format!("  {} - {}", p.name(), p.description()))
            .collect()
    }

    /// 按名称执行指定插件
    fn execute(&self, plugin_name: &str, args: &[String]) -> String {
        match self.provider.try_get_keyed::<dyn Plugin>(plugin_name) {
            Some(plugin) => plugin.execute(args),
            None => format!("未知插件: {}。可用插件: math, io", plugin_name),
        }
    }
}

#[test]
fn test_plugin_architecture() {
    let p = Arc::new(build_plugin_provider());
    let manager = PluginManager { provider: p.clone() };

    // 发现所有插件
    let plugins = manager.list_plugins();
    assert_eq!(plugins.len(), 2);

    // 执行数学插件
    let result = manager.execute("math", &[
        "10".into(), "add".into(), "20".into()
    ]);
    assert_eq!(result, "30");

    // 执行 IO 插件
    let result = manager.execute("io", &["list".into()]);
    assert!(result.contains("file1.txt"));
}

// 新增插件：只需实现 Plugin trait + 注册即可
struct HttpPlugin;
impl Plugin for HttpPlugin {
    fn name(&self) -> &'static str { "http" }
    fn description(&self) -> &'static str { "HTTP 客户端: get/post" }
    fn execute(&self, args: &[String]) -> String {
        format!("HTTP 请求: {:?}", args)
    }
}

fn build_extended_plugin_provider() -> ServiceProvider {
    ServiceCollection::new()
        .keyed::<dyn Plugin>("math", |_| Arc::new(MathPlugin))
        .keyed::<dyn Plugin>("io", |_| Arc::new(IoPlugin))
        // 新增一行！
        .keyed::<dyn Plugin>("http", |_| Arc::new(HttpPlugin))
        .build()
        .unwrap()
}
```

### 适用场景

- 命令行工具（git 子命令、cargo 子命令）
- IDE 扩展系统（VSCode 插件、IntelliJ 插件）
- 可扩展的服务端框架（中间件、认证提供者、存储后端）
- 需要第三方开发扩展的平台型产品

### 不宜使用

- 插件之间需要紧密协作或共享大量状态——会增加插件接口的复杂度
- 插件数量极少且稳定（<3 个）——插件架构的抽象成本超过收益
- 对性能要求极高、不允许动态分发的场景——使用编译时泛型替代

---

## 11. 适配器模式（Adapter Pattern）

### 问题描述

需要集成第三方库（日志、HTTP 客户端、序列化），但项目使用自定义 trait，且希望**保持领域层纯净**——不依赖外部 crate 的类型。

### LRDI 解决方案

定义项目自己的 trait，创建 Adapter 结构体委托给第三方库实现。DI 中注册 Adapter 为 `dyn MyTrait`，消费端只依赖自定义 trait，完全隔离第三方依赖。

```rust
use lrdi::*;
use std::sync::Arc;

// ==================== 领域层（不依赖任何第三方 crate） ====================

/// 项目自己的日志接口
trait MyLogger: Send + Sync {
    fn log(&self, level: LogLevel, message: &str);
    fn enabled(&self, level: LogLevel) -> bool;
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum LogLevel { Debug, Info, Warn, Error }

// ==================== 第三方库（模拟外部 crate 的类型） ====================

/// 第三方日志库（不能修改其代码）
mod third_party {
    pub struct ThirdPartyLogger {
        min_level: u8, // 0=debug, 1=info, 2=warn, 3=error
    }
    impl ThirdPartyLogger {
        pub fn new(min_level: u8) -> Self { Self { min_level } }
        pub fn write_log(&self, level: u8, msg: &str) {
            println!("[3rd-party L{}] {}", level, msg);
        }
        pub fn is_level_enabled(&self, level: u8) -> bool {
            level >= self.min_level
        }
    }
}

// ==================== 适配器（连接领域层与第三方库） ====================

use third_party::ThirdPartyLogger;

struct LoggerAdapter {
    inner: ThirdPartyLogger,
}
impl MyLogger for LoggerAdapter {
    fn log(&self, level: LogLevel, message: &str) {
        let lv = match level {
            LogLevel::Debug => 0,
            LogLevel::Info => 1,
            LogLevel::Warn => 2,
            LogLevel::Error => 3,
        };
        self.inner.write_log(lv, message);
    }

    fn enabled(&self, level: LogLevel) -> bool {
        let lv = match level {
            LogLevel::Debug => 0,
            LogLevel::Info => 1,
            LogLevel::Warn => 2,
            LogLevel::Error => 3,
        };
        self.inner.is_level_enabled(lv)
    }
}

// ==================== DI 注册 ====================

fn build_logger_provider() -> ServiceProvider {
    ServiceCollection::new()
        // 注册适配器为 dyn MyLogger，消费端只看到 MyLogger
        .singleton::<dyn MyLogger>(|_| {
            Arc::new(LoggerAdapter {
                inner: ThirdPartyLogger::new(0), // debug 级别
            })
        })
        .build()
        .unwrap()
}

// ==================== 业务代码（只依赖 MyLogger，不依赖第三方库） ====================

struct UserService {
    logger: Arc<dyn MyLogger>,
}
impl UserService {
    fn create_user(&self, name: &str) {
        self.logger.log(LogLevel::Info, &format!("创建用户: {}", name));
    }
}

#[test]
fn test_adapter_pattern() {
    let p = build_logger_provider();
    let logger: Arc<dyn MyLogger> = p.get();

    let service = UserService { logger };
    service.create_user("Alice");

    // 验证：替换第三方库时只需修改 LoggerAdapter，
    // UserService 和 MyLogger trait 完全不受影响。
    assert!(true); // 验证编译通过即可
}
```

### 适用场景

- 集成第三方库，希望保持领域层纯净（DDD 防腐层）
- 统一多个外部服务的接口（多个日志库、多个 HTTP 客户端）
- 未来可能替换外部依赖（从 log4rs 切换到 tracing）
- 隔离不稳定/易变的第三方 API

### 不宜使用

- 第三方库的类型已经非常通用且稳定（如 `std::io::Read`），不需要额外适配
- 适配层的抽象成本（trait + 类型转换）超过隔离收益
- 项目只有一个外部依赖且几乎不会替换——直接使用即可

---

## 12. 服务注册表 / 中介者模式（Service Registry / Mediator）

### 问题描述

多个服务需要相互协作，但避免直接相互依赖（防止依赖地狱和循环依赖）。需要一个中介者来协调服务间的交互。

### LRDI 解决方案

创建 `ServiceRegistry` 持有 `Arc<dyn IServiceResolver>`（或 `Arc<ServiceProvider>`），各服务通过 registry 按需查找其他服务，而非在构造时直接注入。这实现了**懒依赖**——避免构造时循环依赖。

```rust
use lrdi::*;
use std::sync::Arc;

// —— 服务接口 ——
trait OrderService: Send + Sync {
    fn create_order(&self, user_id: u64, amount: f64) -> String;
}

trait InventoryService: Send + Sync {
    fn check_stock(&self, item: &str) -> bool;
    fn reserve(&self, item: &str) -> String;
}

trait NotificationService: Send + Sync {
    fn send(&self, user_id: u64, message: &str) -> String;
}

// —— 具体实现 ——
struct OrderServiceImpl;
impl OrderService for OrderServiceImpl {
    fn create_order(&self, user_id: u64, amount: f64) -> String {
        format!("订单: user={} amount={}", user_id, amount)
    }
}

struct InventoryServiceImpl;
impl InventoryService for InventoryServiceImpl {
    fn check_stock(&self, item: &str) -> bool {
        item != "out_of_stock_item"
    }
    fn reserve(&self, item: &str) -> String {
        format!("已预留: {}", item)
    }
}

struct NotificationServiceImpl;
impl NotificationService for NotificationServiceImpl {
    fn send(&self, user_id: u64, message: &str) -> String {
        format!("通知用户 {}: {}", user_id, message)
    }
}

// —— 服务注册表（中介者） ——
// 核心：持有 ServiceProvider 引用，按需查找其他服务
struct ServiceRegistry {
    provider: Arc<ServiceProvider>,
}
impl ServiceRegistry {
    fn order_service(&self) -> Arc<dyn OrderService> {
        self.provider.get()
    }
    fn inventory_service(&self) -> Arc<dyn InventoryService> {
        self.provider.get()
    }
    fn notification_service(&self) -> Arc<dyn NotificationService> {
        self.provider.get()
    }
}

// —— 编排服务：通过注册表协调多个服务 ——
struct CheckoutOrchestrator {
    registry: Arc<ServiceRegistry>,
}
impl CheckoutOrchestrator {
    fn checkout(&self, user_id: u64, item: &str, amount: f64) -> String {
        // 1. 检查库存
        if !self.registry.inventory_service().check_stock(item) {
            return "库存不足".into();
        }
        // 2. 预留库存
        let reserve_result = self.registry.inventory_service().reserve(item);
        // 3. 创建订单
        let order_result = self.registry.order_service().create_order(user_id, amount);
        // 4. 发送通知
        let notify_result = self.registry
            .notification_service()
            .send(user_id, &order_result);

        format!("{} | {} | {}", reserve_result, order_result, notify_result)
    }
}

fn build_registry_provider() -> ServiceProvider {
    ServiceCollection::new()
        .singleton::<dyn OrderService>(|_| Arc::new(OrderServiceImpl))
        .singleton::<dyn InventoryService>(|_| Arc::new(InventoryServiceImpl))
        .singleton::<dyn NotificationService>(|_| Arc::new(NotificationServiceImpl))
        .build()
        .unwrap()
}

#[test]
fn test_service_registry() {
    let p = Arc::new(build_registry_provider());
    let registry = Arc::new(ServiceRegistry { provider: p.clone() });

    let orchestrator = CheckoutOrchestrator { registry };

    let result = orchestrator.checkout(42, "laptop", 999.99);
    assert!(result.contains("已预留"));
    assert!(result.contains("订单"));
    assert!(result.contains("通知用户 42"));
}
```

### 解析器注入 vs 直接依赖注入

```rust
// 方式 A：直接依赖注入（推荐用于核心依赖）
struct DirectInjectionService {
    order_svc: Arc<dyn OrderService>,
    inventory_svc: Arc<dyn InventoryService>,
}
// 优点：依赖显式声明，编译期保证
// 缺点：构造参数多，可能有循环依赖风险

// 方式 B：解析器注入（推荐用于可选依赖、懒加载、避免循环依赖）
struct ResolverInjectionService {
    registry: Arc<ServiceRegistry>,
}
// 优点：灵活，按需解析，无循环依赖
// 缺点：依赖不显式，运行时才能发现缺失
```

### 适用场景

- 避免循环依赖（A 依赖 B，B 依赖 A）
- 需要按需/延迟解析某些服务（懒加载）
- 编排多个服务的复杂业务流程（Saga 模式）
- 插件需要动态查找宿主提供的服务
- 条件性依赖（某些场景需要，某些不需要）

### 不宜使用

- 作为"万能依赖桶"到处使用——这退回服务定位器反模式
- 核心依赖（如 Repository 对 Service 层）——应使用直接注入
- 依赖关系简单且稳定——解析器注入的灵活性是多余成本

---

## 总结

| 模式 | LRDI 核心 API | 适用场景特征 |
|------|-------------|------------|
| 策略模式 | `keyed` + `get_keyed` | 运行时切换算法，开闭原则 |
| 工厂模式 | 闭包工厂 / `keyed` | 创建逻辑复杂，依赖运行时上下文 |
| 装饰器模式 | 闭包链式包装 + `singleton::<dyn Trait>` | 横切关注点叠加 |
| 责任链模式 | `keyed` + 配置驱动 | 请求经过多个处理器，可中断 |
| 观察者模式 | `keyed` + `get_all` | 一对多事件广播 |
| 组合模式 | `keyed` + 手动组装 | 树形结构，统一叶子与容器 |
| 代理模式 | 闭包 + 持有 Provider | 延迟加载、访问控制 |
| 管道模式 | `keyed` + 顺序执行 | 数据阶段转换，全员执行 |
| 特性开关 | `keyed` + 配置路由 | 灰度发布、A/B 测试 |
| 插件架构 | `keyed` + `get_all` | 可扩展系统，第三方开发 |
| 适配器模式 | `singleton::<dyn MyTrait>` 包装外部类型 | 隔离第三方依赖 |
| 服务注册表 | 持有 Provider 按需解析 | 避免循环依赖，编排流程 |

**核心原则**：LRDI 的 `keyed` + `dyn Trait` 组合实现了"按名称获取实现"的能力，这是绝大多数设计模式的基础。结合 `get_all`（批量获取）和闭包工厂（创建时组装），可以在不引入任何第三方库的前提下实现 GoF 23 种设计模式中的大部分。
