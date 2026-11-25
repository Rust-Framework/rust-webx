# 性能优化技巧

## Handler 构造

`HandlerCache` 缓存的是编译期注册的 **工厂 + 调用桥**（按请求类型 `TypeId` 查找），不是 Handler 实例：每个请求用请求作用域构造一个 owned Handler，因此 Scoped 依赖（如 DbContext）是全新的。它省掉的是“扫描/反射找 Handler”，不是 DI 解析本身。

## 路由匹配

`Router` 基于 `matchit` 的基数树（radix tree）匹配。静态路由匹配本身不分配；动态路由会把参数复制进请求上下文（每个参数一个值 `String`，2.x 还会为参数名分配 `String` 并建一张 name→value 表），因此不要宣称“请求期零分配”。实测（`cargo bench -p rust-webx-engine --bench router_bench`，Windows 参考）：静态路由约 **58 ns**、1 个参数约 **200 ns**、3 个参数约 **360 ns**、catch-all 约 **208 ns**；参数路由比静态路由多出的 ~140 ns/参数就是上面这些分配，下一破坏性版本计划改为借用切片后消除。

## Arc 共享

- 所有 Singleton 服务通过 `Arc` 共享，避免克隆大对象
- 数据库连接池、缓存客户端注册为 Singleton

## 避免阻塞

```rust
// ✅ 异步数据库操作
let user = self.repo.find(&id).await?;

// ❌ 在 async 上下文中阻塞
let user = std::thread::spawn(|| db.query()).join()?;
```

## 缓存热点数据

```rust
self.cache.get_or_create("config:site", async {
    self.load_site_config().await
}).await?;
```

## Release 构建

```bash
cargo build --release
```

Workspace 已配置 LTO + strip，release 二进制体积和性能最优。

## 基准测试

框架提供 benchmark：

```bash
cargo bench -p rust-webx-host
```

## 小结

默认性能已足够大多数 WebApi 场景；瓶颈通常在数据库和外部服务，而非框架本身。

下一节：[AI 友好开发模式](ai-friendly-development.md)
