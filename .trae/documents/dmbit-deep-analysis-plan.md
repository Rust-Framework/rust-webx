# dmbit 项目深度分析：功能完整性、逻辑闭环与数据维护合理性

## 用户决策

| 决策项 | 选择 |
|--------|------|
| 数据模型 | **方案A：拆分为 Spec（规格）+ Device（实例）** |
| CSV 格式 | **允许 Breaking Change** |
| 扩展性 | **需支持华为昇腾等新型设备** |

---

## 一、业务需求解读（基于 README.md）

### 1.1 核心业务场景
"直播数据智算机房"的设备清单管理系统，管理运算服务器和存储服务器的上架及集成联调。

### 1.2 README 中的设备数据

| 序号 | 产品类别 | 关键配置 | 数量 |
|------|---------|---------|------|
| A | 运算服务器（不含显卡） | 6槽位PCIe扩展板，默认不含显卡 | 1800台 |
| B | 存储服务器 | 36块 8TB DC HC320 硬盘 | 388台 |
| C | 运算服务器（含RTX5090） | RTX5090×6 | 150台 |
| D | 运算服务器（含RTX4090） | RTX4090×6 | 150台 |

共计 **2488 台**，4 种规格配置，2 种产品大类。

### 1.3 隐含的业务需求
1. **设备台帐管理**：增删改查产品/规格/部件
2. **库存批量导入**：通过CSV批量导入设备清单
3. **库存导出**：导出CSV用于Excel查看
4. **运行状态跟踪**：运行中/联调中/待上架/已交付
5. **大屏仪表盘**：KPI统计（总量、类别分布、加速卡汇总、硬盘汇总、状态分布）
6. **管理员认证**：登录/改密码，JWT
7. **扩展性**：未来需支持华为昇腾等新型设备（如NPU加速卡）

---

## 二、现有数据模型分析

### 2.1 当前实体关系

```
Product (产品主表)  1 ──── N  Goods (台账从表)  1 ──── N  GoodsComponent (部件)
    │                        │                              │
    ├─ name                 ├─ brand                       ├─ kind
    ├─ code (唯一)          ├─ parameters                  ├─ model
    ├─ category             ├─ unit                        ├─ capacity_gb
    │  (compute/storage)    ├─ quantity                    ├─ qty_per_unit
    └─ remark               ├─ status  ← 实例属性混入
                            ├─ location ← 实例属性混入
                            ├─ asset_code ← 实例属性混入
                            └─ sort_order
```

### 2.2 问题清单

---

#### **问题 P1：Goods 实体概念混淆 —— 规格定义 vs 设备实例** 🔴

当前 `Goods` 实体同时包含两类属性：

- **规格属性**：`brand`、`parameters`、`unit`、`components`
- **实例属性**：`status`、`location`、`asset_code`

**影响**：
1. 同规格设备部分状态变更时必须**拆分 Goods 行**（数据维护灾难）
2. 同规格设备分配到不同机位时必须拆分 Goods 行
3. `quantity` 含义模糊：是规格总量还是当前可用量？与 `status` 什么关系？

**证据**：[goods.rs:31-38](file:///e:/GitCode/RF/rust-webx/dmbit/domain/src/entities/goods.rs#L31-L38)

**决策**：采用方案A，拆分为 Spec + Device。

---

#### **问题 P2：台账业务唯一键设计错误** 🔴

`assert_goods_key_available` 使用 `(product_id, brand, asset_code, location)` 作为唯一键（[mapping.rs:269-293](file:///e:/GitCode/RF/rust-webx/dmbit/handlers/src/mapping.rs#L269-L293)），CSV导入分组键同样使用此四元组（[inventory.rs:542-547](file:///e:/GitCode/RF/rust-webx/dmbit/handlers/src/inventory.rs#L542-L547)）。

**问题**：`asset_code` 和 `location` 是实例级属性，不应参与规格的唯一标识。

**修正**：Spec 使用 `code`（新增字段）作为业务唯一键。

---

#### **问题 P3：Parameters 结构化/非结构化的往返不一致** 🟡

Parameters 以多行文本存储，但 CSV 导出时拆分为7个固定列（机箱/主板/内存/接口/扩展/电源/光模块）+ "其他参数"列。导入时用 `；` 或 `;` 或 `\n` 解析"其他参数"导致往返不一致。

**证据**：[inventory.rs:40-65](file:///e:/GitCode/RF/rust-webx/dmbit/handlers/src/inventory.rs#L40-L65)（列定义）、[inventory.rs:186](file:///e:/GitCode/RF/rust-webx/dmbit/handlers/src/inventory.rs#L186)（`；` 连接）vs [inventory.rs:215](file:///e:/GitCode/RF/rust-webx/dmbit/handlers/src/inventory.rs#L215)（`\n` 解析）

**决策**：parameters 作为不透明文本字段，CSV 中不做拆分。

---

#### **问题 P4：load_components_for 全表扫描** 🟡

[mapping.rs:295-317](file:///e:/GitCode/RF/rust-webx/dmbit/handlers/src/mapping.rs#L295-L317) 加载全部 Component 后在内存中 O(n*m) 过滤。

**修正**：使用 `goods_id IN (...)` 查询 + HashSet 查找。

---

#### **问题 P5：Dashboard 公开接口无缓存/全量加载** 🟡

每次请求加载全部 Product + Goods + Component，且为公开接口。

**修正**：变为 Spec + Device 后，聚合逻辑基于 Device 计数，可优化查询。

---

#### **问题 P6：软删除 + 唯一约束冲突** 🟡

`Product.code` 有 `#[unique]`（[product.rs:19](file:///e:/GitCode/RF/rust-webx/dmbit/domain/src/entities/product.rs#L19)），软删除后唯一索引仍被占用。

**修正**：确认 ORM 是否支持 `WHERE is_deleted = false` 部分索引；若不支持，软删除时追加时间戳后缀。

---

#### **问题 P7：replace_components 软删除导致数据膨胀** 🟡

[mapping.rs:319-397](file:///e:/GitCode/RF/rust-webx/dmbit/handlers/src/mapping.rs#L319-L397) 全量替换部件时旧记录软删除而非物理删除。

**修正**：全量替换场景应物理删除旧记录。

---

#### **问题 P8：Dashboard compute/storage else 分支隐患** 🟢

[dashboard.rs:99-103](file:///e:/GitCode/RF/rust-webx/dmbit/handlers/src/dashboard.rs#L99-L103) `else` 分支将任何非 "storage" 值归入 compute。新增第三种类别时会出错。

**修正**：使用显式 match。

---

#### **问题 P9：状态值三处硬编码** 🟢

`GOODS_STATUSES` 在 mapping.rs、dashboard.rs、admin.js 三处重复定义。

**修正**：统一定义在 contracts 层。

---

#### **问题 P10：删除 Product 后历史数据不可见** 🟡

删除 Product 级联软删所有 Goods 和 Component，Dashboard 聚合时这些记录消失。

**修正**：拆分 Device 后，设备级的状态跟踪可独立管理。Spec 被删除时 Device 可设为"已淘汰"而非删除。

---

#### **问题 P11：CSV 稀疏续行校验过于严格** 🟢

[inventory.rs:549-612](file:///e:/GitCode/RF/rust-webx/dmbit/handlers/src/inventory.rs#L549-L612) 续行与首行不一致时直接报错，无修复路径。

**修正**：简化 CSV 格式后此问题自然消除。

---

## 三、最优设计方案

### 3.1 新实体模型

```
┌──────────────────────────────────────────────────────────────────┐
│                        Product（产品大类）                         │
│  id | name | code(UK) | category | remark | sort_order | ...     │
│  例: "运算服务器" / code="CMP-SRV" / category="compute"           │
│  例: "存储服务器" / code="STO-SRV" / category="storage"           │
│  例: "AI训练服务器" / code="AI-SRV" / category="compute"  ← 扩展  │
└────────────────────────────┬─────────────────────────────────────┘
                             │ 1:N
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│                        Spec（设备规格）                            │
│  id | product_id(FK) | code(UK) | brand | parameters | unit     │
│  planned_quantity | sort_order | ...                             │
│  例: code="CMP-BASE" / brand="定制" / 不含显卡 / 1800台           │
│  例: code="CMP-5090" / brand="定制" / RTX5090×6 / 150台           │
│  例: code="CMP-4090" / brand="定制" / RTX4090×6 / 150台           │
│  例: code="STO-8TB"  / brand="定制" / 36×8TB / 388台              │
│  例: code="AI-910B"  / brand="华为" / 昇腾910B×8 / N台  ← 扩展    │
└──────────┬────────────────────────────┬──────────────────────────┘
           │ 1:N                        │ 1:N
           ▼                            ▼
┌──────────────────────┐   ┌──────────────────────────────────────┐
│   SpecComponent      │   │           Device（设备实例）            │
│   (规格部件)          │   │  id | spec_id(FK) | status | location │
│                      │   │  asset_code(UK) | serial_no | ...     │
│  id | spec_id(FK)    │   │                                      │
│  kind | model        │   │  每台具体设备独立追踪：                  │
│  capacity_gb         │   │  - 运行状态（运行中/联调中/待上架/已交付）│
│  qty_per_unit        │   │  - 物理位置（机位）                     │
│  sort_order          │   │  - 资产编码                            │
│                      │   │  - 序列号                              │
│  例: RTX5090×6       │   │                                      │
│  例: DC HC320×36     │   │  spec.planned_quantity = count(Device) │
│  例: 昇腾910B×8 ←扩展│   │                                      │
└──────────────────────┘   └──────────────────────────────────────┘
```

### 3.2 设计原则

1. **Spec 定义"一种设备配置"，Device 是"一台具体设备"**
2. **Spec.code 是业务唯一键**（如 `CMP-BASE`、`CMP-5090`），替代当前的四元组
3. **Spec.planned_quantity** = 该规格下 Device 的预期/计划数量（可选，用于初始规划）
4. **Device 独立追踪**每台设备的状态、位置、资产编码
5. **Component 挂载在 Spec 上**，因为部件配置属于规格定义
6. **扩展性**：新增设备类型只需新增 Product + Spec，Component.kind 支持 nup/gpu/accelerator/disk 等

### 3.3 业务唯一键

| 实体 | 业务唯一键 | 说明 |
|------|-----------|------|
| Product | `code` | 如 "CMP-SRV"、"STO-SRV" |
| Spec | `code`（新增） | 如 "CMP-BASE"、"CMP-5090"、"STO-8TB" |
| SpecComponent | `(spec_id, kind, model, capacity_gb)` | 同规格下部件去重 |
| Device | `asset_code` | 资产编码全局唯一（或 `spec_id + serial_no`） |

### 3.4 Dashboard 聚合逻辑

```
总量       = COUNT(Device)                          → 2488 台
运算服务器  = COUNT(Device WHERE spec.product.category = "compute")
存储服务器  = COUNT(Device WHERE spec.product.category = "storage")
各状态数量  = COUNT(Device) GROUP BY status
加速卡汇总  = Σ(COUNT(Device per Spec) × Component.qty_per_unit) WHERE kind = "accelerator"
硬盘汇总    = Σ(COUNT(Device per Spec) × Component.qty_per_unit) WHERE kind = "disk"
存储容量    = Σ(COUNT(Device per Spec) × disk_component.qty_per_unit × capacity_gb)
```

### 3.5 CSV 格式（简化版）

**表头**：
```
产品名称,产品编码,类别,规格编码,品牌,参数,单位,计划数量,部件类型,部件型号,容量,单台数量,备注,排序
```

**规则**：
- parameters 为单个字段，多行内容用双引号包裹（标准 CSV 转义）
- 续行逻辑不变：同一 Spec 的多个 Component 在后续行中仅填写 `规格编码` + 部件列
- 不导入 Device 实例（设备实例通过管理界面单独管理，或导入时选择是否自动生成）

### 3.6 扩展性设计

支持华为昇腾等新型设备的关键设计：

1. **Product.category** 已是 String 类型，可任意扩展（如 "network"、"power"）
2. **Component.kind** 已是 String 类型，`normalize_comp_kind` 已支持 "npu" 映射（[mapping.rs:56-63](file:///e:/GitCode/RF/rust-webx/dmbit/handlers/src/mapping.rs#L56-L63)）
3. **parameters** 作为不透明文本，无需预定义 schema
4. **Dashboard** 的 `accelerator_totals` 可过滤 `kind = "accelerator"` 或 `kind = "npu"`，`disk_totals` 过滤 `kind = "disk"`
5. 如需新增 Dashboard 卡片（如 NPU 汇总），在 DashboardModel 中新增 `npu_totals` 字段即可

---

## 四、实体变更清单

### 4.1 新增实体

#### Spec（设备规格）
```rust
#[table("specs")]
pub struct Spec {
    pub id: String,              // UUID PK
    pub product_id: String,      // FK → Product
    pub code: String,            // UK，规格编码，如 "CMP-BASE"
    pub brand: String,           // 品牌短码
    pub parameters: String,      // 多行键值文本
    pub unit: String,            // 单位（台/套）
    pub planned_quantity: i32,   // 计划数量（可选）
    pub sort_order: i32,
    pub created_id: Option<String>,
    pub created_at: i64,
    pub updated_id: Option<String>,
    pub updated_at: i64,
    pub is_deleted: bool,
    // navigation
    pub product: BelongsTo<Product>,
    pub components: HasMany<SpecComponent>,
    pub devices: HasMany<Device>,
}
```

#### Device（设备实例）
```rust
#[table("devices")]
pub struct Device {
    pub id: String,              // UUID PK
    pub spec_id: String,         // FK → Spec
    pub status: String,          // 运行中/联调中/待上架/已交付
    pub location: String,        // 机位
    pub asset_code: String,      // UK, 资产编码
    pub serial_no: String,       // 序列号（可选）
    pub sort_order: i32,
    pub created_id: Option<String>,
    pub created_at: i64,
    pub updated_id: Option<String>,
    pub updated_at: i64,
    pub is_deleted: bool,
    // navigation
    pub spec: BelongsTo<Spec>,
}
```

#### SpecComponent（重命名自 GoodsComponent）
```rust
#[table("spec_components")]
pub struct SpecComponent {
    pub id: String,
    pub spec_id: String,         // FK → Spec（原 goods_id）
    pub kind: String,            // accelerator/disk/npu/...
    pub model: String,
    pub capacity_gb: i64,
    pub qty_per_unit: i32,
    pub sort_order: i32,
    // ... audit fields
    pub spec: BelongsTo<Spec>,
}
```

### 4.2 删除/修改的实体

| 实体 | 变更 |
|------|------|
| `Goods` | **删除**，拆分为 Spec + Device |
| `GoodsComponent` | **重命名**为 `SpecComponent`，`goods_id` → `spec_id` |
| `Product` | 保持不变 |

---

## 五、API 路由变更清单

### 5.1 Spec（替代原 Goods API）

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/specs` | 规格列表 |
| GET | `/api/products/{id}/specs` | 某产品下的规格 |
| GET | `/api/specs/{id}` | 规格详情（含部件+设备摘要） |
| POST | `/api/specs` | 创建规格 |
| PUT | `/api/specs/{id}` | 更新规格 |
| DELETE | `/api/specs/{id}` | 删除规格（级联删除部件，设备标记为"已淘汰"） |

### 5.2 Device（新增）

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/devices` | 设备列表（支持 spec_id/status/location 筛选） |
| GET | `/api/specs/{id}/devices` | 某规格下的设备 |
| GET | `/api/devices/{id}` | 设备详情 |
| POST | `/api/devices` | 创建设备实例 |
| POST | `/api/specs/{id}/devices/generate` | 根据 planned_quantity 批量生成设备 |
| PUT | `/api/devices/{id}` | 更新设备状态/位置/资产编码 |
| DELETE | `/api/devices/{id}` | 删除设备实例 |

### 5.3 Inventory（变更）

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/inventory/export` | CSV 导出（Spec 规格清单，不含 Device） |
| POST | `/api/inventory/import` | CSV 导入（Spec 规格定义） |
| POST | `/api/inventory/import-with-devices` | CSV 导入并自动生成 Device |

### 5.4 不变的 API

| 方法 | 路径 | 说明 |
|------|------|------|
| Product CRUD | `/api/products` | 保持不变 |
| Auth | `/api/auth/*` | 保持不变 |
| Dashboard | `/api/dashboard` | 聚合逻辑改为基于 Device |

---

## 六、实施计划

### Phase A：数据模型（核心架构变更）

| 步骤 | 内容 | 涉及文件 |
|------|------|---------|
| A1 | 新增 Spec/Device/SpecComponent 实体 | `domain/src/entities/spec.rs`, `device.rs`, `spec_component.rs` |
| A2 | 新增 DTO（SpecModel/DeviceModel/SpecComponentModel） | `contracts/src/spec.rs`, `device.rs` |
| A3 | 新增软删除过滤器 | `domain/src/filters.rs` |
| A4 | 删除 Goods/GoodsComponent 实体，迁移导航关系 | `domain/src/entities/` |

### Phase B：业务逻辑

| 步骤 | 内容 | 涉及文件 |
|------|------|---------|
| B1 | Spec CRUD Handler | `handlers/src/spec.rs` |
| B2 | Device CRUD Handler（含批量生成） | `handlers/src/device.rs` |
| B3 | Dashboard 重写（基于 Device 聚合） | `handlers/src/dashboard.rs` |
| B4 | 移除 mapping.rs 中的 Goods 相关函数，新增 Spec 映射 | `handlers/src/mapping.rs` |

### Phase C：CSV 导入导出重构

| 步骤 | 内容 | 涉及文件 |
|------|------|---------|
| C1 | 简化 CSV 格式（parameters 不分列，新增 spec_code 列） | `handlers/src/inventory.rs` |
| C2 | 导出逻辑（Spec + SpecComponent） | `handlers/src/inventory.rs` |
| C3 | 导入逻辑（两阶段确认保留） | `handlers/src/inventory.rs` |
| C4 | 导入自动生成 Device 功能 | `handlers/src/inventory.rs` |

### Phase D：质量改进

| 步骤 | 内容 | 涉及文件 |
|------|------|---------|
| D1 | 状态值统一定义到 contracts 层 | `contracts/src/constants.rs`（新增） |
| D2 | replace_components → 物理删除旧记录 | `handlers/src/mapping.rs` |
| D3 | load_components_for → IN 查询 | `handlers/src/mapping.rs` |
| D4 | Dashboard category 匹配改为显式 match | `handlers/src/dashboard.rs` |
| D5 | 软删除唯一约束方案 | `domain/src/entities/product.rs` |

### Phase E：前端适配

| 步骤 | 内容 | 涉及文件 |
|------|------|---------|
| E1 | 管理后台适配 Spec/Device API | `wwwroot/assets/js/admin.js` |
| E2 | 大屏适配新 Dashboard 数据结构 | `wwwroot/assets/js/screen.js` |
| E3 | CSV 导入导出界面更新 | `wwwroot/admin/index.html` |

---

## 七、CSV 格式对比

### 当前格式（24 列）
```
产品名称,产品编码,类别,品牌短码,状态,数量,单位,机位,资产编码,
机箱,主板,内存,接口,扩展,电源,光模块,其他参数,
部件类型,部件型号,容量,单台数量,备注,产品排序,台账排序
```

### 新格式（14 列）
```
产品名称,产品编码,类别,规格编码,品牌,参数,单位,计划数量,
部件类型,部件型号,容量,单台数量,备注,排序
```

**变更说明**：
- 移除 `状态/机位/资产编码`（属于 Device 实例）
- 移除 `机箱/主板/内存/接口/扩展/电源/光模块/其他参数` 拆分列 → 合并为 `参数`
- 新增 `规格编码` 作为 Spec 的业务唯一键
- `数量` → `计划数量`（语义更明确）

### 示例数据
```csv
产品名称,产品编码,类别,规格编码,品牌,参数,单位,计划数量,部件类型,部件型号,容量,单台数量,备注,排序
运算服务器,CMP-SRV,算力,CMP-BASE,定制,"机箱：4U服务器机箱
主板：嵌入式工业级主板
内存：SO-DIMM内存
扩展：6槽位PCIe扩展板
电源：定制多路输出电源
光模块：双光千兆",台,1800,,,,,,
,CMP-SRV,,CMP-5090,定制,"机箱：4U服务器机箱
扩展：RTX5090×6",台,150,加速卡,RTX5090,,6,,0
,CMP-SRV,,CMP-4090,定制,"扩展：RTX4090×6",台,150,加速卡,RTX4090,,6,,0
存储服务器,STO-SRV,存储,STO-8TB,定制,"CPU：Intel Xeon
硬盘：36块8TB DC HC320",台,388,硬盘,DC HC320,8TB,36,,0
```

---

## 八、扩展性验证：新增华为昇腾设备

### 场景
机房新增 200 台搭载华为昇腾 910B 的 AI 训练服务器。

### 操作
1. 创建 Product：`name="AI训练服务器"`、`code="AI-SRV"`、`category="compute"`
2. 创建 Spec：`code="AI-910B"`、`brand="华为"`、`parameters="..."`、`planned_quantity=200`
3. 创建 SpecComponent：`kind="npu"`、`model="昇腾910B"`、`qty_per_unit=8`
4. CSV 导入或界面操作批量生成 200 台 Device
5. Dashboard 自动将 NPU 计入加速卡汇总

**无需修改任何代码**，因为：
- Product.category 是 String，可自由取值
- Component.kind 已支持 "npu"（normalize_comp_kind 映射到 "accelerator"）
- parameters 是不透明文本，无预定义 schema
- Dashboard 聚合按 kind 过滤，NPU 自然归入 accelerator_totals

如果需要 Dashboard 单独展示 NPU 汇总卡片，只需在 DashboardModel 添加 `npu_totals` 字段 —— 属于 **纯粹的数据聚合扩展，不改变架构**。
