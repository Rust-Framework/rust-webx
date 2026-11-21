# 分配预算（Allocation Budget）

> 基线：rust-webx 2.2 开发期（`v2.2` 分支工作树）。工具：`crates/alloc-probe`。
> 本文是"性能是预算"（宪法 P4）在分配维度的落地口径。

## 1. 测量方法

- **计数点**：独立 crate `crates/alloc-probe` 内的 `GlobalAlloc`，统计 `alloc + realloc`。
  框架 crate 继续 `unsafe_code = "forbid"`，因此探针必须外置，不能混进框架。
- **两进程**：计数器在 **server 子进程**；负载客户端在父进程，客户端的分配不进入计数。
- **两种模式**（同一 hyper 版本、同一 worker 数、同一响应体 `{"message":"Hello, World!"}`）：
  | 模式 | 说明 |
  |---|---|
  | `framework` | 真实 `Host`：Production、1 worker、metrics off、默认 RequestId、CORS on、`#[get("/hello")]` |
  | `hyper-baseline` | 裸 hyper server：只做 HTTP 解析 + 固定响应写出，代表**无法再降的地板** |
- **窗口**：50 次预热 → 500 次请求；`/__stats` 在窗口前后读取计数；
  `allocs/request = Δallocs / (Δrequests + 1)`（+1 为窗口末尾的 stats 请求）。

```text
cargo run -p alloc-probe --release -- 18471 500            # framework
cargo run -p alloc-probe --release -- 18471 500 baseline   # hyper 地板
ALLOC_PROBE_NO_CORS=1 cargo run -p alloc-probe --release -- 18471 500
```

## 2. 实测数据（2026-11，Windows / Intel Ultra 7，release，thin LTO）

500 请求窗口实测（2026-11，release，thin LTO）：

| 模式 | allocs/request |
|---|---:|
| hyper 基线（无框架） | **6.00** |
| 框架（CORS on）——2.2 基线 | 23.42 |
| 框架（CORS on）——2.4-9 优化后 | **16.42** |
| 框架（CORS off，2.2 对照） | 20.42 |

**框架税 = 总分配 − hyper 地板**：

| 口径 | 2.2 基线 | 2.4-9 优化后 |
|---|---:|---:|
| 框架税（CORS on） | 17.42 | **10.42** |
| 框架税（CORS off） | 14.42 | — |

## 3. 结论：预算必须按"地板 + 框架税"定义

原始目标"总分配 ≤12"隐含 **hyper 地板为 0**；实测地板是 **6.00**，即框架只剩 6 个分配的预算。
当前框架税 17.4 的主要来源（按实测与代码结构推断）：

| 来源 | 估算 | 说明 |
|---|---:|---|
| 中间件链 `async_trait` box | 2–3 | 每个 `IMiddleware::invoke` 一次 boxed future；CORS 单独实测 3.0（含 header 处理） |
| 路由分发 | 3–5 | `RouteTarget::handle` box、宏桥 boxed future、每请求 handler/请求/响应 box |
| JSON 编码与头部值 | 2–3 | 响应缓冲、`content-type` 等头部 |
| 框架初始化 | 1–2 | request id（优化后 1）等 |

因此预算改为：

| 版本 | 框架税上限 | 总分配（地板 6.0） | 依据 |
|---|---:|---:|---|
| **v2.2** | **≤18** | ≤24 | 不回归（当时 17.42），并允许 CI 测量噪声 |
| **v2.4** | **≤12（已达成 10.42）** | ≤18 | 内置中间件与直接分发去 box 后落地；CI 门禁从 18 收紧到 12 |
| **v2.4 拉伸目标** | ≤8 | ≤14 | **未达成**，剩余差距见 §6 |

> 地板随 hyper 版本变化，因此 CI 必须**同窗口测两种模式**并比较框架税，而不是硬编码总分配。

## 4. 已完成的优化（v2.2-4a）

- **request-id 头零拷贝交接**：`IHttpResponse::set_header_owned(String)` + 引擎
  `HeaderValue::from_maybe_shared(Bytes::from(value))`，省掉一次 String→HeaderValue 复制（−1.0）。
- **JSON 编码预分配**：`webx_core::json::encode_json` 以 256B 起步，常见 DTO 一次分配完成；
  大响应正常增长。`JsonResult`、`write_json_response`、路由宏编码器统一走它。

## 5. CI 门禁（v2.2-8a）

`.github/workflows/ci.yml` 的 `alloc` job 运行：

```text
cargo run -p alloc-probe --release --locked -- compare 500 18
```

`compare` 在同一台机器上先测 framework 再测 hyper 地板，输出
`framework=.. baseline=.. tax=..`，`tax > 18` 时退出码 1。同一机器两次实测：
`tax=17.42`、`tax=17.42`（稳定）。门禁比较**框架税**而不是绝对总数，因此 hyper
升级只会移动地板，不会误报。

## 6. v2.4-9 已完成的优化（17.42 → 10.42）与剩余差距

已落地（默认管道，按实测贡献排序）：

1. **内置中间件同步快路径**：`IMiddleware::fast_invoke`（默认 `None`）；
   `RequestIdMiddleware`/`CorsMiddleware` 作为纯 header 变换走同步实现，去掉
   `async_trait` 的 boxed future（每个内置阶段 −1）。
2. **`has_after`**：无 `after` 钩子的内置阶段跳过 boxed `after` future（−2）。
3. **CORS 无 Origin 快速路径**：请求没有 `Origin` 时不做任何 CORS 处理（不 append
   `Vary`、不查允许列表），CORS 阶段从 3.0 降到 ~0。
4. **直接路由分发**：`RouteTarget::direct()` 让无授权/无路由 body cap 的
   `DirectRouteTarget` 由引擎直接调用 dispatch 函数，跳过 `async_trait` box（−1）。
5. **无装箱请求绑定**：`RequestModel::bind_inline`（RPITIT，返回 `impl Future + Send`），
   derive 直接生成绑定体，端点直连调用，去掉一层 boxed future（−1）。

剩余 ~10.4 的构成（估算）：hyper 地板 6.0 之外，主要是每请求响应体 `Bytes`（动态 JSON
必然 1）、请求交接的 `Box<dyn Any>` 与宏 dispatch bridge 的 boxed future（2–3）、
request-id 的 UUID 字符串与响应头（1–2）、引擎上下文的零散 header/路由状态（1–2）。

**走向 ≤8 的路径（需要破坏性窗口，记入 2.5/3.0 计划）**：

- `RespBody` 具体化为 enum（`Full<Bytes>` 快变体）去掉 `BoxBody` 的每响应 1 次分配——
  公开类型别名变更，须在 3.0 窗口；
- 直接分发 ABI 改 poll 式 future（宏与 `HandlerDispatchFn` 联合调整）以去掉每请求
  dispatch box；
- 路由级 inline 绑定后把请求按值交给 dispatch（去掉 `Box<dyn Any>` 交接）。

## 7. CI 门禁

```text
cargo run -p alloc-probe --release --locked -- compare 500 12
```

`compare` 同窗口测 framework 与 hyper 地板并只比较框架税；2.4 起上限 12（实测 10.42）。
