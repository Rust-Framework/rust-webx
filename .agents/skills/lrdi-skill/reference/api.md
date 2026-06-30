# LRDI API 完整参考

> 本文档是 LRDI（Rust 依赖注入框架）的权威 API 参考。根据框架源代码逐方法生成，涵盖所有公开 API 的签名、参数、返回值、副作用和使用示例。

> **导入约定**：本文示例为明确宏来源使用全限定路径（`#[rust_dicore::inject]` 等）。实际编码推荐 `use rust_dicore::*;` 后用 `#[inject]` 等简短形式，详见 `guides/usage-guide.md`。

---

## 目录

1. [ServiceLifetime 枚举](#servicelifetime-枚举)
2. [ServiceCollection（注册 API）](#servicecollection注册-api)
3. [过程宏（Procedural Macros）](#过程宏procedural-macros)
4. [ServiceProvider（解析 API）](#serviceprovider解析-api)
5. [Scope（作用域 API）](#scope作用域-api)
6. [ServiceProviderWrapper（分层容器 API）](#serviceproviderwrapper分层容器-api)
7. [IServiceResolver（核心 trait）](#iserviceresolver核心-trait)
8. [IServiceLocator / ServiceLocatorBridge / RdiProvider](#iservicelocator--servicelocatorbridge--rdiprovider)
9. [RdiError 错误类型](#rdierror-错误类型)

---

## ServiceLifetime 枚举

定义于 `rust_dicore::lifetime`，通过 `rust_dicore::ServiceLifetime` 重新导出。

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceLifetime {
    Transient,
    Scoped,
    Singleton,
}
```

| 变体 | 含义 |
|------|------|
| `Transient` | 每次解析均创建新实例。不从任何缓存读取，也不将结果写入任何缓存。 |
| `Scoped` | 在每个 `Scope` 内缓存在该 Scope 的 `scoped_cache` 中。同一 Scope 内多次解析返回同一实例；跨 Scope 返回不同实例。`ServiceProvider` 本身即 root scope，从根解析 Scoped 服务时在 `root_scoped_cache` 中缓存复用。 |
| `Singleton` | 在 `ServiceProvider::new()` 构建阶段立即执行工厂函数（eager initialization），结果存入 `singleton_cache`（`LazyCache`，per-key `OnceLock`，多线程下工厂只执行一次）。后续所有解析均返回同一缓存实例。build 阶段会检测 captive dependency（Singleton 依赖 Scoped），违反时返回 `RdiError::SingletonDependsOnScoped`。 |

### 生命周期互引用规则

| 外层生命周期 | 可引用 | build 阶段拒绝 | 需开发者自行保证 |
|-------------|--------|---------------|----------------|
| Singleton | Singleton、Transient | Scoped（直接或间接） | — |
| Scoped | Singleton、Scoped | — | 不引用 Transient（Transient 无法被 Scope 缓存持有，可能造成语义混乱） |
| Transient | 所有 | — | — |

> **Captive dependency 检测**：`build()` 阶段会检测 `Singleton → Scoped` 直接或间接（`Singleton → Transient → Scoped`）依赖链，违反时返回 `RdiError::SingletonDependsOnScoped`。Singleton 引用 Transient 是允许的，但需注意：Singleton 永久持有该 Transient 实例（不会随 scope 变化），等同于将其提升为 Singleton 语义。

---

## ServiceCollection（注册 API）

定义于 `rust_dicore::collection`，通过 `rust_dicore::ServiceCollection` 重新导出。

```rust
pub struct ServiceCollection {
    descriptors: Vec<ServiceDescriptor>,
}
```

`ServiceCollection` 是构建器（builder），使用链式调用风格。每个注册方法消耗 `self` 并返回 `Self`。最终调用 `build()` 产出 `Result<ServiceProvider, RdiError>`。

### 类型约束通用规则

除 `instance` 和 `singleton_value` 外，所有方法的类型参数 `T` 均受 `T: ?Sized + Send + Sync + 'static` 约束。这意味着：

- `?Sized`：支持 `dyn Trait` 注册（例如 `dyn MyService`）
- `Send + Sync`：保证跨线程安全
- `'static`：无借用生命周期

### 工厂闭包参数

工厂闭包的参数 `&dyn IServiceResolver` 允许在创建服务实例时解析其依赖项。可以通过 `resolver.get::<Dep>()` 获取依赖。

### TypeId 确定解析身份

每个注册的内部使用 `TypeId::of::<T>()` 对条目分组。因此：

- **具体类型**：`singleton(|_| Arc::new(Foo))` 注册类型 `Foo`，TypeId = `TypeId::of::<Foo>()`
- **Trait 对象**：`singleton(|_| Arc::new(Foo) as Arc<dyn MyTrait>)` — 需显式指定泛型参数 `::<dyn MyTrait>`，TypeId = `TypeId::of::<dyn MyTrait>()`

如果不显式指定 `::<dyn Trait>`，Rust 会推断为具体类型，导致无法按 trait 解析。

```rust
// ✅ 正确——按 trait 解析
col.singleton::<dyn MyTrait>(|_| Arc::new(FooImpl));

// ❌ 错误——注册为 FooImpl，无法按 dyn MyTrait 解析
col.singleton(|_| Arc::new(FooImpl));
```

---

### `new()` — 创建空集合

```rust
pub fn new() -> Self
```

**签名**：

```rust
impl ServiceCollection {
    pub fn new() -> Self
}
```

**参数**：无

**返回值**：`ServiceCollection`，内部 `descriptors` 为空 `Vec`

**副作用**：无

**示例**：

```rust
use rust_dicore::ServiceCollection;

let collection = ServiceCollection::new();
let provider = collection.build().unwrap();
```

---

### `singleton(f)` — 注册 Singleton

```rust
pub fn singleton<T: ?Sized + Send + Sync + 'static>(
    mut self,
    f: impl Fn(&dyn IServiceResolver) -> Arc<T> + Send + Sync + 'static,
) -> Self
```

**参数**：

| 参数 | 说明 |
|------|------|
| `T` | 服务类型（支持 `dyn Trait`） |
| `f` | 工厂闭包，接收 `&dyn IServiceResolver`，返回 `Arc<T>` |

**返回值**：`Self`（链式调用）

**副作用**：向内部 `descriptors` 追加一个 `lifetime: Singleton`、`key: None` 的 `ServiceDescriptor`

**行为**：`build()` 时立即执行工厂；后续所有 `get::<T>()` 返回同一缓存实例

**示例**：

```rust
use std::sync::Arc;
use rust_dicore::{ServiceCollection, IServiceResolver};

struct Database {
    conn_str: String,
}

let provider = ServiceCollection::new()
    .singleton(|_| Arc::new(Database { conn_str: "postgres://...".into() }))
    .build()
    .unwrap();

let db1 = provider.get::<Database>();
let db2 = provider.get::<Database>();
assert!(Arc::ptr_eq(&db1, &db2)); // 同一实例
```

---

### `scoped(f)` — 注册 Scoped

```rust
pub fn scoped<T: ?Sized + Send + Sync + 'static>(
    mut self,
    f: impl Fn(&dyn IServiceResolver) -> Arc<T> + Send + Sync + 'static,
) -> Self
```

**参数**：与 `singleton` 相同

**副作用**：追加 `lifetime: Scoped`、`key: None` 的 `ServiceDescriptor`

**行为**：在每个 `Scope` 内缓存实例；跨子 Scope 创建不同实例。`ServiceProvider` 本身即 root scope，从根解析时实例缓存在 `root_scoped_cache`（后续根级解析返回同一实例，等同应用级单例）。需要请求级隔离时必须用 `create_scope()` 创建子 Scope。

**示例**：

```rust
use std::sync::Arc;
use rust_dicore::ServiceCollection;

struct Session {
    id: u64,
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

let provider = Arc::new(
    ServiceCollection::new()
        .scoped(|_| Arc::new(Session {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
        }))
        .build()
        .unwrap(),
);

let scope1 = provider.create_scope();
let a = scope1.get::<Session>();
let b = scope1.get::<Session>();
assert_eq!(a.id, b.id); // 同一 Scope 内相同

let scope2 = provider.create_scope();
let c = scope2.get::<Session>();
assert_ne!(a.id, c.id); // 跨 Scope 不同
```

---

### `transient(f)` — 注册 Transient

```rust
pub fn transient<T: ?Sized + Send + Sync + 'static>(
    mut self,
    f: impl Fn(&dyn IServiceResolver) -> Arc<T> + Send + Sync + 'static,
) -> Self
```

**参数**：与 `singleton` 相同

**副作用**：追加 `lifetime: Transient`、`key: None` 的 `ServiceDescriptor`

**行为**：每次都执行工厂、返回新实例，不写入任何缓存

**示例**：

```rust
use std::sync::Arc;
use rust_dicore::ServiceCollection;

struct Operation {
    id: u64,
}

static NEXT_OP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

let provider = ServiceCollection::new()
    .transient(|_| Arc::new(Operation {
        id: NEXT_OP.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
    }))
    .build()
    .unwrap();

let op1 = provider.get::<Operation>();
let op2 = provider.get::<Operation>();
assert_ne!(op1.id, op2.id); // 每次不同
```

---

### `keyed(k, f)` — 注册键控 Singleton

```rust
pub fn keyed<T: ?Sized + Send + Sync + 'static>(
    mut self,
    k: impl Into<String>,
    f: impl Fn(&dyn IServiceResolver) -> Arc<T> + Send + Sync + 'static,
) -> Self
```

**参数**：

| 参数 | 说明 |
|------|------|
| `k` | 键名（`impl Into<String>`，支持 `&str`、`String`） |
| `f` | 工厂闭包 |

**副作用**：追加 `lifetime: Singleton`、`key: Some(k.into())` 的 `ServiceDescriptor`

**行为**：与 `singleton` 相同，但通过 `get_keyed::<T>(key)` 按类型+键名解析

**示例**：

```rust
struct PaymentGateway {
    name: String,
}

let provider = ServiceCollection::new()
    .keyed("wechat", |_| Arc::new(PaymentGateway { name: "微信支付".into() }))
    .keyed("alipay", |_| Arc::new(PaymentGateway { name: "支付宝".into() }))
    .build()
    .unwrap();

let wechat = provider.get_keyed::<PaymentGateway>("wechat");
let alipay = provider.get_keyed::<PaymentGateway>("alipay");
assert_eq!(wechat.name, "微信支付");
assert_eq!(alipay.name, "支付宝");
```

---

### `keyed_scoped(k, f)` — 注册键控 Scoped

```rust
pub fn keyed_scoped<T: ?Sized + Send + Sync + 'static>(
    mut self,
    k: impl Into<String>,
    f: impl Fn(&dyn IServiceResolver) -> Arc<T> + Send + Sync + 'static,
) -> Self
```

**副作用**：追加 `lifetime: Scoped`、`key: Some(k.into())` 的 `ServiceDescriptor`

**行为**：键控版 Scoped 服务，Scope 内缓存，跨 Scope 不同实例

---

### `keyed_transient(k, f)` — 注册键控 Transient

```rust
pub fn keyed_transient<T: ?Sized + Send + Sync + 'static>(
    mut self,
    k: impl Into<String>,
    f: impl Fn(&dyn IServiceResolver) -> Arc<T> + Send + Sync + 'static,
) -> Self
```

**副作用**：追加 `lifetime: Transient`、`key: Some(k.into())` 的 `ServiceDescriptor`

**行为**：键控版 Transient 服务，每次解析创建新实例

---

### `instance(arc)` — 注册预构建 Arc\<T\> 为 Singleton

```rust
pub fn instance<T: Send + Sync + 'static>(mut self, v: Arc<T>) -> Self
```

**参数**：

| 参数 | 说明 |
|------|------|
| `T` | 具体类型（不支持 `?Sized`，不支持 `dyn Trait`） |
| `v` | 已构造好的 `Arc<T>` |

**副作用**：追加 `lifetime: Singleton`、`key: None` 的 `ServiceDescriptor`。内部工厂为 `Arc::new(move |_| Arc::new(v.clone()))`，即直接包装传入的 `Arc<T>`。

**行为**：`build()` 时立即执行工厂，结果存入 Singleton 缓存。解析时返回同一个 `Arc<T>`（`Arc::ptr_eq` 为 true）。

**示例**：

```rust
let shared_config = Arc::new(Config { debug: true });
let provider = ServiceCollection::new()
    .instance(shared_config.clone())
    .build()
    .unwrap();

let resolved = provider.get::<Config>();
assert!(Arc::ptr_eq(&shared_config, &resolved));
```

---

### `singleton_value(v)` — 注册普通值为 Singleton

```rust
pub fn singleton_value<T: Send + Sync + 'static>(self, v: T) -> Self
```

**实现**：等价于 `self.instance(Arc::new(v))`

**示例**：

```rust
let provider = ServiceCollection::new()
    .singleton_value("my_connection_string".to_string())
    .build()
    .unwrap();

let conn_str = provider.get::<String>();
assert_eq!(*conn_str, "my_connection_string");
```

---

### `try_add(f)` — 条件 Singleton 注册

```rust
pub fn try_add<T: ?Sized + Send + Sync + 'static>(
    mut self,
    f: impl Fn(&dyn IServiceResolver) -> Arc<T> + Send + Sync + 'static,
) -> Self
```

**参数**：与 `singleton` 相同

**副作用**：先检查是否存在同 `TypeId::of::<T>()` 且 `key.is_none()` 的条目。若已存在，直接返回 `self`（不追加）；否则追加 `lifetime: Singleton`、`key: None` 的条目。

**使用场景**：插件模块或可选覆盖注册，避免重复注册报错

**示例**：

```rust
let provider = ServiceCollection::new()
    .singleton(|_| Arc::new(Logger { level: "info".into() }))
    .try_add(|_| Arc::new(Logger { level: "debug".into() }))  // 不会生效
    .build()
    .unwrap();

let logger = provider.get::<Logger>();
assert_eq!(logger.level, "info"); // 第一个生效
```

---

### `add(lt, f)` — 显式指定生命周期注册

```rust
pub fn add<T: ?Sized + Send + Sync + 'static>(
    mut self,
    lt: ServiceLifetime,
    f: impl Fn(&dyn IServiceResolver) -> Arc<T> + Send + Sync + 'static,
) -> Self
```

**参数**：

| 参数 | 说明 |
|------|------|
| `lt` | 显式 `ServiceLifetime` 变体 |
| `f` | 工厂闭包 |

**副作用**：追加 `lifetime: lt`、`key: None` 的 `ServiceDescriptor`

**示例**：

```rust
use rust_dicore::ServiceLifetime;

let provider = ServiceCollection::new()
    .add(ServiceLifetime::Singleton, |_| Arc::new(Cache::new()))
    .add(ServiceLifetime::Transient, |_| Arc::new(RequestId::new()))
    .build()
    .unwrap();
```

---

### `build()` — 构建 ServiceProvider

```rust
pub fn build(self) -> Result<ServiceProvider, RdiError>
```

**参数**：消耗 `self`

**返回值**：`Result<ServiceProvider, RdiError>`

**副作用**：

1. 遍历所有 `ServiceDescriptor`，按 `TypeId` 分组存入 `ServiceStore`（`HashMap<TypeId, Vec<ServiceEntry>>`）
2. 为每个条目分配 `cache_key`（按注册顺序的 `usize` 索引）
3. 构建 `type_map`（`type_name -> TypeId` 查找表，用于字符串查找）
4. **Captive dependency 检测**：遍历 Singleton 工厂闭包引用的依赖类型，若发现 `Singleton → Scoped` 直接或间接依赖，立即返回 `Err(RdiError::SingletonDependsOnScoped)`，拒绝构建容器
5. 执行**二阶段 Singleton 初始化**：
   - **阶段一**：收集所有 `lifetime == Singleton` 的条目
   - **阶段二**：逐个执行工厂闭包，结果通过 `LazyCache::get_or_init_with` 写入 `singleton_cache`（per-key `OnceLock`，多线程下工厂只执行一次）。如果 A 的工厂引用了尚未初始化的 B，则 B 的工厂惰性执行并回填缓存
6. 返回 `Ok(ServiceProvider)`

**错误**：若检测到 `Singleton → Scoped` 依赖（含 `Singleton → Transient → Scoped` 间接链），返回 `Err(RdiError::SingletonDependsOnScoped { singleton_type, scoped_type })`。

### `from_injected()` — 从属性宏构建集合

```rust
pub fn from_injected() -> Self
```

收集当前 binary 中所有 `#[rust_dicore::inject]` 标注的服务，构建 `ServiceCollection`。
必须在应用入口处调用一次。之后可以链式调用其他注册方法补充服务。

**实现机制**：遍历 `inventory` 迭代器中所有 `ServiceRegistration` 条目（由 `#[rust_dicore::inject]`
在编译期通过 `inventory::submit!` 提交），将其转换为 `ServiceDescriptor` 并存入新集合。

**示例：**

```rust
use rust_dicore::*;
use std::sync::Arc;

#[rust_dicore::inject(singleton)]
struct Logger { prefix: String }

#[rust_dicore::inject(transient)]
struct Worker { logger: Arc<Logger> }

let provider = ServiceCollection::from_injected()
    .singleton(|_| Arc::new(ExtraService))
    .build()
    .unwrap();
```

---

## 过程宏（Procedural Macros）

定义于 `rust-dicore-macros` crate，通过 `rust-dicore` 重新导出。

### `#[rust_dicore::inject]` — 属性宏自动注册

```rust
// 语法格式：
//   #[rust_dicore::inject(<lifetime>)]          // 放在 struct 上：按自身类型注册
//   #[rust_dicore::inject]                      // 放在 `impl Trait for T` 上：按 trait 注册
```

> **放置位置二选一**：`#[inject]` 一般只放一处。放 struct 上 = 注册具体类型；放 impl 上 = 注册 `dyn Trait`。两者同时用会双重注册（通常非预期）。面向接口时 struct 上用 `#[derive(Inject)]`（仅生成构造函数、不注册），impl 上用 `#[inject]`。详见 `guides/usage-guide.md` §放置位置决策与 `guides/macros.md` §2.1。

**生命周期参数：**

| 生命周期 | 含义 |
|------|------|
| `singleton` | Singleton |
| `scoped` | Scoped |
| `transient` | Transient |

**功能：** 标注在 struct 上，在编译期生成三项代码：

1. 一个构造函数 `__rdi_construct_<StructName>(resolver: &dyn IServiceResolver) -> Arc<StructName>`，
   自动从 DI 容器解析所有**标记了 `#[inject(...)]` 的**命名字段（未标记字段用 `Default::default()`）
2. 一个工厂函数，将构造函数包装为 `ServiceFactory` 签名
3. 通过 `rust_dicore::inventory::submit!` 提交 `ServiceRegistration` 条目，供
   `ServiceCollection::from_injected()` 收集

**字段属性（`#[inject(...)]`）：**

显式注入策略：只有标记了 `#[inject(...)]` 的字段才从容器解析，未标记字段使用 `Default::default()`。
可选性由字段类型（`Option<...>`）决定，无需额外标记。

| 属性 | 字段类型 | 含义 |
|------|---------|------|
| *(无标记)* | 任意（需 `Default`） | 内部字段 — 使用 `Default::default()` |
| `#[inject]` | `Arc<T>` | 必选共享 — 未注册则 panic |
| `#[inject]` | `Option<Arc<T>>` | 可选共享 — 未注册返回 `None` |
| `#[inject(key = "k")]` | `Arc<T>` | 键控共享，找不到 key 则 panic |
| `#[inject(key = "k")]` | `Option<Arc<T>>` | 可选键控共享 |
| `#[inject(owned)]` | 裸 `T` | 必选 owned — 未注册或 Singleton 则 panic |
| `#[inject(owned)]` | `Option<T>` | 可选 owned — 未注册或 Singleton 返回 `None` |
| `#[inject(owned, key = "k")]` | 裸 `T` / `Option<T>` | 键控 owned |
| `#[inject(provider)]` | `Arc<dyn IServiceResolver>` | 注入 resolver 自身 |

> 严格类型校验：`#[inject]` 只接受 `Arc<T>` / `Option<Arc<T>>`，
> `#[inject(owned)]` 只接受裸 `T` / `Option<T>`。标记与类型不匹配会触发编译错误。

**示例：**

```rust
use rust_dicore::*;
use std::sync::Arc;

trait ILogger: Send + Sync { fn log(&self, msg: &str); }

// 按自身类型注册为 Singleton
#[rust_dicore::inject(singleton)]
struct Logger { prefix: String }

// 按 trait 注册：struct 上先用 #[derive(Inject)] 生成构造函数（不注册），再在 impl 块上用 #[inject] 注册为 dyn Trait
#[derive(Inject)]
struct ConsoleLogger;
#[rust_dicore::inject]
impl ILogger for ConsoleLogger {
    fn log(&self, msg: &str) { println!("{msg}"); }
}

// 字段属性示例
#[rust_dicore::inject(singleton)]
struct AppService {
    #[inject]
    logger: Option<Arc<dyn ILogger>>,
    name: String,  // 未标记 → Default::default()
}

let provider = ServiceCollection::from_injected()
    .singleton_value("my_app")
    .build()
    .unwrap();

let logger: Arc<Logger> = provider.get();
```

### `#[derive(Inject)]` — 派生宏构造器注入

```rust
#[derive(Inject)]
struct MyService { /* ... */ }
```

为 struct 生成 `__rdi_construct_<StructName>(resolver: &dyn IServiceResolver) -> Arc<StructName>`
构造函数。与 `#[rust_dicore::inject]` 的区别：**不自注册到容器**，仅生成构造函数代码，
需手动通过 `ServiceCollection::new().transient(|p| __rdi_construct_MyService(p))` 等方式注册。

字段属性 `#[inject(...)]` 的语义与 `#[rust_dicore::inject]` 相同。

### `#[rust_dicore::module]` — 编译期模块扫描

```rust
#[rust_dicore::module]
mod services {
    rust_dicore::register!(singleton: dyn ILogger => ConsoleLogger);
    rust_dicore::register!(transient: MyService);
    rust_dicore::register!(keyed "x": singleton: dyn IPlugin => XPlugin);
}

let provider = services::__rdi_build_provider_services();
```

`#[rust_dicore::module]` 扫描模块内所有 `rust_dicore::register!(...)` 声明，生成一个
`__rdi_build_provider_<mod_name>()` 函数，调用该函数即可获得预配置的 `ServiceProvider`。

支持的 `rust_dicore::register!(...)` 语法：

| 语法 | 含义 |
|------|------|
| `singleton: MyType` | 按自身类型注册为 Singleton |
| `scoped: MyType` | 按自身类型注册为 Scoped |
| `transient: MyType` | 按自身类型注册为 Transient |
| `singleton: dyn Trait => ImplStruct` | 按 trait 注册 Singleton |
| `keyed "k": singleton: MyType` | 键控注册 |
| `factory singleton: MyType => MyType { ... }` | 工厂表达式注册 |

---

## ServiceProvider（解析 API）

定义于 `rust_dicore::provider`，通过 `rust_dicore::ServiceProvider` 重新导出。

```rust
pub struct ServiceProvider {
    store: ServiceStore,                                    // TypeId → Vec<ServiceEntry>
    type_map: HashMap<&'static str, TypeId>,                // type_name → TypeId
    singleton_cache: LazyCache,                             // Singleton 实例缓存（per-key OnceLock）
    root_scoped_cache: LazyCache,                           // 根 scope 的 Scoped 实例缓存
    named: RwLock<HashMap<String, Arc<dyn Any + Send + Sync>>>,
}
```

`ServiceProvider` 是根 DI 容器，同时承担 **root scope** 角色：从根直接解析 Scoped 服务时，实例缓存在 `root_scoped_cache` 中复用（与子 `Scope` 的 `scoped_cache` 隔离）。`build()` 后只读，`singleton_cache` 与 `root_scoped_cache` 均使用 `LazyCache`（`RwLock<HashMap<key, Arc<OnceLock<AnyService>>>>`），per-key `OnceLock` 保证多线程下工厂只执行一次。

---

### `get::<T>()` — 按类型解析（panic 版）

```rust
pub fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Arc<T>
```

**行为**：调用 `self.try_get::<T>()`，若返回 `None` 则 panic，消息格式为 `"service not registered: {type_name}"`。

**适用场景**：必需依赖——服务缺失意味着程序无法运行

**示例**：

```rust
let logger = provider.get::<Logger>();  // 不存在则 panic
```

---

### `get_service::<T>()` — 按类型解析（MEDI 命名）

```rust
pub fn get_service<T: ?Sized + Send + Sync + 'static>(&self) -> Option<Arc<T>>
```

**行为**：等价于 `self.get_optional::<T>()`。MEDI（Microsoft.Extensions.DependencyInjection）风格命名，对应 `IServiceProvider.GetService<T>()`。

**示例**：

```rust
// MEDI 风格写法
match provider.get_service::<Logger>() {
    Some(logger) => { /* 使用 */ }
    None => { /* 服务未注册 */ }
}
```

---

### `get_required_service::<T>()` — 按类型解析（MEDI 命名）

```rust
pub fn get_required_service<T: ?Sized + Send + Sync + 'static>(&self) -> Arc<T>
```

**行为**：等价于 `self.get::<T>()`。MEDI 风格命名，对应 `IServiceProvider.GetRequiredService<T>()`。

---

### `get_optional::<T>()` — 按类型解析（安全版）

```rust
pub fn get_optional<T: ?Sized + Send + Sync + 'static>(&self) -> Option<Arc<T>>
```

**行为**：调用 `self.try_get::<T>()`，返回 `Option<Arc<T>>`。

**内部实现**：通过 `TypeId::of::<T>()` 查找存储，取第一个 `key.is_none()` 的条目，然后按生命周期处理缓存/工厂执行。

**示例**：

```rust
if let Some(logger) = provider.get_optional::<Logger>() {
    logger.log("hello");
}
```

---

### `get_keyed::<T>(key)` — 键控解析（panic 版）

```rust
pub fn get_keyed<T: ?Sized + Send + Sync + 'static>(&self, key: &str) -> Arc<T>
```

**行为**：调用 `self.try_get_keyed::<T>(key)`，若返回 `None` 则 panic，消息格式为 `"keyed service not registered: {type_name}:{key}"`。

**参数**：`key: &str` — 注册时传入的键名

**示例**：

```rust
let wechat = provider.get_keyed::<PaymentGateway>("wechat");
```

---

### `try_get_keyed::<T>(key)` — 键控解析（安全版）

内部方法，`get_keyed` 调用它。无需暴露出去。

> 注意：与 `IServiceResolver::try_get_keyed` 同名但不同实现路径。`ServiceProvider` 提供的是 `get_keyed`（public）和内部 `try_get_keyed`（private）。

---

### `get_all::<T>()` — 获取所有同类型实例

```rust
pub fn get_all<T: ?Sized + Send + Sync + 'static>(&self) -> Vec<Arc<T>>
```

**行为**：返回所有匹配 `TypeId::of::<T>()` 的实例（包含无键注册 + 所有键控注册）。若没有匹配项，返回空 `Vec`。

**使用场景**：策略模式、责任链、观察者等需要枚举所有注册服务的场景。

**示例**：

```rust
let all_gateways: Vec<Arc<PaymentGateway>> = provider.get_all::<PaymentGateway>();
for gw in &all_gateways {
    gw.charge(100);
}
```

---

### `get_services::<T>()` — 获取所有同类型实例（MEDI 命名）

```rust
pub fn get_services<T: ?Sized + Send + Sync + 'static>(&self) -> Vec<Arc<T>>
```

**行为**：等价于 `self.get_all::<T>()`。MEDI 风格命名，对应 `IServiceProvider.GetServices<T>()`。

---

### `create_scope()` — 创建作用域

```rust
pub fn scope(self: &Arc<Self>) -> Scope
pub fn create_scope(self: &Arc<Self>) -> Scope  // 别名
```

**参数**：`self: &Arc<Self>` — 要求 `ServiceProvider` 包装在 `Arc` 中

**行为**：创建新的 `Scope`，其内部持有 `parent: Arc<ServiceProvider>` 和空的 `scoped_cache`。

**示例**：

```rust
let provider = Arc::new(ServiceCollection::new()
    .scoped(|_| Arc::new(UnitOfWork { ... }))
    .build()
    .unwrap());

let scope = provider.create_scope();
let uow = scope.get::<UnitOfWork>();
```

---

### `get_named::<T>(name)` — 跨 DLL 命名解析（泛型）

```rust
pub fn get_named<T: Send + Sync + 'static>(&self, name: &str) -> Option<Arc<T>>
```

**行为**：从 `named` 注册表（`RwLock<HashMap<String, Arc<dyn Any + Send + Sync>>>`）中按名称查找，然后尝试 `downcast::<T>()` 转换。

**使用场景**：跨 DLL（cdylib 插件）服务访问。Rust 的 `TypeId` 在不同编译单元间不一致，因此通过字符串名称进行查找。

**示例**：

```rust
// 插件中注册
provider.register_named("plugin_output", Arc::new(OutputWriter::new()));

// 宿主中解析
if let Some(writer) = provider.get_named::<dyn Writer>("plugin_output") {
    writer.write("hello from plugin");
}
```

---

### `get_named_any(name)` — 跨 DLL 命名解析（非泛型）

```rust
pub fn get_named_any(&self, name: &str) -> Option<Arc<dyn Any + Send + Sync>>
```

**行为**：非泛型版本，返回 `Arc<dyn Any + Send + Sync>`，适合 trait 对象分发。

---

### `register_named(name, svc)` — 注册命名服务

```rust
pub fn register_named<T: Send + Sync + 'static>(&self, name: &str, service: Arc<T>)
```

**行为**：将 `service` 插入 `named` 注册表（`HashMap<String, Arc<dyn Any + Send + Sync>>`）。

---

### `remove_named(name)` — 移除命名服务

```rust
pub fn remove_named(&self, name: &str)
```

**行为**：从 `named` 注册表中移除指定名称的服务。用于插件卸载场景。

---

## Scope（作用域 API）

定义于 `rust_dicore::scope`，通过 `rust_dicore::Scope`（及别称 `rust_dicore::ServiceScope`）重新导出。

```rust
pub struct Scope {
    parent: Arc<ServiceProvider>,
    scoped_cache: LazyCache,
}
```

`Scope` 通过 `ServiceProvider::create_scope()` 创建。Singleton 委托父 `ServiceProvider` 解析；Transient 和 Scoped 在自身上下文中解析，确保 Transient 工厂内的 Scoped 依赖绑定到当前子 scope（而非回退到根 `root_scoped_cache`）。`scoped_cache` 使用 `LazyCache`（per-key `OnceLock`），多线程下工厂只执行一次。

### 生命周期处理逻辑

| 解析的 ServiceLifetime | Scope 行为 |
|------------------------|-----------|
| `Singleton` | 委托给 `self.parent.get_any_by_entry(entry)`，从父级 Singleton 缓存读取。因为 Singleton 在 `build()` 时已立即执行，父级缓存一定有值 |
| `Transient` | 执行 `(entry.factory)(self)`，每次创建新实例。用 `self`（Scope）解析依赖，Transient 内部的 Scoped 依赖绑定到当前子 scope |
| `Scoped` | 通过 `self.scoped_cache.get_or_init_with(entry.cache_key, factory)` 解析。`LazyCache` 的 per-key `OnceLock` 保证多线程下工厂只执行一次 |

### Scope 可用的公开方法

#### `get::<T>()` — 按类型解析（panic）

```rust
pub fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Arc<T>
```

行为与 `ServiceProvider::get` 一致：未找到则 panic。

#### `get_optional::<T>()` / `get_service::<T>()` — 安全解析

```rust
pub fn get_optional<T: ?Sized + Send + Sync + 'static>(&self) -> Option<Arc<T>>
pub fn get_service<T: ?Sized + Send + Sync + 'static>(&self) -> Option<Arc<T>>
```

`get_service` 是 `get_optional` 的 MEDI 别名。

#### `get_required_service::<T>()` — MEDI 解析（panic）

```rust
pub fn get_required_service<T: ?Sized + Send + Sync + 'static>(&self) -> Arc<T>
```

等价于 `self.get::<T>()`。

#### `get_keyed::<T>(key)` — 键控解析（panic）

```rust
pub fn get_keyed<T: ?Sized + Send + Sync + 'static>(&self, key: &str) -> Arc<T>
```

委托给父级 `ServiceProvider` 的条目存储，但按 Scoped/Singleton/Transient 生命周期分别缓存处理。

#### `get_all::<T>()` / `get_services::<T>()` — 获取全部

```rust
pub fn get_all<T: ?Sized + Send + Sync + 'static>(&self) -> Vec<Arc<T>>
pub fn get_services<T: ?Sized + Send + Sync + 'static>(&self) -> Vec<Arc<T>>
```

从 `self.parent.entries_by_tid(&tid)` 获取所有条目，逐条按生命周期处理并返回。

#### `get_named_any(name)` — 命名服务

```rust
pub fn get_named_any(&self, name: &str) -> Option<Arc<dyn Any + Send + Sync>>
```

直接委托给 `self.parent.get_named_any(name)`。

### Scope 的 IServiceLocator 支持

`Scope` 通过 `impl_service_locator!` 宏实现 `IServiceLocator` trait，同时暴露 `rdi_register_named` 和 `rdi_remove_named` 方法（均委托给父级 `ServiceProvider`）。

---

## ServiceProviderWrapper（分层容器 API）

定义于 `rust_dicore::wrapper`，通过 `rust_dicore::ServiceProviderWrapper` 重新导出。

```rust
pub struct ServiceProviderWrapper {
    child: Arc<ServiceProvider>,
    root: Arc<ServiceProvider>,
}
```

**核心语义**：child-first 查找，root-fallback。Child 覆盖 Root 的同类型服务；Child 中不存在的服务回退到 Root。

**使用场景**：插件子容器覆盖宿主服务、请求级别隔离、多租户等。

---

### `new(child, root)` — 构造函数

```rust
pub fn new(child: Arc<ServiceProvider>, root: Arc<ServiceProvider>) -> Arc<Self>
```

**参数**：

| 参数 | 说明 |
|------|------|
| `child` | 子容器的 `Arc<ServiceProvider>`，优先级更高 |
| `root` | 根容器的 `Arc<ServiceProvider>`，作为回退 |

**返回值**：`Arc<ServiceProviderWrapper>`

**副作用**：无

---

### `child()` / `root()` — 访问子/根容器

```rust
pub fn child(&self) -> &Arc<ServiceProvider>
pub fn root(&self) -> &Arc<ServiceProvider>
```

---

### `get::<T>()` — Child-first 解析（panic）

```rust
pub fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Arc<T>
```

**行为**：先查 `self.child.get_optional::<T>()`，命中则返回；否则查 `self.root.get_optional::<T>()`；都不存在则 panic。

**示例**：

```rust
use rust_dicore::{ServiceCollection, ServiceProviderWrapper};
use std::sync::Arc;

struct Logger { level: String }

let root = Arc::new(ServiceCollection::new()
    .singleton(|_| Arc::new(Logger { level: "root".into() }))
    .build().unwrap());

let child = ServiceCollection::new()
    .singleton(|_| Arc::new(Logger { level: "child".into() }))
    .build().unwrap();

let wrapper = ServiceProviderWrapper::new(Arc::new(child), root);
let logger = wrapper.get::<Logger>();
assert_eq!(logger.level, "child"); // child 优先
```

---

### `get_optional::<T>()` — Child-first 安全解析

```rust
pub fn get_optional<T: ?Sized + Send + Sync + 'static>(&self) -> Option<Arc<T>>
```

**行为**：`child.get_optional::<T>().or_else(|| root.get_optional::<T>())`

---

### `get_keyed::<T>(key)` — Child-first 键控解析（panic）

```rust
pub fn get_keyed<T: ?Sized + Send + Sync + 'static>(&self, key: &str) -> Arc<T>
```

**行为**：通过 `IServiceResolver::get_keyed_any` 依次查询 child 和 root，使用 `std::any::type_name::<T>()` 作为类型键。

---

### `get_all::<T>()` — 合并子+根结果

```rust
pub fn get_all<T: ?Sized + Send + Sync + 'static>(&self) -> Vec<Arc<T>>
```

**行为**：先取 `self.child.get_all::<T>()`，再 `extend` 追加 `self.root.get_all::<T>()`。**注意**：结果是**拼接**，不是覆盖。如果 child 和 root 都注册了同一键服务，两者都会出现在结果中。

---

### `get_named::<T>(name)` — 分层命名解析

```rust
pub fn get_named<T: Send + Sync + 'static>(&self, name: &str) -> Option<Arc<T>>
```

**行为**：`child.get_named::<T>(name).or_else(|| root.get_named::<T>(name))`

---

### `get_named_any(name)` — 分层命名解析（非泛型）

```rust
pub fn get_named_any(&self, name: &str) -> Option<Arc<dyn Any + Send + Sync>>
```

**行为**：`child.get_named_any(name).or_else(|| root.get_named_any(name))`

---

## IServiceResolver（核心 trait）

定义于 `rust_dicore::entry`，通过 `rust_dicore::IServiceResolver` 重新导出。

```rust
pub trait IServiceResolver: Send + Sync {
    fn get_any(&self, key: &str) -> Option<Arc<dyn Any + Send + Sync>>;
    fn get_keyed_any(&self, key: &str, variant: &str) -> Option<Arc<dyn Any + Send + Sync>>;

    // 以下为带默认实现的方法

    fn get<T: ?Sized + Sync + Send + 'static>(&self) -> Arc<T>
    where Self: Sized;

    fn try_get<T: ?Sized + Sync + Send + 'static>(&self) -> Option<Arc<T>>
    where Self: Sized;

    fn get_keyed<T: ?Sized + Sync + Send + 'static>(&self, variant: &str) -> Arc<T>
    where Self: Sized;

    fn try_get_keyed<T: ?Sized + Sync + Send + 'static>(&self, variant: &str) -> Option<Arc<T>>
    where Self: Sized;
}
```

### 必须实现的方法

#### `get_any(key)` — 类型擦除解析

```rust
fn get_any(&self, key: &str) -> Option<Arc<dyn Any + Send + Sync>>
```

通过 `type_name` 字符串查找服务。`key` 一般为 `std::any::type_name::<T>()` 的值。

- `ServiceProvider` 实现：通过 `type_map` 将 `key` 转为 `TypeId`，查找 `store`，返回第一个 `key.is_none()` 的条目
- `Scope` 实现：通过 `parent.entries_by_str(key)` 查找
- `ServiceProviderWrapper` 实现：child-first → root-fallback

#### `get_keyed_any(key, variant)` — 类型擦除键控解析

```rust
fn get_keyed_any(&self, key: &str, variant: &str) -> Option<Arc<dyn Any + Send + Sync>>
```

通过 `type_name` + 键名查找。`key` 为类型名，`variant` 为键名。

### 默认实现的方法

#### `get::<T>()`

```rust
fn get<T: ?Sized + Sync + Send + 'static>(&self) -> Arc<T>
where Self: Sized,
```

**实现**：

1. 调用 `self.get_any(std::any::type_name::<T>())`
2. `downcast::<Arc<T>>()` 解包双重 Arc 包装
3. 返回 `Arc::clone(&*downcast_result)`
4. 失败则 panic

#### `try_get::<T>()`

```rust
fn try_get<T: ?Sized + Sync + Send + 'static>(&self) -> Option<Arc<T>>
where Self: Sized,
```

同 `get` 但返回 `Option`，不 panic。

#### `get_keyed::<T>(variant)`

```rust
fn get_keyed<T: ?Sized + Sync + Send + 'static>(&self, variant: &str) -> Arc<T>
where Self: Sized,
```

1. 调用 `self.get_keyed_any(std::any::type_name::<T>(), variant)`
2. downcast 解包
3. 失败则 panic

#### `try_get_keyed::<T>(variant)`

```rust
fn try_get_keyed<T: ?Sized + Sync + Send + 'static>(&self, variant: &str) -> Option<Arc<T>>
where Self: Sized,
```

同 `get_keyed` 但返回 `Option`。

### 实现者清单

| 类型 | 实现方式 |
|------|---------|
| `ServiceProvider` | 直接 `impl IServiceResolver` |
| `Scope` | 直接 `impl IServiceResolver` |
| `ServiceProviderWrapper` | 直接 `impl IServiceResolver` |
| `RdiProvider` | 直接 `impl IServiceResolver`（枚举分发） |

---

## IServiceLocator / ServiceLocatorBridge / RdiProvider

### IServiceLocator trait

定义于 `rust_dicore::service_locator`。

```rust
pub trait IServiceLocator: Send + Sync {
    fn get_any(&self, type_key: &str) -> Option<Arc<dyn Any + Send + Sync>>;
    fn get_any_named(&self, name: &str) -> Option<Arc<dyn Any + Send + Sync>>;
    fn register_named_any(&self, name: &str, service: Arc<dyn Any + Send + Sync>);
    fn remove_named(&self, name: &str);
}
```

**与 `IServiceResolver` 的关系**：

- `IServiceResolver`：内部核心 trait，类型系统（TypeId）+ 字符串混合查找，有泛型默认方法
- `IServiceLocator`：外部集成 trait，纯字符串查找。用于跨 DLL 插件等场景

`IServiceLocator` 通过 `impl_service_locator!` 宏自动为 `ServiceProvider`、`Scope`、`ServiceProviderWrapper` 实现。

### INamedRegistrar trait

```rust
pub trait INamedRegistrar: Send + Sync {
    fn register_named_any(&self, name: &str, service: Arc<dyn Any + Send + Sync>);
    fn remove_named(&self, name: &str);
}
```

单独抽取注册操作，用于需要可变性的场景。

---

### RdiProvider 枚举

定义于 `rust_dicore::bridge`。

```rust
pub enum RdiProvider {
    Root(Arc<ServiceProvider>),
    Wrapped(Arc<ServiceProviderWrapper>),
}
```

**作用**：统一 `ServiceProvider` 和 `ServiceProviderWrapper`，使两者可互换地置于 `IServiceResolver` 后方。

**实现 `IServiceResolver`**：枚举分发到对应内部类型。

**自身方法**：

```rust
impl RdiProvider {
    pub fn get_named_any(&self, name: &str) -> Option<Arc<dyn Any + Send + Sync>>;
    pub fn register_named_any(&self, name: &str, service: Arc<dyn Any + Send + Sync>);
    pub fn remove_named(&self, name: &str);
}
```

**示例**：

```rust
use rust_dicore::{RdiProvider, ServiceProvider};
use std::sync::Arc;

let provider = Arc::new(/* ServiceProvider 或 ServiceProviderWrapper */);
let rdi = RdiProvider::Root(provider);
// 可通过 rdi 统一使用 IServiceResolver 接口
```

---

### ServiceLocatorBridge

定义于 `rust_dicore::bridge`。

```rust
pub struct ServiceLocatorBridge {
    provider: Arc<RdiProvider>,
}
```

**作用**：将 RDI 的 `RdiProvider`（编译期注册服务 + 命名服务注册表）桥接到 `IServiceLocator` trait。

**构造函数**：

```rust
impl ServiceLocatorBridge {
    pub fn new(provider: Arc<RdiProvider>) -> Self;
    pub fn provider(&self) -> &Arc<RdiProvider>;
}
```

**解析顺序**：

1. `get_any(type_key)`：委托给 `self.provider.get_any(type_key)` → RDI 编译期注册服务
2. `get_any_named(name)`：委托给 `self.provider.get_named_any(name)` → 命名服务注册表
3. `register_named_any` / `remove_named`：写入/删除命名注册表

**实现 `INamedRegistrar`**：将 `register_named_any` / `remove_named` 委托给 `self.provider`。

**使用场景**：向外部 crate 或插件系统暴露统一的 `IServiceLocator` 接口。

**示例**：

```rust
use rust_dicore::{ServiceCollection, ServiceLocatorBridge, RdiProvider};
use std::sync::Arc;

let provider = Arc::new(
    ServiceCollection::new()
        .singleton(|_| Arc::new(42i32))
        .build()
        .unwrap(),
);

let rdi = Arc::new(RdiProvider::Root(provider));
let bridge = ServiceLocatorBridge::new(rdi);

// 作为 IServiceLocator 传递给插件系统
fn install_plugin(loc: &dyn IServiceLocator) {
    // ...
}
```

---

### impl_service_locator! 宏

```rust
macro_rules! impl_service_locator {
    ($ty:ty) => {
        impl IServiceLocator for $ty {
            fn get_any(&self, type_key: &str) -> Option<Arc<dyn Any + Send + Sync>> {
                IServiceResolver::get_any(self, type_key)
            }
            fn get_any_named(&self, name: &str) -> Option<Arc<dyn Any + Send + Sync>> {
                self.get_named_any(name)
            }
            fn register_named_any(&self, name: &str, service: Arc<dyn Any + Send + Sync>) {
                self.rdi_register_named(name, service);
            }
            fn remove_named(&self, name: &str) {
                self.rdi_remove_named(name);
            }
        }
    };
}
```

**自动实现**：`ServiceProvider`、`Scope`、`ServiceProviderWrapper` 均通过此宏获得 `IServiceLocator` 实现。

---

## RdiError 错误类型

定义于 `rust_dicore::error`，通过 `rust_dicore::RdiError` 重新导出。

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RdiError {
    #[error("service not registered: {0}")]
    ServiceNotFound(&'static str),

    #[error("keyed service not registered: key={key}, type={ty}")]
    KeyedServiceNotFound { key: String, ty: &'static str },
}
```

| 变体 | 说明 |
|------|------|
| `ServiceNotFound(&'static str)` | 服务未注册，值为 `type_name` |
| `KeyedServiceNotFound { key: String, ty: &'static str }` | 键控服务未找到，携带键名和类型名 |

> **注意**：当前实现中 `build()` 不会返回错误。`RdiError` 设计为预留扩展，`get()` 等方法选择 panic 而非返回 `Result`。`RdiError` 可用于自定义的安全解析封装。

---

## 内部数据结构速查

以下为框架内部类型，不直接暴露给使用者，但有助于理解行为。

### ServiceDescriptor

```rust
pub struct ServiceDescriptor {
    pub type_id: TypeId,
    pub type_name: &'static str,
    pub key: Option<String>,
    pub factory: ServiceFactory,
    pub lifetime: ServiceLifetime,
}
```

注册阶段的内部表示。`build()` 时转换为 `ServiceEntry`。

### ServiceEntry

```rust
pub struct ServiceEntry {
    pub cache_key: usize,
    pub key: Option<String>,
    pub type_name: &'static str,
    pub factory: ServiceFactory,
    pub lifetime: ServiceLifetime,
}
```

解析阶段的内部表示。`cache_key` 用于缓存索引（按注册顺序分配）。

### ServiceFactory

```rust
pub type ServiceFactory =
    Arc<dyn Fn(&dyn IServiceResolver) -> Arc<dyn Any + Send + Sync> + Send + Sync>;
```

工厂闭包类型别名。接收 `&dyn IServiceResolver`，返回 `Arc<dyn Any + Send + Sync>`（类型擦除）。

### ServiceStore

```rust
pub type ServiceStore = HashMap<TypeId, Vec<ServiceEntry>>;
```

按 `TypeId` 分组存储所有服务条目。

---

## 双重 Arc 包装说明

LRDI 的内部门道：工厂闭包的返回值经过包装。

```
注册时：
    ServiceCollection::singleton(|r| Arc::new(MyService))
    内部 push(): Arc::new(move |r| Arc::new(Arc::new(MyService)))
                            ^^^^^^^^  ^^^^^^^^
                            外层 Arc<dyn Any>  内层 Arc<T>

解析时：
    get_any_by_entry() → Arc<dyn Any + Send + Sync>   // 外层 Arc
    extract()          → downcast::<Arc<T>>()         // 取出内层 Arc<T>
```

这种设计使类型擦除（`Arc<dyn Any>`）成为可能，同时保持 `Arc<T>` 的引用计数语义。

---

> **文档版本**：基于 rust-di 源代码生成，与 `e:\GitCode\RF\rust-di\crates\rust-dicore\src\` 内核源代码一一对应。
