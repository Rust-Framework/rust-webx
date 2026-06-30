# LRDI 测试指南

## 一、测试哲学

LRDI 的核心价值之一是**可测试性**。通过 DI 容器，你可以在测试中用 mock 实现替换真实服务，而无需修改被测试的代码。

## 二、Mock 替换模式

### 2.1 基础模式：替换单个依赖
```rust
trait Database: Send + Sync {
    fn query(&self, sql: &str) -> Vec<String>;
}

struct RealDatabase;
impl Database for RealDatabase {
    fn query(&self, sql: &str) -> Vec<String> { /* 真实数据库查询 */ vec![] }
}

struct MockDatabase;
impl Database for MockDatabase {
    fn query(&self, _sql: &str) -> Vec<String> { vec!["mock_result".into()] }
}

struct UserService { db: Arc<dyn Database> }

// 生产环境
fn prod_provider() -> ServiceProvider {
    ServiceCollection::new()
        .singleton::<dyn Database>(|_| Arc::new(RealDatabase))
        .transient(|p| Arc::new(UserService { db: p.get() }))
        .build().unwrap()
}

// 测试环境
fn test_provider() -> ServiceProvider {
    ServiceCollection::new()
        .singleton::<dyn Database>(|_| Arc::new(MockDatabase))
        .transient(|p| Arc::new(UserService { db: p.get() }))
        .build().unwrap()
}

#[test]
fn test_user_service() {
    let p = test_provider();
    let svc: Arc<UserService> = p.get();
    // 断言 mock 行为
}
```

关键：注册为 dyn Database，生产用 RealDatabase，测试用 MockDatabase。UserService 代码完全相同。

### 2.2 进阶模式：状态化 Mock

使用 AtomicUsize 或 Mutex 让 mock 携带调用计数、参数记录：

```rust
struct SpyDatabase {
    call_count: AtomicUsize,
    last_query: Mutex<String>,
}
impl Database for SpyDatabase { /* ... */ }
```

### 2.3 模式：部分替换

只替换需要 mock 的依赖，其余依赖保持真实：
- 数据库 → Mock（需要模拟）
- 日志 → RealLogger（不需要模拟，轻量）
- 缓存 → RealCache（不需要模拟）

## 三、测试辅助函数

### 3.1 通用测试 Provider 构建器
提供 build_test_provider 函数，接收要 mock 的服务，其余用默认值。

### 3.2 cfg(test) 模块
在模块内用 #[cfg(test)] 定义测试专用的 provider 构建函数。

### 3.3 测试 fixtures
使用 std::sync::Once 或 lazy_static 构建共享的测试 provider。

## 四、集成测试 vs 单元测试

### 单元测试
- 每个测试构建独立的 ServiceProvider
- 只注册被测试的服务和其直接依赖（mock 化）
- 快速、隔离

### 集成测试
- 构建完整的 ServiceProvider（真实实现）
- 使用 test database / test config
- 测试服务间的实际协作

## 五、Scope 测试

展示如何测试 Scoped 生命周期：
- 验证同一 scope 内 scoped 服务缓存
- 验证不同 scope 间 scoped 服务独立
- 验证 Scope drop 后行为

## 六、ServiceProviderWrapper 测试

展示如何测试分层容器：
- child 优先解析
- root fallback
- child 对 root 不可见

## 七、常见测试反模式

1. 不要每次测试都从头构建复杂 provider——提取公共构建函数
2. 不要 mock 一切——只 mock 外部依赖（数据库、网络、文件系统）
3. 不要在测试中测试 LRDI 本身——测试你的业务逻辑
4. 不要忘记测试服务未注册的错误路径
