# 分配预算（Allocation Budget）

> 本文是原则 P4「性能是预算」在分配维度的落地口径。基线：rust-webx 2.5.0。
> 工具：`crates/alloc-probe`（dev-only，不发布）。

## 1. 测量方法

- **计数点**：`GlobalAlloc` 包装，统计 `alloc + realloc`；框架 crate 保持
  `unsafe_code = "forbid"`，因此探针必须外置。
- **两进程**：计数器在 server 子进程；负载客户端在父进程，客户端的分配不计入。
- **两种模式**（同一 hyper 版本、同一 worker 数、同一响应体 `{"message":"Hello, World!"}`）：

  | 模式 | 说明 |
  |---|---|
  | `framework` | 真实 `Host`：Production、1 worker、metrics off、默认 RequestId、CORS on、`#[get("/hello")]` |
  | `hyper-baseline` | 裸 hyper server：只做 HTTP 解析 + 固定响应写出，代表**无法再降的地板** |

- **窗口**：50 次预热 → 500 次请求；计数在窗口前后读取，
  `allocs/request = Δallocs / (Δrequests + 1)`（+1 为窗口末尾的 stats 请求）。

## 2. 当前基线

同机、同窗口、release（thin LTO）实测：

| 模式 | allocs/request |
|---|---:|
| hyper 基线（无框架） | **6.00** |
| 框架（CORS on） | **16.42** |
| **框架税 = 总分配 − 地板** | **10.42** |

地板随 hyper 版本移动，因此预算必须按 **地板 + 框架税** 定义；CI 比较框架税，不硬编码总数。

## 3. 预算与门禁

| 项 | 上限 |
|---|---:|
| 框架税 | **≤12**（当前 10.42） |
| 对应总分配（地板 6.00） | ≤18 |

```text
cargo run -p alloc-probe --release --locked -- compare 500 12
```

`compare` 同窗口测 framework 与 hyper 地板，输出 `framework=.. baseline=.. tax=..`；
`tax > 12` 退出码 1。

复现单模式：

```text
cargo run -p alloc-probe --release -- 18471 500            # framework
cargo run -p alloc-probe --release -- 18471 500 baseline   # hyper 地板
ALLOC_PROBE_NO_CORS=1 cargo run -p alloc-probe --release -- 18471 500
```

## 4. 当前构成

框架税 10.42 的主要来源（按代码结构）：

| 来源 | 说明 |
|---|---|
| 响应体 `Bytes` | 动态 JSON 必然至少 1 次分配 |
| 请求交接与 dispatch bridge | handler/请求/响应的装箱、宏 dispatch future |
| request-id 与响应头 | UUID 字符串 + `HeaderValue` |
| 引擎上下文 | 路由参数/header 视图的零散分配 |

## 5. 后续方向（非 2.x 承诺）

达到税 ≤8 需要改变公开形状或分发 ABI，记入下一个 major 评估：

- 响应体具体化为 enum，去掉每响应 `BoxBody` 分配；
- 直接分发改 poll 式 future，去掉每请求 dispatch box；
- 路由级 inline 绑定后按值交接请求，去掉 `Box<dyn Any>`。

这些都属于破坏性窗口，2.x 不实现。
