# 用法指南与决策矩阵（AI agent 参考）

本指南是 rust-dicore 推荐用法的紧凑决策参考，供生成代码时快速路由。详细说明见 `docs/rust-dicore/08-best-practices/usage-guide.md`。

## 1. 导入约定（生成代码必遵）

每个使用 DI 的文件顶部固定两行：

```rust
use rust_dicore::*;
use std::sync::Arc;
```

**禁止**生成 `#[rust_dicore::inject]`、`rust_dicore::register!()`、`rust_dicore::ServiceCollection::new()` 等全限定路径。统一用短名：

| 禁止 | 用 |
|------|----|
| `#[rust_dicore::inject]` | `#[inject]` |
| `#[rust_dicore::inject(scoped)]` | `#[inject(scoped)]` |
| `#[derive(rust_dicore::Inject)]` | `#[derive(Inject)]` |
| `#[rust_dicore::module]` | `#[module]` |
| `rust_dicore::register!(...)` | `register!(...)` |
| `rust_dicore::ServiceCollection` | `ServiceCollection` |

大型多 crate 项目可选 `use rust_dicore as di;` → `#[di::inject]`。

## 2. 场景 → 推荐方式路由表

| 场景 | 推荐方式 | 关键 API |
|------|---------|---------|
| 标准服务自动注册 | `#[inject]` 属性宏 | `#[inject(singleton\|scoped\|transient)]` |
| 面向接口 trait 注册 | `#[inject]` 在 impl 块 | `#[inject] impl Trait for T` |
| 多实现运行时路由 | keyed | `#[inject(keyed = "k")]` 或 `keyed_singleton` |
| 集中声明式配置 | module | `#[module]` + `register!()` |
| 已有实例/配置值 | 实例注入 | `singleton_value(T)` / `instance(Arc<T>)` |
| 条件可选注册 | try_add | `.try_add(...)` |
| 复杂工厂/运行时参数 | 手动闭包 | `.singleton(\|p\| Arc::new(...))` |
| 独占修改 `&mut self` | owned 注入 | `#[inject(owned)]` 裸 T |

### 放置位置决策：struct 还是 impl？（二选一）

`#[inject]` 放 struct 上 = 注册**具体类型**；放 `impl Trait for T` 上 = 注册 **`dyn Trait`**。两者同时用 = 双重注册（通常非预期）。`#[derive(Inject)]` 只生成构造函数、不注册，是面向接口时 struct 侧的正确选择。

| 目标身份 | struct 上 | impl 上 | 结果 |
|---------|----------|---------|------|
| 仅具体类型（无 trait） | `#[inject]` | *不放* | ✅ |
| 仅 `dyn Trait`（**推荐**） | `#[derive(Inject)]` | `#[inject]` | ✅ |
| 两者都要（罕见） | `#[inject]` | `#[inject]` | ⚠️ 双重注册 |
| 仅 impl、struct 无构造 | *不放* | `#[inject]` | ❌ 编译失败 |

```rust
// ✅ Handler 推荐放 impl 上：struct 用 #[derive(Inject)] 仅构造，impl 用 #[inject] 注册为 trait
#[derive(Inject)]
struct UserHandler { #[inject] user_svc: Arc<dyn IUserService> }
#[inject] impl IUserHandler for UserHandler { /* ... */ }

// ❌ 双重注册：struct 和 impl 都用 #[inject]
#[inject(transient)] struct UserHandler { /* ... */ }
#[inject] impl IUserHandler for UserHandler { /* ... */ }  // 可被 get::<UserHandler>() 绕过

// ✅ 无 trait 契约（配置/值对象）：只在 struct 上用 #[inject]
#[inject(singleton)] struct Config { port: u16 }
```

**何时放 struct**：服务作具体类型对外暴露（配置、值对象，无 trait 契约）。
**何时放 impl**：服务有 trait 契约，面向接口便于 mock（Handler/Service/Repository，**推荐**）。**Handler 推荐放 impl 上**，struct 上用 `#[derive(Inject)]`。

## 3. 构造函数策略决策树

```
依赖字段能否被容器直接解析？
├─ 能（都是注册的服务）
│  → 用 #[inject] 字段标记（首选，零样板）
│     #[inject(singleton)]
│     struct S { #[inject] dep: Arc<dyn ITrait>, state: u32 }
│     // state 未标记 → Default::default()
└─ 不能（需计算 / 已有实例 / 运行时参数）
   ├─ 初始化逻辑复杂
   │  → 手写构造函数 + 闭包（结构体最干净，零 DI 属性）
   │     struct S { dep: Arc<dyn IT>, threshold: u32 }
   │     impl S { pub fn new(dep: Arc<dyn IT>, t: u32) -> Self { ... } }
   │     .singleton(|p| Arc::new(S::new(p.get(), p.get::<Config>().t)))
   └─ 简单已有实例
      → instance / singleton_value
      .instance(Arc::new(existing)) / .singleton_value(Config { .. })
```

## 4. 字段注入标记速查（仅 `#[inject]` 标记策略适用）

| 标记 | 字段类型 | 行为 |
|------|---------|------|
| *(无标记)* | 任意（需 Default） | `Default::default()` |
| `#[inject]` | `Arc<T>` | 共享注入，未注册 panic |
| `#[inject]` | `Option<Arc<T>>` | 可选共享，未注册 None |
| `#[inject(key = "k")]` | `Arc<T>` | 键控共享 |
| `#[inject(owned)]` | 裸 `T` | owned 注入，Singleton panic |
| `#[inject(owned)]` | `Option<T>` | 可选 owned |
| `#[inject(provider)]` | `Arc<dyn IServiceResolver>` | 注入 resolver |

严格类型校验：`#[inject]` 仅 `Arc<T>`/`Option<Arc<T>>`，`#[inject(owned)]` 仅裸 `T`/`Option<T>`。可选性由 `Option<...>` 类型决定，无 `optional` 标记。

## 5. 简洁性技巧清单（生成代码时优先应用）

1. `use rust_dicore::*` 统一引入，禁全限定路径
2. `#[inject]` 属性宏零样板（注册+构造一步到位），优于手动 `ServiceCollection` 逐个注册
3. impl 块 trait 自动推断，无需 `as = dyn Trait`
4. `singleton_value(T)` 自动 Arc，优于 `instance(Arc::new(T))`
5. `from_injected()` 一行收集所有 `#[inject]` 注册
6. `Option<Arc<T>>` 可选依赖由类型决定，无需额外标记
7. 单元结构体 `struct Config;` 直接支持
8. `try_add` 条件注册不覆盖已有
9. 未标记字段自动 `Default`，结构体保持干净（无 `#[rdi(skip)]`）
10. keyed 服务实现策略模式，一接口多实现

## 6. 完整示例（推荐写法）

```rust
use rust_dicore::*;
use std::sync::Arc;

#[inject(singleton)]
struct OrderService {
    #[inject] repo: Arc<dyn IOrderRepo>,
    #[inject] logger: Arc<dyn ILogger>,
    threshold: u32,                 // 未标记 → Default
}

#[inject] impl IOrderRepo for PgOrderRepo { /* ... */ }
#[inject] impl ILogger for ConsoleLogger { /* ... */ }

fn build() -> Result<ServiceProvider, RdiError> {
    ServiceCollection::from_injected().build()
}
```
