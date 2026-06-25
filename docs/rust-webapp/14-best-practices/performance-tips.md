# 性能优化技巧

## Handler 缓存

框架在 `HandlerCache` 中缓存 Handler 解析结果，避免每次请求重新查 DI 容器。

## 路由 Trie

`Router` 使用 Trie 树匹配，时间复杂度 O(path_segments)，优于线性扫描。

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
cargo bench -p rust-webapp-host
```

## 小结

默认性能已足够大多数 WebApi 场景；瓶颈通常在数据库和外部服务，而非框架本身。

下一节：[AI 友好开发模式](ai-friendly-development.md)
