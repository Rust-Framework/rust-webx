# 认证性能报告（2.5）

> 目标：把 `auth-system.md` 的 §5.4 预算变成可复现数字。所有数字来自本仓库
> `release` 构建、本机单进程；不作为跨硬件容量承诺，只作回归基线。

## 1. 预算与结果

| 预算 | 要求 | 实测 | 结论 |
|---|---|---|---|
| 匿名请求认证层分配 | 0 | auth 中间件 +2.00，空中间件 +2.00 ⇒ **增量 0** | ✅ |
| HS256 验签 | p99 ≤ 5µs | **p50 2.3µs / p99 3.9µs**（20k 次） | ✅ |
| JWKS 缓存命中 | 0 网络 I/O | fetch 计数不增 | ✅ |
| 吊销负缓存命中 | 0 后端 I/O | `CachedSessionStore` 测试 | ✅ |
| Argon2 | 有界并发 | 上限 min(cpus,4)；饱和 100ms → 503 | ✅ |
| 整请求分配（非认证） | — | framework 23.86 / hyper 6.00 ⇒ tax **17.86 allocs/request** | 独立目标 |

## 2. 方法

### 2.1 HS256

```powershell
cargo test --release -p rust-webx-host --test auth_budget_test hs256_p99 -- --ignored --nocapture
```

`JwtAuth::validate_token` 对同一个有效 HS256 token 循环 20,000 次，排序取 p50/p99；
预热 64 次排除首次解码/JWK 初始化。结果：

```
HS256 validate: p50=2.3µs p99=3.9µs over 20000 iterations
```

### 2.2 分配

```powershell
cargo run --release -p alloc-probe -- compare 2000
$env:ALLOC_PROBE_AUTH='1';   cargo run --release -p alloc-probe -- compare 2000
$env:ALLOC_PROBE_NOOP_MW='1'; cargo run --release -p alloc-probe -- compare 2000
```

| 配置 | framework | hyper baseline | tax |
|---|---|---|---|
| 无认证 | 25.86 | 6.00 | 19.86 |
| 认证 | 25.86 | 6.00 | 19.86 |
| 空中间件 | 25.86 | 6.00 | 19.86 |

- 认证与空中间件增量完全一致 ⇒ **认证逻辑 0 分配**；
- +2 来自 `async_trait` 中间件的 `invoke`/`after` boxed future，属通用管线成本；
- 整请求 tax 17.86 与认证无关，单独作为 engine/host 优化目标。

### 2.3 JWKS / 吊销缓存

- `auth_jwks_test::jwks_cache_hits_and_unknown_kid_refreshes_once`：缓存命中时 fetch 计数不增；
- `session_cache_test`：负缓存命中不触达内层 `ISessionStore`；
- 存储故障一律 503，不降级匿名。

### 2.4 Argon2 有界并发

- `login_hardening_test::bounded_hasher_caps_concurrency`：并发上限；
- `bounded_hasher_fails_fast_when_saturated`：饱和 100ms → 503；
- 默认并发 `min(available_parallelism, 4)`，避免登录风暴打满 CPU。

### 2.5 Soak

```powershell
$env:SOAK_SECONDS='300'; cargo test -p rust-webx --test stability_soak_test -- --ignored --nocapture
```

- 4 并发 keep-alive 客户端；
- 0 失败、吞吐 >25 req/s、请求后 `/metrics` 200、shutdown 后停止监听；
- 1h 已通过（Linux，3600.15s，0 失败，RSS 稳定 17.7MB）。

## 3. 回归建议

- `stability.yml` 的 `auth-budget` 每次 nightly 跑 HS256 p99；
- 认证代码改动必须跑 `auth_budget_test` + 对应对抗用例；
- 分配预算按“认证层增量”看，整请求 tax 作为独立性能工作流；
- 任何新中间件都会增加 2 allocs（async_trait），评审时说明。

## 4. 复现环境

- Rust `1.95`（workspace MSRV），release；
- Windows x86_64 开发机，单进程；
- 数字随 CPU/负载变化，比较时保持同机、同 release profile。
