# AI 友好开发模式

## 为什么 rust-webx 适合 AI 辅助开发

### 1. 请求即边界

每个 API 端点由独立的 Request + Handler 组成，AI 可以一次生成一个完整模块：

```
请为 rust-webx 创建「获取博客文章列表」端点：
- 路由：GET /api/blog/posts
- 支持分页
- 返回 PagedResponse<BlogPostDto>
```

AI 生成的代码可直接放入 `contracts/blog.rs`（DTO + `IBlogService` trait + `IRequest`）+ `handlers/blog.rs`（`BlogService` 实现 + Handler）。

**contracts 禁止引用 domain**；DTO 必须在 contracts 定义。

### 2. 强约定减少歧义

| 约定 | AI 收益 |
|------|--------|
| 四步端点模式 | 生成模板固定 |
| contracts/handlers 分层 | 文件放置明确 |
| `#[handler]` 自动注册 | 无需修改 main.rs |
| `Result<Error>` 错误处理 | 无需猜测错误模式 |

### 3. 编译器验证 AI 输出

类型不匹配、遗漏注册等问题在 `cargo check` 时暴露，AI 可根据编译错误自我修正。

## 推荐 AI 协作工作流

```
1. 描述需求（含路由、输入输出类型）
2. AI 生成 contracts + handlers
3. cargo check 验证
4. 根据编译错误迭代
5. 集成测试验证
```

## 单文件单端点模式

大型项目可为每个端点创建独立文件，便于 AI 并行生成：

```
handlers/blog/
├── list_posts.rs
├── get_post.rs
├── create_post.rs
└── mod.rs
```

## 提示词模板

```
使用 rust-webx 框架，遵循以下约定：
- contracts/：Request、Response DTO、enum、I…Service trait（仅依赖框架，禁止引用 domain）
- handlers/：IRequestHandler 实现 + I…Service 实现（inject_attr 自动注册）
- domain/：实体与迁移（可引用 contracts 枚举）
- 使用 #[get]/#[post] 标注 impl IRequest<T>
- Handler 注入 Arc<dyn I…Service>，使用 inject_attr + #[handler(inject)]
- 错误使用 Error::NotFound / Error::Validation
- 返回 Result<T>

请实现：{你的需求描述}
```

## 小结

框架的强约定和类型安全使 AI 生成的代码质量远高于自由风格项目。

下一节：[代码审查清单](code-review-checklist.md)
