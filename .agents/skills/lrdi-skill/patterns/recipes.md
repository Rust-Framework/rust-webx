# LRDI 完整应用示例集

## 快速起步模板

### 模板 1：最小化 LRDI 项目（`#[inject]` 属性宏）

```rust
use rust_dicore::*;
use std::sync::Arc;

#[inject(singleton)]
struct Config;

#[inject(transient)]
struct AppService {
    #[inject]
    config: Arc<Config>,
}

// handlers 层：实现接口并注册（struct 用 #[derive(Inject)] 仅生成构造函数，impl 用 #[inject] 注册为 trait）
#[derive(Inject)]
struct UserService {
    #[inject]
    db: Arc<dyn IDbContext>,  // 依赖接口
}

#[inject]  // 在 impl 上：注册为接口
impl IUserService for UserService {
    fn create(&self, name: &str) -> User { ... }
}

// Handler 依赖接口
#[inject(transient)]
struct CreateUserHandler {
    #[inject]
    user_svc: Arc<dyn IUserService>,  // 接口依赖，可测试可替换
}

fn main() {
    let provider = Arc::new(
        ServiceCollection::from_injected()
            .build()
            .unwrap()
    );
    let handler: Arc<CreateUserHandler> = provider.get();
    handler.user_svc.create("Alice");
}
```

### 模板 2：策略模式（keyed 多实现）

```rust
use rust_dicore::*;
use std::sync::Arc;

// 多实现策略：struct 用 #[derive(Inject)] 仅生成构造函数，impl 用 #[inject] 注册为 trait
#[derive(Inject)]
struct CreditCardGateway;

#[inject]  // 在 impl 上：注册为默认 dyn IPayGateway
impl IPayGateway for CreditCardGateway { ... }

// 组合根补充 keyed 注册
let provider = ServiceCollection::from_injected()
    .keyed_singleton::<dyn IPayGateway>("wechat", |_| Arc::new(WechatGateway))
    .keyed_singleton::<dyn IPayGateway>("alipay", |_| Arc::new(AlipayGateway))
    .build()
    .unwrap();

// 运行时路由
let gateway: Arc<dyn IPayGateway> = provider
    .try_get_keyed(method)
    .unwrap_or_else(|| provider.get());  // 回退默认
```

### 模板 3：带作用域的 Web 请求处理

```rust
use rust_dicore::*;
use std::sync::Arc;

fn handle_request(provider: &Arc<ServiceProvider>, req: Request) -> Response {
    let scope = provider.create_scope();
    let ctx: Arc<RequestContext> = scope.get();
    let handler: Arc<RequestHandler> = scope.get();
    handler.handle(ctx, req)
}
```

### 模板 4：带 mock 的测试

```rust
use rust_dicore::*;
use std::sync::Arc;

#[test]
fn test_business_logic() {
    let p = ServiceCollection::new()
        .singleton(|_| Arc::new(MockRepo::new()))
        .transient(|p| Arc::new(MyService::new(p.get())))
        .build().unwrap();
    let svc: Arc<MyService> = p.get();
    let result = svc.do_something();
    assert!(result.is_ok());
}
```

---

## 端到端示例

以下为完整应用示例大纲，适合需要多文件协作的复杂场景。

## 1. Web 服务（三层架构 + Scoped 请求）

完整的 Actix-web / Axum 风格 Web 服务示例：
- 定义 trait: UserRepo, OrderRepo（数据访问）
- 定义 trait: UserService, OrderService（业务逻辑）
- 实现: PgUserRepo, PgOrderRepo, UserServiceImpl, OrderServiceImpl
- 使用 #[derive(Inject)] 自动生成服务构造函数
- 每个 HTTP 请求创建 Scope，RequestContext 为 Scoped
- 请求处理器从 Scope 解析服务

```rust
// 完整示例：main.rs + handlers.rs + services.rs + repos.rs 的骨架代码
```

包含：
- Config 从环境变量加载（Singleton）
- DbPool 连接池（Singleton）
- RequestContext 含 request_id, user_id（Scoped）
- UserRepo, OrderRepo（Transient，依赖 DbPool）
- UserService, OrderService（Transient，依赖各 Repo）
- 请求处理流程：创建 Scope → 解析 RequestContext → 解析 Service → 执行

## 2. CLI 工具（命令模式 + 键控服务）

完整的命令行工具示例：
- 定义 Command trait: name(), execute(args: &[String]), help()
- 实现多个命令：InitCommand, BuildCommand, ServeCommand
- 使用 keyed 注册所有命令
- 主循环：解析 args → get_keyed(command_name) → execute

展示：
- 如何通过 get_all() 生成帮助信息（列出所有命令）
- 如何添加新命令而不修改主循环
- 如何实现命令别名

## 3. 微内核插件架构

完整示例：
- 宿主核心（Core）：事件总线、配置服务、日志服务（Singleton，root provider）
- 插件接口：trait Plugin + 元数据
- 插件发现：扫描 plugins/ 目录，每个子目录一个 plugin config
- 插件加载：为每个插件构建独立的 ServiceCollection → ServiceProvider → ServiceProviderWrapper
- 插件间通信：通过 root 中的 EventBus
- 插件覆盖：插件可以覆盖 root 的 Logger（child-first）

展示完整的模块结构和代码。

## 4. 多租户 SaaS

完整示例：
- 共享基础设施：数据库连接池、Redis 缓存、消息队列（Singleton，root）
- 租户级服务：每个租户有独立的 ServiceProvider（child）
- 租户解析中间件：从请求中提取 tenant_id
- 租户容器工厂：创建/缓存/销毁租户容器
- 租户特定配置覆盖

展示 ServiceProviderWrapper 在多租户场景中的应用。

## 5. 特性开关系统

完整示例：
- Feature trait
- 配置驱动的 FeatureService
- 与 LRDI 集成：feature 作为 keyed 服务，运行时根据配置选择
- FeatureService 作为 Singleton，应用启动时从配置中心加载
- A/B 测试路由逻辑

## 6. 事件驱动架构

完整示例：
- Event trait: type_name(), payload()
- EventBus: 订阅/发布/取消订阅
- EventHandler trait: handle(event)
- 使用 get_all::<dyn EventHandler>() 自动发现所有处理器
- 事件发布者注入 EventBus（Singleton），事件处理器自动注册

展示 DI + 事件总线的协同工作。
