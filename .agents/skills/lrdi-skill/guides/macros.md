# rust-dicore 过程宏深度指南

rust-dicore（原 lrdi）的过程宏近期经历了一次重构：crate 名称从 `lrdi` 改为 `rust-dicore`，属性宏 `inject_attr` 改名为 `inject`，函数式宏 `inject!` 改名为 `register!`，并移除了 `as = dyn Trait` 语法，改为通过在 `impl Trait for T` 块上放置 `#[inject]` 来实现接口注册。

本指南面向新 API 编写。所有示例使用 `rust_dicore::` 导入路径与 `#[inject(...)]` 字段属性。

> **导入约定**：本文示例为明确宏来源使用全限定路径（`#[rust_dicore::inject]` 等）。实际编码推荐 `use rust_dicore::*;` 后用 `#[inject]` 等简短形式，详见 `guides/usage-guide.md`。

---

## 一、宏全景：三种放置位置

rust-dicore 的过程宏体系围绕三个**正交**的放置位置展开，理解它们的关系是掌握本指南的前提：

| 放置位置 | 宏 | 生成内容 | 注册类型 |
|---------|-----|---------|---------|
| **struct 上派生** | `#[derive(Inject)]` | 仅生成构造函数 `__rdi_construct_{Type}` | 不注册 |
| **struct 上属性** | `#[rust_dicore::inject]` | 生成构造函数 + 工厂 + `type_name` 辅助函数 + inventory 注册 | 注册为**具体类型** |
| **`impl Trait for T` 上属性** | `#[rust_dicore::inject]` | 生成工厂 + `type_name` 辅助函数 + inventory 注册（复用 struct 上生成的构造函数） | 注册为 **`dyn Trait`** |

三种方式可以自由组合：

- 只想生成构造函数、由其他方式负责注册 → 用 `#[derive(Inject)]`
- 一个具体服务要直接对外暴露 → 在 struct 上用 `#[rust_dicore::inject]`
- 要面向接口编程（注册为 `dyn Trait`）→ 先在 struct 上用 `#[derive(Inject)]` 或 `#[rust_dicore::inject]` 生成构造函数，再在 `impl Trait for T` 块上用 `#[rust_dicore::inject]` 完成接口注册

> **迁移说明（旧 API → 新 API）**
> - `lrdi::` → `rust_dicore::`；`lrdi-macros` → `rust-dicore-macros`
> - `#[lrdi::inject_attr(...)]` → `#[rust_dicore::inject(...)]`（属性宏改名）
> - `lrdi::inject!(...)` → `rust_dicore::register!(...)`（函数式宏改名，让出 `inject` 名字给属性宏——Rust 不允许同一 crate 内属性宏与函数式宏同名）
> - 字段属性统一为 `#[inject(...)]`（派生宏声明为 `#[proc_macro_derive(Inject, attributes(inject))]`）。**显式注入**：只有标记了 `#[inject]` / `#[inject(owned)]` 的字段才从容器解析，未标记字段用 `Default::default()`。可选性由字段类型 `Option<...>` 决定，不再有 `skip`/`optional` 标记。
> - **`as = dyn Trait` 与 `as = [dyn A, dyn B]` 语法已移除**。`InjectArgs` 不再有 `Plain`/`AsTrait`/`AsTraits` 变体。接口注册改用「在 `impl Trait for T` 上放 `#[inject]`」的方式。

---

## 二、`#[rust_dicore::inject]` 属性宏完整指南（推荐方式）

`#[rust_dicore::inject]` 是 rust-dicore **推荐的首选方式**。它是一个属性宏（attribute macro），可以放在两种位置上：

1. **放在 struct 上**：生成构造函数，并将服务以**具体类型**注册到全局 `inventory`
2. **放在 `impl Trait for T` 块上**：从 impl 块**自动检测** trait 类型，将服务以 `dyn Trait` 注册，复用 struct 侧生成的构造函数

两种位置共用同一套属性参数语法：

```rust
#[rust_dicore::inject]              // 生命周期默认 singleton
#[rust_dicore::inject(singleton)]
#[rust_dicore::inject(scoped)]
#[rust_dicore::inject(transient)]
```

---

### 2.1 两个放置位置：struct 与 impl 块

#### 位置一：struct 上——注册为具体类型

```rust
use rust_dicore::*;
use std::sync::Arc;

// 一行属性即完成：构造函数生成 + 以 MyService 类型注册到 inventory
#[rust_dicore::inject(singleton)]
struct MyService {
    logger: Arc<Logger>,
}

let provider = ServiceCollection::from_injected().build()?;
let svc: Arc<MyService> = provider.get::<MyService>();
```

#### 位置二：impl 块上——注册为 `dyn Trait`

接口注册需要两步：先让 struct 拥有构造函数（**推荐用 `#[derive(Inject)]`**——仅生成构造函数、不注册；若用 struct 上的 `#[rust_dicore::inject]` 会同时把具体类型也注册进 inventory，导致双重注册），再在 `impl Trait for T` 块上放 `#[rust_dicore::inject]` 完成接口注册。

```rust
use rust_dicore::*;
use std::sync::Arc;

trait IPlugin: Send + Sync {
    fn name(&self) -> &'static str;
}

// 第一步：struct 上用 #[derive(Inject)] 生成构造函数（不注册）
#[derive(Inject)]
struct TestPlugin {
    name: &'static str,  // 未标记 → Default::default()
}

impl IPlugin for TestPlugin {
    fn name(&self) -> &'static str { self.name }
}

// 第二步：在 impl 块上放 #[inject]，自动以 dyn IPlugin 注册
#[rust_dicore::inject(singleton)]
impl IPlugin for TestPlugin {}

let provider = ServiceCollection::from_injected().build()?;
let plugin: Arc<dyn IPlugin> = provider.get::<dyn IPlugin>();
```

**关键点：**

- 宏从 `impl Trait for Type` 语法中**自动提取** `Trait`，无需在属性里写 `as = dyn Trait`
- impl 块上生成的工厂函数名为 `__rdi_factory_{Type}_for_{Trait}`，按「类型-trait 对」唯一命名，因此**同一个 struct 实现多个 trait 时可以分别注册**而不会冲突
- 工厂内部调用 `__rdi_construct_{Type}`——这个构造函数必须由 struct 侧的 `#[derive(Inject)]` 或 struct 上的 `#[rust_dicore::inject]` 提供
- 不支持 negative impl（`impl !Trait for T`），会编译报错

#### 放置位置二选一（避免双重注册）

`#[inject]` 放 struct 上 = 注册为**具体类型**；放 impl 块上 = 注册为 **`dyn Trait`**。两者同时用 = 同一 struct 被注册两次（具体类型 + trait），通常非预期——消费者能绕过 trait 直接 `get::<具体类型>()`，破坏面向接口原则。

**核心区分**：`#[derive(Inject)]` 与 `#[inject]` 都能在 struct 侧生成构造函数，但只有 `#[inject]` 会向 inventory 提交注册。因此面向接口时 struct 上应选 `#[derive(Inject)]`（仅构造、不注册）。

| 目标注册身份 | struct 上 | impl 上 | 结果 |
|------------|----------|---------|------|
| 仅具体类型（无 trait） | `#[inject]` | *不放* | ✅ 单一注册 |
| 仅 `dyn Trait`（**推荐**） | `#[derive(Inject)]` | `#[inject]` | ✅ 单一注册 |
| 两者都要（罕见） | `#[inject]` | `#[inject]` | ⚠️ 双重注册，需注释说明 |
| 仅 impl、struct 无构造 | *不放* | `#[inject]` | ❌ 编译失败 |

```rust
// ✅ Handler 面向接口：struct 用 #[derive(Inject)] 仅生成构造函数，impl 用 #[inject] 注册为 trait
#[derive(Inject)]
struct UserHandler {
    #[inject]
    user_svc: Arc<dyn IUserService>,
}

#[inject]  // 仅注册为 dyn IUserHandler
impl IUserHandler for UserHandler {
    fn handle(&self, req: Req) -> Resp { /* ... */ }
}

// ❌ 双重注册：struct 和 impl 都用 #[inject] → 容器里既有 UserHandler 又有 dyn IUserHandler
#[inject(transient)]
struct UserHandler { /* ... */ }

#[inject]
impl IUserHandler for UserHandler { /* ... */ }  // 消费者可 get::<UserHandler>() 绕过接口

// ✅ 无 trait 契约的简单服务：只在 struct 上用 #[inject]
#[inject(singleton)]
struct Config { port: u16 }
```

**何时放 struct 上**：服务作为具体类型对外暴露（无 trait，或消费者直接用具体类型，如配置、值对象）。
**何时放 impl 上**：服务有 trait 契约，面向接口便于 mock 替换（Handler / Service / Repository 均属此类，**推荐**）。**Handler 推荐放 impl 上**，struct 上用 `#[derive(Inject)]`。

---

### 2.2 宏展开原理：struct 放置

`#[rust_dicore::inject]` 的入口是 `inject` 函数（源码：`crates/macros/src/lib.rs`）。它把属性参数解析为 `InjectArgs`，把 item 解析为 `syn::Item`，再交给 `expand_inject` 分发：

```rust
fn expand_inject(args: InjectArgs, item: syn::Item) -> syn::Result<proc_macro2::TokenStream> {
    let lifetime = args.lifetime.unwrap_or(LT::S);  // 默认 singleton
    match &item {
        syn::Item::Struct(s) => expand_inject_struct(s, lifetime),
        syn::Item::Impl(i)  => expand_inject_impl(i, lifetime),
        _ => Err(syn::Error::new_spanned(
            &item,
            "#[inject] can only be placed on a struct or a trait impl block",
        )),
    }
}
```

**`InjectArgs` 结构体**（重构后只剩一个字段，旧版的 `Plain`/`AsTrait`/`AsTraits` 变体已移除）：

```rust
struct InjectArgs {
    lifetime: Option<LT>,
}

impl Parse for InjectArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.is_empty() {
            return Ok(InjectArgs { lifetime: None });  // 默认 singleton
        }
        let lt: LT = input.parse()?;
        Ok(InjectArgs { lifetime: Some(lt) })
    }
}
```

struct 放置的展开由 `expand_inject_struct` 完成，分为五个阶段：

**阶段 1：解析属性参数**

解析 `InjectArgs`，缺省时生命周期为 `LT::S`（singleton）。

**阶段 2：解析结构体字段**

遍历 `Fields::Named`（也支持 `Fields::Unit`），对每个字段调用 `parse_ia`，扫描字段上的 `#[inject(...)]` 属性填充 `IA` 结构体（详见 §3.3）。

**阶段 3：生成构造器函数**

生成 `__rdi_construct_{Name}`（与 `#[derive(Inject)]` 完全相同的构造逻辑）：

```rust
#[doc(hidden)]
#[allow(non_snake_case)]
pub fn __rdi_construct_{Name}(resolver: &dyn rust_dicore::IServiceResolver)
    -> ::std::sync::Arc<{Name}>
{
    ::std::sync::Arc::new({Name} { /* 字段初始化表达式 */ })
}
```

**阶段 4：生成工厂函数与 `type_name` 辅助函数**

工厂把 `Arc<{Name}>` 二次包装为 `Arc<dyn Any + Send + Sync>` 以兼容 `ServiceRegistration`：

```rust
#[doc(hidden)]
#[allow(non_snake_case)]
fn __rdi_factory_{Name}(resolver: &dyn rust_dicore::IServiceResolver)
    -> ::std::sync::Arc<dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync>
{
    let v: ::std::sync::Arc<{Name}> = __rdi_construct_{Name}(resolver);
    ::std::sync::Arc::new(v)
        as ::std::sync::Arc<dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync>
}

#[doc(hidden)]
#[allow(non_snake_case)]
fn __rdi_type_name_{Name}() -> &'static str {
    ::std::any::type_name::<{Name}>()
}
```

> **注意：** `ServiceRegistration.type_name_fn` 是一个**函数指针** `fn() -> &'static str`，而不是 `&'static str` 字符串。这样做是为了规避 `std::any::type_name::<T>()` 在常量上下文不可用的问题（tracking issue #63084）。`from_injected()` 在运行时调用 `(reg.type_name_fn)()` 取得字符串。

**阶段 5：生成 inventory 注册代码**

```rust
rust_dicore::inventory::submit! {
    rust_dicore::ServiceRegistration {
        lifetime: rust_dicore::ServiceLifetime::Singleton,  // 根据参数确定
        type_id: ::std::any::TypeId::of::<{Name}>(),
        type_name_fn: __rdi_type_name_{Name},
        factory: __rdi_factory_{Name},
    }
}
```

**最后：剥离字段上的 `#[inject(...)]` 属性后输出 struct。**

由于没有 `#[derive(Inject)]` 时编译器不认识 `inject` 这个 helper attribute，`expand_inject_struct` 在输出 struct 前会 `retain` 掉字段上所有 `path` 为 `inject` 的属性，避免编译错误：

```rust
let mut stripped = struct_item.clone();
if let syn::Fields::Named(n) = &mut stripped.fields {
    for field in n.named.iter_mut() {
        field.attrs.retain(|a| !a.path().is_ident("inject"));
    }
}
```

最终输出：剥离属性后的 struct + 构造器函数 + `type_name` 辅助函数 + 工厂函数 + inventory 注册代码。

---

### 2.3 宏展开原理：impl 块放置（自动 trait 检测）

impl 块放置由 `expand_inject_impl` 完成。其核心是**从 `impl_item.trait_` 字段中自动提取 trait 路径**。

**阶段 1：校验是 trait impl**

`ItemImpl.trait_` 是 `Option<(Option<Bang>, Path, For)>`。若为 `None`（即裸 `impl Type {}` 而非 `impl Trait for Type`），报错：

```rust
let (not, trait_path, _) = impl_item.trait_.as_ref().ok_or_else(|| {
    syn::Error::new_spanned(
        impl_item,
        "#[inject] on an impl block requires a trait impl: use `impl Trait for Type`",
    )
})?;
if not.is_some() {
    return Err(syn::Error::new_spanned(
        impl_item,
        "#[inject] does not support negative impls",
    ));
}
```

**阶段 2：提取类型标识与 trait 后缀**

```rust
let impl_ty = &impl_item.self_ty;
let type_ident = match &**impl_ty {
    syn::Type::Path(tp) => tp.path.segments.last().map(|s| s.ident.clone()),
    _ => None,
}.ok_or_else(|| syn::Error::new_spanned(impl_ty, "expected a path type"))?;

let trait_segment = trait_path.segments.last().unwrap();
let trait_suffix = &trait_segment.ident;
```

由此得到三个唯一命名：

```rust
let constructor_fn      = format_ident!("__rdi_construct_{}", type_ident);
let factory_name        = format_ident!("__rdi_factory_{}_for_{}", type_ident, trait_suffix);
let type_name_fn_name   = format_ident!("__rdi_type_name_{}_for_{}", type_ident, trait_suffix);
```

按「类型-trait 对」唯一，因此同一 struct 实现多个 trait 时不会冲突。

**阶段 3：生成工厂函数**

工厂调用 struct 侧生成的 `__rdi_construct_{Type}`，把 `Arc<Type>` 协变为 `Arc<dyn Trait>`，再包装为 `Arc<dyn Any + Send + Sync>`：

```rust
let trait_ty = quote! { dyn #trait_path };

fn __rdi_factory_{Type}_for_{Trait}(resolver: &dyn rust_dicore::IServiceResolver)
    -> ::std::sync::Arc<dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync>
{
    let v:  ::std::sync::Arc<#impl_ty>   = #constructor_fn(resolver);
    let v2: ::std::sync::Arc<#trait_ty>  = v;   // Arc<Type> → Arc<dyn Trait>
    ::std::sync::Arc::new(v2)
        as ::std::sync::Arc<dyn ::std::any::Any + ::std::marker::Send + ::std::marker::Sync>
}
```

**阶段 4：生成 `type_name` 辅助函数与 inventory 注册**

```rust
fn __rdi_type_name_{Type}_for_{Trait}() -> &'static str {
    ::std::any::type_name::<dyn #trait_path>()
}

rust_dicore::inventory::submit! {
    rust_dicore::ServiceRegistration {
        lifetime: rust_dicore::ServiceLifetime::Singleton,
        type_id: ::std::any::TypeId::of::<dyn #trait_path>(),  // ← 以 dyn Trait 注册
        type_name_fn: __rdi_type_name_{Type}_for_{Trait},
        factory: __rdi_factory_{Type}_for_{Trait},
    }
}
```

**关键点：** `type_id` 是 `TypeId::of::<dyn #trait_path>()`，因此 `provider.get::<dyn IPlugin>()` 能命中。这是「注册类型 = 解析类型」规则的体现。

最终输出：原 impl 块（原样保留）+ `type_name` 辅助函数 + 工厂函数 + inventory 注册代码。

---

### 2.4 语法完整参考

#### 语法 1：struct 上的普通注册

```rust
#[rust_dicore::inject(singleton)]
struct MyService {
    #[inject]
    logger: Arc<Logger>,
    label: String,  // 未标记 → Default::default()
}
```

生命周期可选值：`singleton`、`scoped`、`transient`。省略时默认 `singleton`。

等价于手动编写：

```rust
#[derive(Inject)]
struct MyService { /* ... */ }
// + 手动注册：
// ServiceCollection::new().singleton(|r| __rdi_construct_MyService(r))
```

#### 语法 2：impl 块上的接口注册

```rust
trait IPlugin: Send + Sync { fn name(&self) -> &'static str; }

#[derive(Inject)]
struct TestPlugin { name: &'static str }  // 未标记 → Default::default()

impl IPlugin for TestPlugin {
    fn name(&self) -> &'static str { self.name }
}

#[rust_dicore::inject(singleton)]
impl IPlugin for TestPlugin {}
```

impl 块的函数体可以非空——宏会把原 impl 块原样输出，只在其后追加工厂与注册代码。

#### 语法 3：同一 struct 注册为多个 trait

由于工厂按「类型-trait 对」唯一命名，可以分别在每个 impl 块上放 `#[inject]`：

```rust
trait IPlugin: Send + Sync { fn name(&self) -> &'static str; }
trait ILogger: Send + Sync { fn log(&self, msg: &str); }

#[derive(Inject)]
struct DualService;

impl IPlugin for DualService { fn name(&self) -> &'static str { "dual" } }
impl ILogger for DualService { fn log(&self, msg: &str) { println!("{msg}"); } }

#[rust_dicore::inject(singleton)]
impl IPlugin for DualService {}

#[rust_dicore::inject(singleton)]
impl ILogger for DualService {}
```

生成 `__rdi_factory_DualService_for_IPlugin` 与 `__rdi_factory_DualService_for_ILogger` 两个独立注册，两者复用同一个 `__rdi_construct_DualService` 构造函数。

#### 语法 4：字段 `#[inject(...)]` 属性

struct 上的 `#[rust_dicore::inject]` 与 `#[derive(Inject)]` 共用同一套字段属性，属性名为 `inject`。
**显式注入策略**：只有标记了 `#[inject(...)]` 的字段才从容器解析，未标记字段使用 `Default::default()`。
可选性由字段类型（`Option<...>`）决定，无需 `optional` 标记。

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
| `#[inject(provider)]` | `Arc<dyn IServiceResolver>` | 直接注入 resolver |

> 严格类型校验：`#[inject]` 只接受 `Arc<T>` / `Option<Arc<T>>`，
> `#[inject(owned)]` 只接受裸 `T` / `Option<T>`。标记与类型不匹配会触发编译错误。

详见 §3.3。

---

### 2.5 `ServiceCollection::from_injected()` 收集注册

`ServiceCollection::from_injected()` 是联结 `#[rust_dicore::inject]` 属性和 `ServiceCollection` 的关键 API。它遍历 `inventory` 中所有 `ServiceRegistration`，在运行时调用 `type_name_fn` 取得类型名，组装为 `ServiceCollection`：

```rust
pub fn from_injected() -> Self {
    let mut descriptors = Vec::new();
    for reg in inventory::iter::<ServiceRegistration> {
        let factory: ServiceFactory = Arc::new(move |r| (reg.factory)(r));
        descriptors.push(ServiceDescriptor {
            type_id: reg.type_id,
            type_name: (reg.type_name_fn)(),  // ← 运行时调用函数指针
            key: None,
            factory,
            lifetime: reg.lifetime,
        });
    }
    Self { descriptors }
}
```

`ServiceRegistration` 的定义（`crates/core/src/registration.rs`）：

```rust
pub struct ServiceRegistration {
    pub lifetime: ServiceLifetime,
    pub type_id: TypeId,
    pub type_name_fn: fn() -> &'static str,  // 函数指针，非字符串
    pub factory: fn(&dyn IServiceResolver) -> Arc<dyn Any + Send + Sync>,
}

inventory::collect!(ServiceRegistration);
```

**使用方式：**

```rust
let provider = ServiceCollection::from_injected()
    // 可额外手动注册（覆盖或补充）
    .singleton(|_| Arc::new(ExtraService::default()))
    .build()?;
```

**注意事项：**

- `from_injected()` 收集所有使用了 `#[rust_dicore::inject]` 的注册（struct 放置与 impl 放置都会生成 `submit!`），**跨 crate 有效**（依赖 inventory 的全局注册机制）
- 返回的 `ServiceCollection` 可以继续链式调用其他注册方法
- 若同时使用 `from_injected()` 和手动注册相同类型，后者会覆盖前者
- 要求依赖 rust-dicore 的默认特性（启用 `inventory`）

---

### 2.6 实战：完整示例

```rust
use rust_dicore::*;
use std::sync::Arc;

// ── 普通注册（struct 上）──
#[rust_dicore::inject(singleton)]
struct Logger {
    prefix: String,  // 未标记 → Default::default()
}

impl Default for Logger {
    fn default() -> Self { Logger { prefix: "app".into() } }
}

// ── 接口注册（impl 块上，自动检测 trait）──
trait IPlugin: Send + Sync {
    fn name(&self) -> &'static str;
}

#[derive(Inject)]
struct TestPlugin {
    name: &'static str,  // 未标记 → Default::default()
}

impl IPlugin for TestPlugin {
    fn name(&self) -> &'static str { self.name }
}

#[rust_dicore::inject(singleton)]
impl IPlugin for TestPlugin {}

impl Default for TestPlugin {
    fn default() -> Self { TestPlugin { name: "test" } }
}

// ── 多 trait 注册 ──
trait ILogger: Send + Sync {
    fn log(&self, msg: &str);
}

#[derive(Inject)]
struct DualService;

impl IPlugin for DualService { fn name(&self) -> &'static str { "dual" } }
impl ILogger for DualService { fn log(&self, msg: &str) { println!("{msg}"); } }

#[rust_dicore::inject(singleton)]
impl IPlugin for DualService {}

#[rust_dicore::inject(singleton)]
impl ILogger for DualService {}

// ── 字段注入完整示例 ──
#[rust_dicore::inject(singleton)]
struct OrderService {
    #[inject]
    logger: Arc<Logger>,
    #[inject]
    notifier: Option<Arc<Logger>>,
    #[inject(key = "audit")]
    audit: Arc<Logger>,
    label: String,  // 未标记 → Default::default()
    #[inject(provider)]
    resolver: Arc<dyn IServiceResolver>,
}

// ── 构建 provider ──
fn main() -> Result<(), RdiError> {
    let provider = ServiceCollection::from_injected()
        .keyed("audit", |_| Arc::new(Logger { prefix: "audit".into() }))
        .build()?;

    let svc: Arc<OrderService> = provider.get::<OrderService>();
    let plugin: Arc<dyn IPlugin> = provider.get::<dyn IPlugin>();
    println!("Logger prefix: {}", svc.logger.prefix);
    println!("Plugin name: {}", plugin.name());
    Ok(())
}
```

---

### 2.7 与 `#[derive(Inject)]` 的关系

| 对比维度 | `#[rust_dicore::inject]`（struct） | `#[derive(Inject)]` |
|---------|-----------------------------------|---------------------|
| **宏类型** | 属性宏 (attribute macro) | 派生宏 (derive macro) |
| **构造函数生成** | ✅ 自动生成 | ✅ 自动生成 |
| **服务注册** | ✅ `inventory::submit!` 自动注册（具体类型） | ❌ 需要手动注册 |
| **接口注册** | 在 `impl Trait for T` 上再放一次 `#[inject]` | 同左（共用构造函数） |
| **需额外代码** | 无 | 需手写 `.singleton(\|r\| __rdi_construct_X(r))` 或在 impl 块上放 `#[inject]` |
| **跨 crate 收集** | ✅ `from_injected()` 自动收集 | ❌ 需手动组合 |
| **推荐度** | ⭐ **首选** | 仅用于只需构造函数、不需注册的场景 |

**结论：** 对于绝大多数场景，推荐直接使用 `#[rust_dicore::inject(...)]`。仅在「只想生成构造函数、由 `#[module]` 或手动 `ServiceCollection` 负责注册」时才使用 `#[derive(Inject)]`。

---

### 2.8 编译错误排查

| 错误信息 | 原因 | 解决方法 |
|---------|------|---------|
| `"#[inject] can only be placed on a struct or a trait impl block"` | 把 `#[inject]` 放在 enum/union/fn 等位置 | 仅放在 struct 或 `impl Trait for T` 块上 |
| `"named struct or unit struct required"` | struct 不是命名字段或单元结构体（如元组结构体） | 改为命名字段或单元结构体 |
| `"#[inject] on an impl block requires a trait impl: use \`impl Trait for Type\`"` | impl 块不是 trait impl（裸 `impl Type {}`） | 改写为 `impl Trait for Type` |
| `"#[inject] does not support negative impls"` | 用在 `impl !Trait for T` 上 | 移除属性，不支持 negative impl |
| `"expected a path type"` | impl 的 self type 不是路径类型 | 确保 `for` 后是简单路径类型 |
| `unknown lifetime: ...` | 生命周期参数拼写错误 | 使用 `singleton`、`scoped` 或 `transient` |
| `#[inject]` 属性解析失败 | 字段属性拼写错误，如 `#[inject(keyed = "...")]` | 仅支持 `owned`、`provider`、`key` |
| `Default` 缺失 | 未标记字段或 impl 块复用的构造函数要求 `Default` | 为该类型实现 `Default` |
| `#[inject] requires Arc<T> or Option<Arc<T>>` | `#[inject]` 标在了裸 `T` 字段上 | 改用 `#[inject(owned)]` |
| `#[inject(owned)] requires bare T or Option<T>` | `#[inject(owned)]` 标在了 `Arc<T>` 字段上 | 改用 `#[inject]` |
| 运行时 panic `"svc not registered"` | 依赖未注册 | 检查 `from_injected()` 或手动注册 |
| 运行时 panic `"keyed not found"` | 键控依赖未注册 | 确保键名拼写匹配 |

---

## 三、`#[derive(Inject)]` 完整指南

> **说明：** `#[derive(Inject)]` 是 rust-dicore 的底层派生宏。它**仅生成构造函数**（`__rdi_construct_*`），不处理服务注册。
> 推荐使用 `#[rust_dicore::inject(...)]` 替代——后者在相同构造函数生成能力的基础上额外自动完成 inventory 注册。
> 以下内容供需要手动控制注册流程、为 impl 块提供构造函数、或理解底层原理时参考。

派生宏声明为 `#[proc_macro_derive(Inject, attributes(inject))]`，因此字段上的 helper 属性名是 **`inject`**。

### 3.1 宏展开原理

`#[derive(Inject)]` 由 `inject_derive` 入口函数驱动，调用 `expand_inject_derive`。展开过程分为四个阶段：

**第一阶段：解析结构体定义**

`expand_inject_derive` 接收 `DeriveInput`，从中提取结构体名称 `name`，并匹配 `input.data`：

```rust
fn expand_inject_derive(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let fn_name = format_ident!("__rdi_construct_{}", name);
    let constructor_body = match &input.data {
        syn::Data::Struct(s) => match &s.fields {
            Fields::Named(named) => { /* 逐字段生成初始化表达式 */ }
            Fields::Unit => quote! { #name },
            _ => return Err(syn::Error::new_spanned(name, "named fields or unit struct")),
        },
        _ => return Err(syn::Error::new_spanned(name, "struct only")),
    };
    // ...
}
```

支持两种字段形式：

- **`Fields::Named`**：逐字段生成初始化表达式
- **`Fields::Unit`**：单元结构体，直接 `quote! { #name }`

若为元组结构体，报错 `"named fields or unit struct"`；若不是 struct（enum/union），报错 `"struct only"`。

**第二阶段：遍历字段，解析 `#[inject(...)]` 属性**

对每个字段调用 `parse_ia`，扫描字段属性并填充 `IA` 结构体：

```rust
#[derive(Default)]
struct IA {
    inject: bool,      // 是否有 #[inject] 标记（非 provider）
    owned: bool,       // owned 注入
    provider: bool,    // 注入 resolver
    key: Option<String>,
}
fn parse_ia(f: &Field) -> IA {
    let mut a = IA::default();
    for attr in &f.attrs {
        if !attr.path().is_ident("inject") {   // ← 字段 helper 属性名是 "inject"
            continue;
        }
        a.inject = true;
        let Ok(l) = attr.meta.require_list() else { continue; };  // #[inject] 无 list → 仅共享注入标记
        l.parse_nested_meta(|m| {
            if m.path.is_ident("owned") { a.owned = true; }
            else if m.path.is_ident("provider") {
                a.provider = true;
                a.inject = false;
            } else if m.path.is_ident("key") {
                a.key = Some(m.value()?.parse::<syn::LitStr>()?.value());
            }
            Ok(())
        }).ok();
    }
    a
}
```

**第三阶段：为每个字段生成解析表达式**

`gen_field_init` 返回 `syn::Result<TokenStream>`（支持编译期类型校验报错）。先调用 `classify_field(&field.ty)` 获取内层类型与 `FieldKind`（详见 §3.2），再按标记分发：

| 判断条件 | FieldKind 校验 | 生成逻辑 |
|---------|---------------|---------|
| 未标记（`!inject && !provider`） | 不校验 | `Default::default()` |
| `provider` | 不校验 | `resolver.clone()` |
| `inject && !owned` | 必须 `Arc` / `OptionArc`，否则 `syn::Error` | `OptionArc` → `get(_keyed)_any` 返回 Option；`Arc` → `get(_keyed)_any` + `unwrap_or_else(panic!)` |
| `inject && owned` | 必须 `Owned` / `OptionOwned`，否则 `syn::Error` | `OptionOwned` → `get(_keyed)_owned_any` + `try_unwrap` 返回 Option；`Owned` → + `unwrap_or_else(panic!)` |

可选性完全由 `FieldKind::OptionArc` / `OptionOwned`（即字段类型 `Option<...>`）决定，无需 `optional` 标记。

**第四阶段：生成构造器函数**

```rust
#[doc(hidden)]
pub fn __rdi_construct_{Name}(resolver: &dyn rust_dicore::IServiceResolver)
    -> ::std::sync::Arc<{Name}>
{
    ::std::sync::Arc::new({Name} { /* 每个字段的初始化表达式 */ })
}
```

关键特征：
- 函数标记 `#[doc(hidden)]`，IDE 中可调用但不污染文档补全
- 参数固定为 `resolver: &dyn rust_dicore::IServiceResolver`
- 返回固定为 `Arc<TypeName>`
- 字段初始化表达式直接内联展开，零额外函数调用开销

---

### 3.2 `classify_field()` 函数——类型分类

`classify_field()` 从字段类型中剥离外层包装，提取内层类型 `T` 并返回 `FieldKind`，供 `gen_field_init` 决定解析路径与类型校验。

```rust
#[derive(Clone, Copy, PartialEq)]
enum FieldKind {
    Arc,          // Arc<T>           → 共享注入
    OptionArc,    // Option<Arc<T>>   → 可选共享
    Owned,        // 裸 T             → owned 注入
    OptionOwned,  // Option<T>        → 可选 owned
}

fn classify_field(ty: &syn::Type) -> (proc_macro2::TokenStream, FieldKind) {
    // 返回值：(inner_type_tokens, FieldKind)
}
```

**识别规则：**

| 输入类型 | 输出 `inner` | `FieldKind` | 说明 |
|---------|-------------|------------|------|
| `Arc<Logger>` | `Logger` | `Arc` | 匹配 `Path("Arc")` 且含泛型参数 |
| `Option<Arc<MyService>>` | `MyService` | `OptionArc` | 嵌套匹配：`Option` → `Arc` → 泛型参数 |
| `Option<MyService>` | `MyService` | `OptionOwned` | `Option<T>` 中 `T` 非 `Arc` |
| `String` / 裸 `T` | `String` / `T` | `Owned` | 不匹配任何包装，原样返回 |
| `Arc<dyn ITrait>` | `dyn ITrait` | `Arc` | trait object 同样支持 |

**实现细节**（出自 `crates/macros/src/lib.rs`）：

```rust
fn classify_field(ty: &syn::Type) -> (proc_macro2::TokenStream, FieldKind) {
    if let syn::Type::Path(p) = ty {
        if let Some(last) = p.path.segments.last() {
            match last.ident.to_string().as_str() {
                "Arc" => {
                    if let Some(inner) = single_generic_type(last) {
                        return (quote! {#inner}, FieldKind::Arc);
                    }
                }
                "Option" => {
                    if let Some(inner_ty) = single_generic_type(last) {
                        if let syn::Type::Path(ip) = inner_ty {
                            if let Some(ilast) = ip.path.segments.last() {
                                if ilast.ident == "Arc" {
                                    if let Some(t) = single_generic_type(ilast) {
                                        return (quote! {#t}, FieldKind::OptionArc);
                                    }
                                } else {
                                    // Option<T> where T is not Arc → OptionOwned
                                    return (quote! {#inner_ty}, FieldKind::OptionOwned);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    (quote! {#ty}, FieldKind::Owned)
}
```

`FieldKind` 同时承担两个职责：决定解析 API（`get_any` vs `get_owned_any`）与**严格类型校验**——`#[inject]` 拒绝非 `Arc`/`OptionArc`，`#[inject(owned)]` 拒绝非 `Owned`/`OptionOwned`，由 `gen_field_init` 返回 `syn::Error` 触发编译错误。

---

### 3.3 字段属性完整参考表

字段属性名是 **`inject`**（与派生宏声明 `#[proc_macro_derive(Inject, attributes(inject))]` 对应）。显式注入：未标记字段用 `Default::default()`，可选性由 `FieldKind`（`Option<...>`）决定：

| 属性标记 | 字段类型要求 | 解析行为 | 未找到时行为 |
|---------|------------|---------|------------|
| **无标记** | 任意（需 `Default`） | `Default::default()` | N/A（直接使用默认值） |
| `#[inject]` | `Arc<T>` | `resolver.get_any(type_name::<T>())` → downcast 到 `Arc<T>` | **panic!** `"svc not registered"` |
| `#[inject]` | `Option<Arc<T>>` | `resolver.get_any` → downcast → `map(Arc::clone)` | 返回 `None`（不 panic） |
| `#[inject(key = "k")]` | `Arc<T>` | `resolver.get_keyed_any(type_name::<T>(), "k")` → downcast | **panic!** `"keyed not found"` |
| `#[inject(key = "k")]` | `Option<Arc<T>>` | `resolver.get_keyed_any` → downcast → `map` | 返回 `None`（不 panic） |
| `#[inject(owned)]` | 裸 `T` | `resolver.get_owned_any(type_name::<T>())` → downcast → `try_unwrap` | **panic!** `"owned svc not registered or Singleton"` |
| `#[inject(owned)]` | `Option<T>` | `resolver.get_owned_any` → downcast → `try_unwrap` → `map` | 返回 `None`（不 panic） |
| `#[inject(owned, key = "k")]` | 裸 `T` / `Option<T>` | `resolver.get_keyed_owned_any(type_name::<T>(), "k")` → downcast → `try_unwrap` | panic / `None`（同上） |
| `#[inject(provider)]` | `Arc<dyn IServiceResolver>` | `resolver.clone()` | N/A（直接传入 resolver） |

**属性组合规则：**

- `provider` 与 `owned`/`key` 互斥：`provider` 设置后 `inject` 置 false，仅注入 resolver
- `owned` 与 `key` 可组合：`#[inject(owned, key = "k")]` 表示键控 owned
- `key` 可与共享 `#[inject]` 或 owned `#[inject(owned)]` 组合，不能与 `provider` 组合
- 可选性不由标记决定，而由字段类型 `Option<...>` 决定（`FieldKind::OptionArc` / `OptionOwned`）

---

### 3.4 实战：完整服务定义

以下示例展示一个覆盖全部字段形态的 `OrderService`：

```rust
use rust_dicore::*;
use rust_dicore_macros::Inject;
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};

// 未标记字段类型必须实现 Default
#[derive(Default)]
struct Counter(AtomicU64);

impl Counter {
    fn next(&self) -> u64 { self.0.fetch_add(1, Ordering::SeqCst) }
}

// 覆盖全部字段形态的完整服务
#[derive(Inject)]
struct OrderService {
    // 1. #[inject] 必选共享：必须已注册，否则 panic
    #[inject]
    logger: Arc<Logger>,

    // 2. 未标记：内部字段，使用 Default::default()
    counter: Counter,

    // 3. #[inject] 可选共享：未注册时返回 None（由 Option 类型决定）
    #[inject]
    notifier: Option<Arc<Notifier>>,

    // 4. #[inject(key)] 键控共享：必须已注册，否则 panic
    #[inject(key = "order-logger")]
    order_logger: Arc<Logger>,

    // 5. #[inject(key)] 可选键控共享：未注册键 → None
    #[inject(key = "audit")]
    audit_logger: Option<Arc<Logger>>,

    // 6. #[inject(owned)] 必选 owned：未注册或 Singleton 则 panic
    #[inject(owned)]
    ctx: DbContext,

    // 7. #[inject(provider)]：直接获取 resolver
    #[inject(provider)]
    resolver: Arc<dyn IServiceResolver>,
}

#[derive(Default)]
struct Logger { prefix: String }

#[derive(Default)]
struct Notifier;

struct DbContext { n: u64 }

// 注册所有依赖
fn build_provider() -> Result<Arc<ServiceProvider>, RdiError> {
    Ok(Arc::new(ServiceCollection::new()
        .singleton(|_| Arc::new(Logger { prefix: "main".into() }))
        .keyed("order-logger", |_| Arc::new(Logger { prefix: "order".into() }))
        .singleton(|_| Arc::new(Notifier))
        .transient(|_| Arc::new(DbContext { n: 0 }))
        .build()?))
}

// 解析验证
fn test_order_service() {
    let provider = build_provider().unwrap();
    let service = __rdi_construct_OrderService(&provider);

    assert_eq!(service.logger.prefix, "main");          // #[inject] 共享
    assert_eq!(service.counter.next(), 0);               // 未标记 → Default
    assert!(service.notifier.is_some());                 // 可选共享 → Some
    assert_eq!(service.order_logger.prefix, "order");    // 键控共享
    assert!(service.audit_logger.is_none());             // 未注册键 → None
    // service.ctx 是 owned DbContext（每次新建）
    // service.resolver 可直接用于后续解析
}
```

---

### 3.5 编译错误排查

| 错误信息 | 原因 | 解决方法 |
|---------|------|---------|
| `"named fields or unit struct"` | 结构体是元组结构体（如 `struct Foo(i32)`） | 改为 `struct Foo { field: T }` 或单元结构体 `struct Foo;` |
| `"struct only"` | 对 `enum` 或 `union` 使用了 `#[derive(Inject)]` | 仅对 `struct` 使用此宏 |
| `#[inject]` 属性解析失败 | 属性拼写错误，如 `#[inject(keyed = "...")]` | 检查属性名是 `inject`，子标记仅 `owned`、`provider`、`key` |
| `#[inject] requires Arc<T> or Option<Arc<T>>` | `#[inject]` 标在了裸 `T` 字段上 | 改用 `#[inject(owned)]` |
| `#[inject(owned)] requires bare T or Option<T>` | `#[inject(owned)]` 标在了 `Arc<T>` 字段上 | 改用 `#[inject]` |
| `Default` trait 缺失 | 未标记字段类型未实现 `Default` | 为该类型实现 `Default`，如 `#[derive(Default)]` 或手动 `impl` |
| 运行时 panic `"svc not registered"` / `"keyed not found"` | 对应服务未注册 | 检查 `ServiceCollection` 注册：类型名和 key 必须匹配 |

---

### 3.6 生成函数命名

生成的函数名格式为 `__rdi_construct_{TypeName}`（注意前缀是 `__rdi_`，不是 `__lrdi_`）。

```rust
// 源码中：
let fn_name = format_ident!("__rdi_construct_{}", name);

// 示例展开：
#[derive(Inject)]
struct MyService { log: Arc<Logger> }

// 生成：
#[doc(hidden)]
pub fn __rdi_construct_MyService(resolver: &dyn rust_dicore::IServiceResolver)
    -> ::std::sync::Arc<MyService> { ... }
```

- 函数标记 `#[doc(hidden)]`，在 `cargo doc` 生成的文档中不可见
- 在 IDE 中虽然可见，但因文档隐藏不会污染自动补全列表
- 调用方式：`__rdi_construct_MyService(&resolver)`——直接使用，无需 `use` 或完全限定路径（只要在同一模块内）

这个构造函数是 **impl 块放置 `#[inject]` 的复用基础**：impl 块生成的 `__rdi_factory_{Type}_for_{Trait}` 会调用它。

---

## 四、`#[rust_dicore::module]` 完整指南（特定场景）

> **说明：** `#[rust_dicore::module]` 是特定场景下使用的宏方案，主要用于以下场景：
> - **外部类型注册**：注册不在当前 crate 定义的服务（如第三方库类型）
> - **条件编译**：通过模块级的 `#[cfg(...)]` 控制整组注册
> - **特性开关**：结合 Cargo feature flag 控制模块激活
> - **跨 DLL 插件导出**：模块自动生成 `build` 函数，适合插件按模块组织
>
> **常规场景优先使用 `#[rust_dicore::inject(...)]`**：对于当前 crate 内定义的服务，直接使用属性宏更加简洁。
> 以下内容供需要在模块级组织注册、使用 `register!` 声明语法或理解底层原理时参考。

### 4.1 宏展开原理

`#[rust_dicore::module]` 是一个属性宏（attribute macro），由 `module` 入口函数驱动，作用于 `mod` 块。

展开过程分为 7 个阶段：

**阶段 1：解析模块定义**

`expand_md` 接收 `ItemMod`，提取模块名（`m.ident`）和模块内容。若模块无 body（如 `mod foo;` 仅声明），产生错误 `"body required"`。

```rust
fn expand_md(mut m: ItemMod) -> syn::Result<proc_macro2::TokenStream> {
    let mn = m.ident.clone();
    let fn_n = format_ident!("__rdi_build_provider_{}", mn);
```

函数名固定为 `__rdi_build_provider_{module_name}`。

**阶段 2：分类模块内的 item（`register` 路径检测）**

遍历模块内的所有 item，将每个 item 分为两类：

- **`register!` 调用**：匹配 `syn::Item::Macro`，且 macro path 为 `register` 或 `rust_dicore::register` → 尝试解析为 `ID` 结构体，成功则加入 `rs` 向量
- **其他 item**：所有非 `register!` 的宏调用、函数、类型定义等 → 原样保留在 `cl` 向量中

```rust
let mut rs = Vec::new();  // 注册项列表
let mut cl = Vec::new();  // 保留项列表
for i in &is {
    match i {
        syn::Item::Macro(mc) => {
            let ps = mc.mac.path.segments.iter()
                .map(|s| s.ident.to_string()).collect::<Vec<_>>().join("::");
            if ps == "register" || ps == "rust_dicore::register" {
                if let Ok(r) = syn::parse2::<ID>(mc.mac.tokens.clone()) {
                    rs.push(r);  // ← 加入注册项
                }
            } else {
                cl.push(i.clone());  // ← 保留非 register 宏
            }
        }
        _ => cl.push(i.clone()),  // ← 保留所有非宏 item
    }
}
```

> **注意：** 检测的路径是 `register` 或 `rust_dicore::register`（旧版检测的是 `inject` 或 `lrdi::inject`，已随函数式宏改名而更新）。

**阶段 3：生成 ServiceCollection 链式调用**

对每个注册项 `ID`，根据其 `kind`（`IK` 枚举）生成对应的方法调用：

| `IK` 变体 | 对应 ServiceCollection 方法 | 方法选择依据 |
|----------|---------------------------|------------|
| `IK::N { lt, ty, imp }` | `lmt(lt)`（`singleton` / `scoped` / `transient`） | 生命周期 → 方法名 |
| `IK::K { key, lt, ty }` | `kmt(lt)`（`keyed` / `keyed_scoped` / `keyed_transient`） | 生命周期 → 键控方法名 |
| `IK::F { lt, f }` | `lmt(lt)`（`singleton` / `scoped` / `transient`） | 生命周期 → 方法名，使用自定义闭包 |

**阶段 4：生成 `__rdi_build_provider_*` 函数**

将 `ServiceCollection` 链式调用封装到函数中：

```rust
#[doc(hidden)]
pub fn __rdi_build_provider_{module}() -> ::std::result::Result<
    ::std::sync::Arc<rust_dicore::ServiceProvider>, rust_dicore::RdiError>
{
    Ok(::std::sync::Arc::new(
        rust_dicore::ServiceCollection::new()
            #(#ch)*  // ← 逐个链式调用
            .build()?
    ))
}
```

**阶段 5：重复 key 检测**

调用 `vd(&rs)` 函数，扫描所有 `IK::K` 注册项，检查是否有重复 key。详见 §4.5。

**阶段 6：保留原模块中的非 `register!` item**

所有未被识别为 `register!` 的 item 原样保留在模块中。

**阶段 7：输出完整模块**

将保留的 item 与新生成的函数合并，输出改写后的 `ItemMod`：

```rust
cl.push(bi);  // 将构建函数加入保留项
m.content = Some((syn::token::Brace::default(), cl));
Ok(quote! {#m})
```

---

### 4.2 `IK` 枚举——所有声明类型

`IK` 枚举定义在宏内部（非公开），表示三种注册模式：

```rust
enum IK {
    N { lt: LT, ty: syn::Type, imp: Option<syn::Type> },
    K { key: String, lt: LT, ty: syn::Type },
    F { lt: LT, f: syn::Expr },
}
```

| 变体 | 含义 | 对应语法 | 生成的 ServiceCollection 方法 |
|------|------|---------|---------------------------|
| `IK::N` | 普通注册 | `singleton: MyType` | `.singleton(\|_\| Arc::new(MyType::default()))` |
| `IK::N` (含 imp) | 接口实现注册 | `singleton: dyn ITrait => MyImpl` | `.singleton::<dyn ITrait>(\|_\| Arc::new(MyImpl::default()))` |
| `IK::K` | 键控注册 | `keyed "k": singleton: MyType` | `.keyed::<MyType>("k", \|_\| ...)` |
| `IK::F` | 自定义工厂 | `factory singleton: Type => expr` | `.singleton(move \|_\| Arc::new(expr))` |

**字段详解：**

- `lt: LT`：生命周期标签，决定方法名（见 §4.3）
- `ty: syn::Type`：服务的具体类型
- `imp: Option<syn::Type>`：（仅 `IK::N`）`dyn Trait` 的实现类型，若为 `Some` 则生成 `dyn Trait` → `ImplType` 映射
- `key: String`：（仅 `IK::K`）键名
- `f: syn::Expr`：（仅 `IK::F`）闭包体表达式

---

### 4.3 `LT` 枚举——生命周期解析

`LT` 枚举实现 `syn::Parse`，将标识符映射为生命周期：

```rust
enum LT { S, Sc, T }

impl Parse for LT {
    fn parse(i: ParseStream) -> syn::Result<Self> {
        match i.parse::<syn::Ident>()?.to_string().as_str() {
            "singleton" => Ok(LT::S),
            "scoped"    => Ok(LT::Sc),
            "transient" => Ok(LT::T),
            o => Err(syn::Error::new(i.span(), format!("unknown lifetime: {o}"))),
        }
    }
}
```

**对应关系：**

| 输入标识符 | `LT` 变体 | 普通注册方法 (`lmt`) | 键控注册方法 (`kmt`) |
|-----------|----------|-------------------|-------------------|
| `singleton` | `LT::S` | `.singleton(...)` | `.keyed(...)` |
| `scoped` | `LT::Sc` | `.scoped(...)` | `.keyed_scoped(...)` |
| `transient` | `LT::T` | `.transient(...)` | `.keyed_transient(...)` |

`lmt()` 和 `kmt()` 函数将 `LT` 枚举还原为对应的方法标识符：

```rust
fn lmt(lt: LT) -> proc_macro2::TokenStream {
    match lt {
        LT::S  => quote! {singleton},
        LT::Sc => quote! {scoped},
        LT::T  => quote! {transient},
    }
}

fn kmt(lt: LT) -> proc_macro2::TokenStream {
    match lt {
        LT::S  => quote! {keyed},
        LT::Sc => quote! {keyed_scoped},
        LT::T  => quote! {keyed_transient},
    }
}
```

---

### 4.4 声明语法完整参考

#### 语法 1：普通 singleton 注册

```rust
rust_dicore::register!(singleton: MyService);
```

等价于：
```rust
ServiceCollection::new()
    .singleton(|_: &dyn IServiceResolver| Arc::new(MyService::default()))
```

要求 `MyService` 实现 `Default`。

#### 语法 2：scoped 注册

```rust
rust_dicore::register!(scoped: RequestContext);
```

每次从 scope 中解析时重新创建实例。

#### 语法 3：transient 注册

```rust
rust_dicore::register!(transient: UuidGenerator);
```

每次解析都创建新实例。

#### 语法 4：dyn Trait → Impl 映射

```rust
rust_dicore::register!(singleton: dyn IPlugin => TestPlugin);
```

生成 `.singleton::<dyn IPlugin>(|_| Arc::new(TestPlugin::default()))`。`TestPlugin` 必须实现 `IPlugin + Default`。

#### 语法 5：键控注册

```rust
rust_dicore::register!(keyed "special": singleton: Logger);
rust_dicore::register!(keyed "temp": scoped: Logger);
rust_dicore::register!(keyed "per-call": transient: Logger);
```

分别对应 `.keyed("special", ...)`、`.keyed_scoped("temp", ...)`、`.keyed_transient("per-call", ...)`。

#### 语法 6：自定义工厂

```rust
rust_dicore::register!(factory singleton: Logger => Logger { prefix: "factory".into() });
```

生成 `.singleton(move |_: &dyn IServiceResolver| Arc::new(Logger { prefix: "factory".into() }))`。闭包体可以是任意表达式。

支持的 factory 生命周期：`factory singleton:`、`factory scoped:`、`factory transient:`。

#### 语法 7：`register!` 路径形式

```rust
// 在 #[rust_dicore::module] 内部，两种写法等价：
register!(singleton: MyService);
rust_dicore::register!(singleton: MyService);
```

宏通过检查 macro path 是否匹配 `"register"` 或 `"rust_dicore::register"` 来判断：

```rust
if ps == "register" || ps == "rust_dicore::register" { ... }
```

> `register!` 本身是一个**占位函数式宏**——独立调用时展开为空 TokenStream。它的真正用途是在 `#[module]` 内部被属性宏识别并转化为注册代码。

---

### 4.5 `vd()` 函数——重复 key 检测

`vd()` 函数（validate dedup）在代码生成前扫描所有注册项，检测键控注册中是否有重复的 key 值：

```rust
fn vd(rs: &[ID]) -> syn::Result<()> {
    let mut sn: std::collections::HashMap<String, usize> = HashMap::new();
    for r in rs {
        if let IK::K { key, .. } = &r.kind {
            let e = sn.entry(key.clone()).or_default();
            *e += 1;
            if *e > 1 {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!("rdi-E004: duplicate key `{key}`"),
                ));
            }
        }
    }
    Ok(())
}
```

**检测逻辑：**
1. 创建空的 `HashMap<String, usize>`
2. 遍历所有注册项，仅处理 `IK::K` 变体
3. 记录每个 key 的出现次数
4. 若出现次数 > 1，返回编译错误 `rdi-E004: duplicate key \`{key}\``

**示例 —— 触发编译错误：**

```rust
#[rust_dicore::module]
mod broken {
    rust_dicore::register!(keyed "same": singleton: Logger);
    rust_dicore::register!(keyed "same": singleton: Logger);  // ← 编译错误！
}
```

编译输出：
```
error: rdi-E004: duplicate key `same`
```

---

### 4.6 实际生成代码示例

**输入模块：**

```rust
#[rust_dicore::module]
mod my_services {
    rust_dicore::register!(singleton: Logger);
    rust_dicore::register!(singleton: dyn IPlugin => TestPlugin);
    rust_dicore::register!(keyed "special": singleton: Logger);
    rust_dicore::register!(factory singleton: Config => Config { env: "prod".into() });

    // 普通函数会被保留
    fn helper() -> &'static str { "hello" }
}
```

**展开后的等效代码：**

```rust
mod my_services {
    // 保留的普通函数
    fn helper() -> &'static str { "hello" }

    // 生成的构建函数
    #[doc(hidden)]
    pub fn __rdi_build_provider_my_services()
        -> ::std::result::Result<
            ::std::sync::Arc<rust_dicore::ServiceProvider>, rust_dicore::RdiError>
    {
        Ok(::std::sync::Arc::new(
            rust_dicore::ServiceCollection::new()
                .singleton(|_: &dyn rust_dicore::IServiceResolver|
                    ::std::sync::Arc::new(<Logger as ::std::default::Default>::default()))
                .singleton::<dyn IPlugin>(|_: &dyn rust_dicore::IServiceResolver|
                    ::std::sync::Arc::new(<TestPlugin as ::std::default::Default>::default()))
                .keyed::<Logger>("special", |_: &dyn rust_dicore::IServiceResolver|
                    ::std::sync::Arc::new(<Logger as ::std::default::Default>::default()))
                .singleton(move |_: &dyn rust_dicore::IServiceResolver|
                    ::std::sync::Arc::new(Config { env: "prod".into() }))
                .build()?
        ))
    }
}
```

**使用方式：**

```rust
let provider = my_services::__rdi_build_provider_my_services().unwrap();
let logger: Arc<Logger> = provider.get::<Logger>();
let plugin: Arc<dyn IPlugin> = provider.get::<dyn IPlugin>();
let special_logger: Arc<Logger> = provider.get_keyed::<Logger>("special");
```

---

### 4.7 与手动 `ServiceCollection` 的取舍

#### 何时使用 `#[rust_dicore::module]`

| 场景 | 优势 |
|------|------|
| **声明式配置** | 注册项集中声明在模块中，一目了然 |
| **就近配置** | 注册代码和实现代码在同一模块，减少跳转 |
| **库/插件导出** | 每个 `mod` 自动生成 `build` 函数，适合插件按模块组织 |
| **简单工厂** | 仅需 `Default` 的注册项，无需手写闭包 |
| **键控服务批量注册** | 语法简洁，重复 key 自动检测 |

#### 何时使用手动 `ServiceCollection`

| 场景 | 必要性 |
|------|-------|
| **复杂工厂逻辑** | 需要依赖注入、条件判断、外部配置读取的工厂闭包 |
| **动态注册** | 运行时根据配置/环境决定注册哪些服务 |
| **条件注册** | 使用 `try_add` 等条件方法 |
| **非 `Default` 构造** | 工厂需要传入参数，`#[module]` 的 factory 语法可支持但不如手写闭包灵活 |
| **需要 `instance()` / `singleton_value()`** | 直接注入已有实例 |

#### 推荐组合策略

在同一个项目中，三种方式可以自由组合：

```rust
// 1. #[rust_dicore::module] 用于模块级服务集合
#[rust_dicore::module]
mod infra {
    rust_dicore::register!(singleton: Logger);
    rust_dicore::register!(singleton: Config);
}

// 2. 手动 ServiceCollection 用于动态/条件注册
fn build_with_feature_toggle(enable_cache: bool) -> Result<Arc<ServiceProvider>, RdiError> {
    let base = ServiceCollection::new()
        .singleton(|_| Arc::new(Logger { prefix: "app".into() }));
    if enable_cache {
        base.singleton(|_| Arc::new(RedisCache::connect("redis://localhost")))
            .build()
    } else {
        base.singleton(|_| Arc::new(MemoryCache::default()))
            .build()
    }
}
```

---

## 五、宏混合使用策略

### 5.1 四种方式对比

| 维度 | 手动工厂 | `#[rust_dicore::inject]`（推荐） | `#[derive(Inject)]` | `#[rust_dicore::module]` |
|------|---------|-------------------------------|---------------------|------------------------|
| **宏类型** | — | 属性宏 | 派生宏 | 属性宏 |
| **构造函数生成** | 手写 | ✅ 自动 | ✅ 自动 | ❌ |
| **自动注册（具体类型）** | ❌ 手写 | ✅ inventory（struct 放置） | ❌ 手写 | ✅ 模块内声明 |
| **自动注册（dyn Trait）** | ❌ 手写 | ✅ inventory（impl 块放置） | ❌ 手写 | ✅（`dyn T => Impl` 语法） |
| **控制粒度** | 最细，完全可控 | 中等 | 消除构造器样板 | 消除注册样板 |
| **代码量** | 最多 | 最少 | 中等（需手写注册） | 较少 |
| **灵活性** | 最高 | 较高 | 中等（需配合 `#[inject(provider)]`） | 较低（局限于 Default 构造） |
| **编译期安全** | 运行时发现缺失 | 运行时 panic 缺失 | 运行时 panic 缺失 | 编译期检测重复 key |
| **适用场景** | 复杂依赖链 | **通用首选** | 仅需构造函数、为 impl 块提供构造器 | 外部类型、条件编译、插件 |

### 5.2 推荐架构组合

选择优先级：`#[rust_dicore::inject]` > `#[rust_dicore::module]` > 手动 `ServiceCollection`

```
┌──────────────────────────────────────────────────────────┐
│  应用层（组合根）                                           │
│  ┌──────────────────────────────────────────────────────┐ │
│  │  ServiceCollection::from_injected() + 手动补充        │ │
│  │  - 自动收集所有 #[rust_dicore::inject] 注册           │ │
│  │    （struct 放置 + impl 块放置）                      │ │
│  │  - 动态注册 / 条件注册                                │ │
│  │  - 生命周期精细控制                                   │ │
│  └──────────────────────────────────────────────────────┘ │
│                           │                                │
│  ┌────────────────────────▼─────────────────────────────┐ │
│  │  服务层                                                │ │
│  │  #[rust_dicore::inject(singleton)] struct Svc {...}  │ │
│  │  #[derive(Inject)] struct Plugin {...}               │ │
│  │  #[rust_dicore::inject] impl ITrait for Plugin {}    │ │
│  │  #[rust_dicore::inject] impl ITrait2 for Plugin {}   │ │
│  └──────────────────────────────────────────────────────┘ │
│                           │                                │
│  ┌────────────────────────▼─────────────────────────────┐ │
│  │  模块层（特定场景）                                       │ │
│  │  #[rust_dicore::module] mod infra_module { ... }     │ │
│  │  #[rust_dicore::module] mod plugin_module { ... }    │ │
│  └──────────────────────────────────────────────────────┘ │
│                           │                                │
│  ┌────────────────────────▼─────────────────────────────┐ │
│  │  领域层（兼容旧代码）                                       │ │
│  │  #[derive(Inject)] struct OrderService { ... }       │ │
│  │  #[derive(Inject)] struct UserService { ... }        │ │
│  └──────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────┘
```

### 5.3 完整混合示例

```rust
use rust_dicore::*;
use rust_dicore_macros::Inject;
use std::sync::Arc;

// ─── 类型定义 ─────────────────────────────────────

#[derive(Default)]
struct DatabaseConfig { url: String }

#[derive(Default)]
struct CacheConfig { ttl_secs: u64 }

#[derive(Default)]
struct Logger { prefix: String }

// ─── 推荐方式：struct 上 #[rust_dicore::inject] ────

#[rust_dicore::inject(singleton)]
struct OrderProcessor {
    #[inject]
    logger: Arc<Logger>,
    #[inject]
    db_config: Arc<DatabaseConfig>,
    #[inject]
    cache_config: Option<Arc<CacheConfig>>,
}

// ─── 接口注册：impl 块上 #[rust_dicore::inject] ────

trait IPlugin: Send + Sync { fn name(&self) -> &'static str; }

#[derive(Inject)]
struct OrderPlugin;

impl IPlugin for OrderPlugin {
    fn name(&self) -> &'static str { "order" }
}

#[rust_dicore::inject(singleton)]
impl IPlugin for OrderPlugin {}

// ─── 基础设施模块：使用 #[rust_dicore::module] 注册外部类型 ─

#[rust_dicore::module]
mod infra {
    rust_dicore::register!(singleton: super::Logger);
    rust_dicore::register!(singleton: super::DatabaseConfig);
}

// ─── 组合根：from_injected + 手动补充 ──────────────

fn build_app(use_cache: bool) -> Result<Arc<ServiceProvider>, RdiError> {
    let mut col = ServiceCollection::from_injected();

    // 手动注册 #[rust_dicore::module] 中的服务
    let infra_provider = infra::__rdi_build_provider_infra()?;
    col = col
        .singleton(|_| Arc::clone(&infra_provider.get::<Logger>()))
        .singleton(|_| Arc::clone(&infra_provider.get::<DatabaseConfig>()));

    if use_cache {
        col = col.singleton(|_| Arc::new(CacheConfig { ttl_secs: 300 }));
    }

    Arc::new(col.build()?)
}

fn main() -> Result<(), RdiError> {
    let provider = build_app(true)?;
    let order_proc: Arc<OrderProcessor> = provider.get::<OrderProcessor>();
    let plugin: Arc<dyn IPlugin> = provider.get::<dyn IPlugin>();

    assert!(order_proc.cache_config.is_some());  // 因 use_cache=true
    assert_eq!(plugin.name(), "order");
    Ok(())
}
```

---

## 六、源码映射速查表

源码文件：`crates/macros/src/lib.rs`（共约 581 行）。行号为近似范围，可能随重构微调。

| 宏/概念 | 源码位置 (`crates/macros/src/lib.rs`) | 关键类型/函数 |
|--------|--------------------------------------------------|-------------|
| `#[derive(Inject)]` 入口 | 第 14-19 行 | `#[proc_macro_derive(Inject, attributes(inject))]` → `inject_derive()` |
| 派生宏展开逻辑 | 第 21-41 行 | `expand_inject_derive()`（处理 `Fields::Named` 与 `Fields::Unit`） |
| 类型分类 `classify_field()` | 第 43-96 行 | `FieldKind { Arc, OptionArc, Owned, OptionOwned }` + `classify_field(ty) -> (inner, FieldKind)` |
| 字段初始化 `gen_field_init()` | 第 98-183 行 | `gen_field_init(field) -> syn::Result<TokenStream>`（显式注入分发 + 严格类型校验） |
| 字段属性 `IA` 结构体 | 第 185-191 行 | `IA { inject, owned, provider, key }` |
| `parse_ia()` 字段属性解析 | 第 192-216 行 | `attr.path().is_ident("inject")`（注意是 `inject`） |
| `register!` 占位函数式宏 | 第 139-142 行 | `#[proc_macro] register()` → 展开为空 |
| `#[module]` 入口 | 第 148-153 行 | `#[proc_macro_attribute] module()` |
| 模块展开 `expand_md()` | 第 155-220 行 | 检测 `register` / `rust_dicore::register` 路径 |
| 生命周期→方法映射 | 第 221-234 行 | `lmt()`、`kmt()` |
| 重复 key 检测 `vd()` | 第 235-250 行 | `rdi-E004: duplicate key` |
| `LT` 枚举 + `Parse` | 第 252-267 行 | `LT::{S, Sc, T}` |
| `IK` 枚举 + `ID` 结构体 + `Parse` | 第 268-335 行 | `IK::{N, K, F}` |
| `InjectArgs` 结构体 + `Parse` | 第 350-362 行 | `InjectArgs { lifetime: Option<LT> }`（无 Plain/AsTrait/AsTraits） |
| `#[inject]` 属性宏入口 | 第 364-372 行 | `#[proc_macro_attribute] inject()` |
| `expand_inject()` 分发 | 第 374-384 行 | `Item::Struct` → `expand_inject_struct`；`Item::Impl` → `expand_inject_impl` |
| struct 放置展开 | 第 387-488 行 | `expand_inject_struct()`：构造器 + `__rdi_factory_{Name}` + `__rdi_type_name_{Name}` + inventory + 剥离 `#[inject]` |
| impl 块放置展开 | 第 496-573 行 | `expand_inject_impl()`：自动检测 trait，生成 `__rdi_factory_{Type}_for_{Trait}`，注册为 `dyn Trait` |
| `lt_to_token()` | 第 575-581 行 | `LT` → `rust_dicore::ServiceLifetime::*` |

跨文件引用：

| 概念 | 源码位置 | 关键定义 |
|------|---------|---------|
| `ServiceRegistration` | `crates/core/src/registration.rs` | `{ lifetime, type_id, type_name_fn, factory }` + `inventory::collect!` |
| `ServiceCollection::from_injected()` | `crates/core/src/collection.rs` 第 129-142 行 | 遍历 `inventory::iter::<ServiceRegistration>`，运行时调用 `(reg.type_name_fn)()` |
