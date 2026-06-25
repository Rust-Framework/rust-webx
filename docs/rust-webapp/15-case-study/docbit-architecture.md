# 架构与模块划分

## 源码结构

```
docbit/src/
├── main.rs           # 组合根：Host 配置 + DI 注册
├── startup.rs        # DbInitService：迁移 + 种子 + 文档索引
├── common/           # 共享工具
├── contracts/        # API 契约
│   ├── auth.rs       # LoginRequest, RegisterRequest, AuthMeRequest
│   ├── blog.rs
│   ├── docs.rs       # 文档 API 契约
│   ├── site.rs
│   ├── user.rs
│   └── work.rs
├── handlers/         # 处理器
│   ├── auth.rs       # inject_attr + #[handler(inject)]
│   ├── blog.rs
│   ├── docs.rs
│   ├── site.rs
│   ├── user.rs
│   └── work.rs
├── services/         # 领域服务
│   ├── docs.rs       # DocService：文件系统文档扫描
│   └── site.rs
└── domain/           # 实体 + 迁移
    ├── user.rs
    ├── blog.rs
    ├── work.rs
    └── migrations/
```

## main.rs 分析

```rust
let host = Host::builder()
    .mode(AppMode::Development)
    .register(move |svc| {
        // 注册 DbContext 和 DocService
        svc.singleton::<Mutex<DbContext>>(...);
        svc.singleton::<DocService>(...);
    })
    .use_spa("wwwroot")
    .use_auth()
    .use_memory_cache()
    .build();

host.run().await?;
```

`main.rs` 只做三件事：
1. 创建框架外的依赖（DbContext、DocService）
2. 配置 Host 能力（SPA、Auth、Cache）
3. 启动

所有 Handler 通过 `inject_attr` 自动注册，无需在 main 中逐个列出。

## startup.rs 分析

`DbInitService` 实现 `IHostedService`：

1. 运行数据库迁移
2. 种子管理员账户
3. 种子作品集和博客数据
4. 生成文档 INDEX.json

这是「应用初始化」的标准模式——不在 `main()` 中写初始化逻辑。

## 数据流示例：用户登录

```
POST /api/auth/login
    → contracts/auth.rs (LoginRequest 反序列化)
    → handlers/auth.rs (LoginHandler)
        → 查询数据库验证密码
        → bcrypt::verify
        → jsonwebtoken::encode 签发 Token
    → 返回 AuthResponse { token, user }
```

## 小结

Docbit 的架构是 rust-webapp 推荐结构的完整实例化，可直接作为新项目的模板。

下一节：[可复用的模式提炼](docbit-patterns.md)
