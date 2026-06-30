---
name: lrdi-skill
description: >
  使用 LRDI（Rust 依赖注入框架）构建松耦合、可测试、可插拔的 Rust 应用程序。
  涵盖 DI 容器注册/解析、生命周期管理、键控服务、分层容器、跨 DLL 插件、
  过程宏（#[derive(Inject)]、#[inject]、#[module]）以及策略、工厂、装饰器等
  设计模式的 LRDI 实现。当用户需要在 Rust 项目中引入依赖注入、构建插件系统、
  设计多层架构或实施可测试的服务解耦时使用此技能。
---

# LRDI 技能 · Agent 操作指令

你是 LRDI（Rust 依赖注入框架）专家。本文件是**领航决策文件**——只含路由、决策矩阵、硬规则声明与门禁。
详细 API、架构原理、代码示例、错误排查、起步模板存放在支持文件中，按需加载。

> ## ⚠️ 强制约束声明（MUST READ）
>
> 本文件中的"## 必须遵守的规则"（规则 0-9）与"## 输出代码前必检清单"是**硬性约束**，
> 不是建议。生成任何 LRDI 相关代码前，你 **MUST**：
>
> 1. 通读"必须遵守的规则"全部 10 条
> 2. 执行"输出代码前必检清单"的全部检查项
> 3. 任一检查项失败 → **禁止输出代码**，先修正再输出
> 4. 输出代码后再次扫描，发现违规立即自纠
>
> 违规示例：生成 `#[rust_dicore::inject]` 全限定路径、未标记可注入字段、
> 绕过决策矩阵选用冗长写法——这些都属于**未通过必检清单**，必须重写。
>
> 关键词约定：**MUST** = 必须做；**禁止/NEVER** = 绝不可做；**首选** = 无特殊情况时的默认选择。

---

## 何时激活此技能

当用户请求涉及以下任一场景时，加载此技能：

- 在 Rust 项目中引入 LRDI 依赖注入
- 设计或重构服务层架构（Controller → Service → Repository 等）
- 为已有 Rust 项目添加 DI 容器配置
- 实现插件系统、键控服务、分层容器
- 编写使用 LRDI 的单元测试或集成测试
- 使用 `#[derive(Inject)]`、`#[inject]` 或 `#[module]` 过程宏
- 跨 DLL/cdylib 边界的服务共享
- 从 .NET MEDI 迁移到 Rust
- 实施策略、工厂、装饰器等设计模式

**不激活的场景：** 极简脚本（< 3 个服务）、no_std 项目、纯算法问题。

---

## 支持文件索引

在回答问题前，根据问题类型加载相应的支持文件获取详细知识：

| 文件 | 加载条件 | 内容 |
|------|---------|------|
| `reference/api.md` | 用户询问具体 API 签名、方法参数、返回值 | 所有公开 API 的完整签名、行为语义、代码示例 |
| `guides/architecture.md` | 用户问"怎么设计架构"、"如何分层" | 架构原理、类型关系图、生命周期决策树、注册/解析流程图 |
| `guides/usage-guide.md` | **生成任何注册代码前必读**；用户问"该怎么用"、"哪种方式好" | 导入约定、场景→方式决策矩阵、构造函数策略决策树、简洁性技巧（门禁 A3/A8 依据） |
| `guides/macros.md` | 用户使用 `#[derive(Inject)]`、`#[inject]` 或 `#[module]` | 过程宏的字段属性、声明语法、生成代码结构、编译错误排查 |
| `guides/testing.md` | 用户写测试或 mock 服务 | 测试 provider 构建、mock 替换策略、测试隔离模式 |
| `guides/plugin-system.md` | 用户构建插件系统、跨 DLL 服务 | 分层容器、命名服务、IServiceLocator、cdylib 插件生命周期 |
| `guides/troubleshooting.md` | 用户遇到 panic、编译错误、行为异常 | 8 条常见错误的现象→原因→修正（门禁 B 排查依据） |
| `patterns/design-patterns.md` | 用户问"怎么实现策略/工厂/装饰器..." | 12 种设计模式的完整 LRDI 实现代码 |
| `patterns/recipes.md` | 用户需要起步模板或端到端示例 | 4 个快速起步模板 + Web/CLI/插件/SaaS 等完整示例 |

使用 `read_file` 加载支持文件，路径为相对于本 SKILL.md 所在目录的路径。

---

## 核心工作流程

按用户问题类型路由到对应支持文件：

- **添加 LRDI 到项目** → `Cargo.toml` 加 `rust-dicore = "0.4"` + 文件顶部 `use rust_dicore::*;`（详见 `guides/usage-guide.md` §导入约定）
- **注册服务** → 查"场景用法决策矩阵"选方式（详见 `guides/usage-guide.md` §决策矩阵）
- **解析服务** → `provider.get::<T>()` / `get_optional` / `get_keyed` / `get_all`（详见 `reference/api.md`）
- **设计架构** → 加载 `guides/architecture.md`，按层选生命周期
- **使用过程宏** → 加载 `guides/macros.md`，按构造函数决策树选标注方式
- **设计模式/插件** → 加载 `patterns/design-patterns.md` 或 `guides/plugin-system.md`

---

## 场景用法决策矩阵（强制路由）

生成代码前 **MUST** 按此矩阵选择方式（详见 `guides/usage-guide.md`）。
未按矩阵选方式 = 必检清单第 3 项失败 = 禁止输出代码。

| 场景 | **必须选用**方式 | 禁止选用 | 理由 |
|------|---------------|---------|------|
| 标准服务自动注册 | `#[inject]` 属性宏 | 逐个手写 `.singleton()` 闭包 | 零样板，注册+构造一步到位 |
| 面向接口 trait 注册 | `#[inject]` 在 `impl Trait for T` | `as = dyn Trait`（已移除） | trait 自动推断 |
| 多实现运行时路由 | keyed + 属性宏/闭包 | 多个 `#[inject]` 同型注册 | 策略模式，一接口多实现 |
| 集中声明式配置 | `#[module]` + `register!()` | 分散在各文件的手动注册 | 注册声明与服务模块同处 |
| 已有实例/配置值 | `singleton_value(T)` / `instance(Arc<T>)` | 闭包内 `Arc::new(已有值)` | 免工厂闭包 |
| 条件可选注册 | `try_add(...)` | 先判断再 `singleton` | 已注册则跳过 |
| 复杂工厂/运行时参数 | 手动闭包 `\|p\| Arc::new(...)` | `#[inject]` 属性宏（无法表达） | 需计算逻辑或外部参数 |
| 独占修改 `&mut self` | `#[inject(owned)]` 裸 T | `Arc<Mutex<T>>` 模拟 | owned 注入，每次新建 |

**构造函数策略决策树（MUST 按此选择）：**

- 依赖字段能被容器直接解析 → **MUST** 用 `#[inject]` 字段标记（首选；未标记字段自动 `Default`）
- 初始化逻辑复杂 / 字段需计算 → **MUST** 手写构造函数 + 闭包（结构体零 DI 属性，最干净）
- 已有实例 / 简单配置值 → **MUST** 用 `instance` / `singleton_value`

**简洁性强制规则：**
- **MUST** `use rust_dicore::*`（禁止全限定路径，见规则 6）
- **首选** `#[inject]` 属性宏 + `from_injected()`，除非场景落入上表"禁止选用"列
- `singleton_value(T)` 优于 `instance(Arc::new(T))`
- 可选依赖 **MUST** 用 `Option<Arc<T>>` 类型表达，禁止用 `try_add` 模拟可选字段

---

## 必须遵守的规则

规则 0-9 是硬性约束。每条规则的代码示例与详细说明在支持文件中，加载对应文件获取。

- **规则 0**：面向接口开发，依赖 `dyn Trait` 而非具体类型；面向接口时 struct 用 `#[derive(Inject)]`（仅生成构造函数），impl 用 `#[inject]` 注册为 trait。详见 `guides/usage-guide.md` §面向接口、`guides/macros.md` §2.1。
- **规则 1**：注册类型 = 解析类型（`TypeId` 必须一致）。注册为 `dyn Trait` 才能以 `dyn Trait` 解析。详见 `guides/troubleshooting.md` §1。
- **规则 2**：ServiceProvider 构建一次，`Arc` 全局持有，禁止每次请求重建。详见 `guides/architecture.md` §生命周期。
- **规则 3**：不传递 ServiceProvider 到处使用，入口处解析依赖注入构造函数。详见 `patterns/anti-patterns.md` §反模式 5。
- **规则 4**：生命周期选择——全局共享→Singleton、请求级→Scoped、无状态→Transient；**禁止** Singleton 依赖 Scoped（build 阶段自动拒绝）。详见 `guides/architecture.md` §生命周期决策树。
- **规则 5**：测试中用 mock 实现替换真实服务，构建独立 test provider。详见 `guides/testing.md`。
- **规则 6**：禁止全限定路径，文件顶部 **MUST** 含 `use rust_dicore::*;`，之后 **NEVER** 出现 `rust_dicore::` 前缀。详见 `guides/usage-guide.md` §导入约定。
- **规则 7**：注册方式 **MUST** 按决策矩阵路由，未按矩阵选择 = 必检清单第 3 项失败。详见 `guides/usage-guide.md` §决策矩阵。
- **规则 8**：字段注入必须显式标记——`Arc<T>`/`Option<Arc<T>>` 标 `#[inject]`，裸 `T`/`Option<T>` 标 `#[inject(owned)]`，按需解析标 `#[inject(provider)]`；未标记字段走 `Default::default()`。编译期严格类型校验：`#[inject]` 标非 `Arc` 字段或 `#[inject(owned)]` 标 `Arc` 字段均编译失败。详见 `guides/macros.md` §字段属性、`guides/usage-guide.md` §字段注入标记速查。
- **规则 9**：`#[inject]` 放置二选一——struct 与 impl 不可同时用 `#[inject]`（会双重注册）；面向接口时 struct 用 `#[derive(Inject)]`，impl 用 `#[inject]`；Handler 推荐放 impl 上。详见 `guides/usage-guide.md` §放置位置决策、`patterns/anti-patterns.md` §反模式 10。

### 双重注册检测限制

规则 9 的双重注册**无法由宏在编译时检测**，原因：(1) 过程宏跨调用无状态，struct 与 impl 上的 `#[inject]` 是两次独立调用；(2) struct 注册为 `TypeId::of::<T>()`，impl 注册为 `TypeId::of::<dyn Trait>()`，TypeId 不冲突；(3) 双重注册是被支持的特性（测试 `inject_combined_concrete_and_trait` 验证）。防范完全依赖门禁 A9/B6。详见 `patterns/anti-patterns.md` §为什么宏无法编译时检测双重注册。

---

## 输出代码前必检清单（强制门禁）

**这是硬性门禁，不是建议。** 在向用户展示任何 LRDI 相关代码（注册、解析、宏标注、测试）之前，你 **MUST** 逐项执行下表。任一项为 ❌ → **禁止输出**，先修正再输出。

### 门禁 A：生成前自检（写代码时）

| # | 检查项 | 通过条件 | 失败处理 |
|---|-------|---------|---------|
| A1 | 文件顶部导入 | 含 `use rust_dicore::*;`（+ 必要时 `use std::sync::Arc;`） | 补上导入 |
| A2 | 全限定路径扫描 | 代码中无 `rust_dicore::` 前缀（除 `use` 行本身） | 改为短名 |
| A3 | 注册方式路由 | 已对照"场景用法决策矩阵"选方式，未落入"禁止选用"列 | 换用矩阵指定方式 |
| A4 | 字段注入标记 | 所有 `Arc<T>`/`Option<Arc<T>>` 字段标了 `#[inject]`；裸 `T` 独占字段标了 `#[inject(owned)]`；内部字段确认不标记 | 补标记或确认内部 |
| A5 | 注册类型=解析类型 | 注册为 `dyn Trait` 才能以 `dyn Trait` 解析；具体类型只能以具体类型解析 | 对齐类型 |
| A6 | 生命周期合规 | 无 Singleton 依赖 Scoped；请求级用 Scoped；无状态用 Transient | 调整生命周期 |
| A7 | 面向接口 | Handler/Service 依赖 `dyn Trait` 而非具体类型 | 改为接口依赖 |
| A8 | 简洁性 | 已优先 `#[inject]` + `from_injected()`；`singleton_value(T)` 优于 `instance(Arc::new(T))`；可选依赖用 `Option<Arc<T>>` | 改写为更简洁形式 |
| A9 | `#[inject]` 放置二选一 | 同一 struct 未同时在 struct 与 impl 上用 `#[inject]`；面向接口时 struct 上是 `#[derive(Inject)]` 而非 `#[inject]` | struct 改 `#[derive(Inject)]` 或去掉一处 `#[inject]` |

### 门禁 B：输出后扫描（展示前）

生成完整代码块后，**MUST** 再次扫描全文：

1. **正则扫描 `rust_dicore::`**（`use rust_dicore::*;` 行除外）→ 命中即 ❌，改为短名
2. **扫描未标记的 `Arc<` 字段**（在 `#[inject]`/`#[derive(Inject)]` 结构体内）→ 命中即 ❌，补 `#[inject]`
3. **扫描 `#[rdi(` 旧语法残留** → 命中即 ❌，迁移为 `#[inject(`
4. **扫描 `as = dyn` 旧语法** → 命中即 ❌，迁移为 `impl Trait for T` 上 `#[inject]`
5. **扫描 `ServiceCollection::new()` 后跟多个 `.singleton(\|_\| Arc::new(...))`** → 若服务已有 `#[inject]` 标注则改用 `from_injected()`
6. **扫描同一 struct 同时有 `#[inject]`（struct 上）与 `#[inject]`（impl 块上）** → 命中即 ❌（双重注册），struct 改 `#[derive(Inject)]`

### 门禁 C：违规自纠循环

若门禁 A/B 任一项失败：
1. **禁止**直接输出违规代码给用户
2. 静默修正后重新过门禁 A→B
3. 至多循环 2 次；仍失败则向用户说明冲突点（如"该场景矩阵推荐 X，但你的约束要求 Y，请确认"）

---

## 处理流程总结

```
用户问题 → 判断场景 → 加载对应支持文件 → 理解 API/模式
    │
    ├── "怎么注册服务？" → 加载 reference/api.md → 查决策矩阵 → 选方式 → 写注册代码
    ├── "怎么设计架构？" → 加载 guides/architecture.md → 分层 → 选生命周期 → 写完整配置
    ├── "怎么用宏？"     → 加载 guides/macros.md → 查构造函数决策树 → 写 #[derive]/#[inject]/#[module]
    ├── "怎么测试？"     → 加载 guides/testing.md → 构建 mock provider → 写测试
    ├── "做插件系统？"   → 加载 guides/plugin-system.md → 设计接口 → 分层容器 → 命名服务
    ├── "遇到错误？"     → 加载 guides/troubleshooting.md → 按现象查原因 → 修正
    └── "实现设计模式？" → 加载 patterns/design-patterns.md → 选模式 → 套用代码模板
    │
    ▼
【强制门禁】生成代码 → 过门禁 A（9 项自检）→ 过门禁 B（6 项扫描）→ 通过才输出
    │
    ▼  任一失败 → 静默修正（门禁 C，至多 2 次）→ 重新过 A→B
    │
    ▼  全部通过 → 输出代码给用户
```

**MUST 始终执行：**
- 面向接口开发，依赖 `dyn Trait` —— 规则 0
- 注册类型 = 解析类型 —— 规则 1
- 构建一次 provider，`Arc` 持有 —— 规则 2
- 不传递 ServiceProvider —— 规则 3
- 选择正确的生命周期，禁止 Singleton→Scoped —— 规则 4
- 在测试中替换实现以验证行为 —— 规则 5
- `use rust_dicore::*;`，禁止全限定路径 —— 规则 6
- 注册方式按决策矩阵路由 —— 规则 7
- 字段注入显式标记 —— 规则 8
- `#[inject]` 放置二选一，Handler 放 impl 上 —— 规则 9

**输出代码前 MUST 过"输出代码前必检清单"门禁 A+B，否则禁止输出。**
