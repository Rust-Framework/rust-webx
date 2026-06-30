# 常见错误排查

当用户遇到错误时，按以下顺序排查。每条给出 **现象 → 原因 → 修正**。

> 本文示例采用简短路径风格，假定文件顶部已 `use rust_dicore::*;`。

## 1. panic: "service not registered: xxx"

**原因**：注册时的类型与解析时的类型不一致（`TypeId` 不匹配）。常见于注册为具体类型却以 `dyn Trait` 解析，或反之。

**修正**：确保注册类型 = 解析类型（规则 1）。

```rust
// ❌ 注册为具体类型，解析为 trait → panic
.singleton(|_| Arc::new(FileLogger))
let logger: Arc<dyn ILogger> = provider.get();  // panic

// ✅ 注册为 dyn Trait，解析为 dyn Trait
.singleton::<dyn ILogger>(|_| Arc::new(FileLogger))
let logger: Arc<dyn ILogger> = provider.get();  // OK
```

## 2. panic: "keyed service not registered: xxx:yyy"

**原因**：key 名称拼写错误，或 keyed 服务未注册，或注册为其他 key。

**修正**：检查 key 拼写；安全解析用 `try_get_keyed`。

```rust
.keyed::<dyn IPayGateway>("wechat", |_| Arc::new(WechatPay))
// 安全解析，未注册返回 None 而非 panic
if let Some(gw) = provider.try_get_keyed::<dyn IPayGateway>("wechat") { /* ... */ }
```

## 3. `#[derive(Inject)]` 编译失败：named struct or unit struct required

**原因**：使用了元组结构体（`struct Foo(i32)`）或 enum——宏仅支持命名结构体和单元结构体。

**修正**：改用命名结构体或单元结构体。详见 `guides/macros.md` 编译错误排查表。

```rust
// ❌ 元组结构体
struct Service(i32, String);

// ✅ 命名结构体
struct Service { value: i32, name: String }

// ✅ 单元结构体
struct Marker;
```

## 4. `#[inject]` 编译失败：only structs are supported

**原因**：同上——`#[inject]` 属性宏放在了非命名/非单元结构体上。

**修正**：改用命名结构体或单元结构体。

## 5. `#[inject]` 无法推断依赖（字段取到 Default 值而非注入实例）

**原因**：字段未显式标 `#[inject]`——未标记字段走 `Default::default()`，不会从容器解析。或字段类型未注册。

**修正**：依赖字段 **MUST** 标 `#[inject]`（`Arc<T>`）或 `#[inject(owned)]`（裸 `T`）；确保对应类型已注册；按需解析用 `#[inject(provider)]`。详见 `guides/usage-guide.md` 字段注入标记速查。

```rust
// ❌ 漏标 #[inject] → 取 Default → 运行时 panic
struct Handler { repo: Arc<dyn IRepo> }

// ✅ 显式标记
struct Handler { #[inject] repo: Arc<dyn IRepo> }
```

## 6. `#[module]` 编译失败：duplicate key `xxx`

**原因**：`#[module]` 块内 `register!()` 重复定义同名 keyed 服务。

**修正**：确保每个 key 在 module 内唯一。详见 `guides/macros.md` §`#[module]` 指南。

## 7. 服务未按预期缓存（Singleton 每次不同 / Scoped 不随请求变化）

**原因**：生命周期设置错误。Singleton 在 provider 构建时即缓存；根级 Scoped 缓存在 `root_scoped_cache`（等同应用级单例）。

**修正**：
- 全局共享 → Singleton
- 请求级隔离 → Scoped + `provider.create_scope()` 子 Scope 解析
- 每次新建 → Transient

详见 `guides/architecture.md` 生命周期决策树。

## 8. 路径冗长 / 代码啰嗦

**原因**：未使用 `use rust_dicore::*;` 简化导入；手动逐个 `.singleton()` 而非用 `from_injected()`。

**修正**：
- 文件顶部加 `use rust_dicore::*;`，禁止 `#[rust_dicore::inject]` 全限定路径（规则 6）
- 用 `#[inject]` 属性宏 + `from_injected()` 替代逐个手动注册
- `singleton_value(T)` 优于 `instance(Arc::new(T))`

详见 `guides/usage-guide.md` 简洁性技巧清单。
