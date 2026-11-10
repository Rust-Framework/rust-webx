# 升级到 0.9（破坏性变更）

0.9.0 的主题是**把“默认安全、声明即契约”落到框架内部**：修复 0.8.0 对抗评审发现的
23 条缺陷（安全问题、协议一致性、身份模型、错误方向、扩展点可组合性）。多数改动是行为
修正；下面按“需要改代码”与“行为变化”分类。

## 必须改代码的 Breaking

### 1. 匿名身份必须显式声明

`Option<IdentityUser>` 不再能只写 `#[fromclaim]`：

```rust
// 0.8
#[fromclaim]
pub viewer: Option<IdentityUser>,

// 0.9
#[fromclaim(optional)]
pub viewer: Option<IdentityUser>,
```

- 只写 `#[fromclaim]` 会编译失败，错误信息直接给出 `#[fromclaim(optional)]`；
- 一个请求类型只能有**一个**身份字段（`user` + `viewer` 同时存在会编译失败）；
- 动机：0.8 里“可选登录”写错就变成完全公开，`require_user()` 忘调即越权。

### 2. `RequestContext::run` 接收完整身份

```rust
// 0.8
RequestContext::run(operator_id: Option<String>, fut)

// 0.9
RequestContext::run(identity: Option<Arc<IdentityUser>>, fut)
```

框架的 HTTP dispatch 已在绑定请求字段**之前**建立上下文，业务代码无需改动；
自己调用 `run` 的代码传入 `ctx.claims()` 构造的 `Arc<IdentityUser>` 即可。
`operator_id()` 保留（从身份派生）。

### 3. 中间件的默认槽位改为“认证之后”

`use_middleware` / `use_middleware_with` 注册的中间件从“CORS/JWT 之前”移到
**AfterAuth 槽**，因此 `ctx.claims()` 能读到身份（框架自带 `RequestTracing` 因此
不再恒为 `anon`）。需要其它位置时用新 API：

```rust
use webx::MiddlewareSlot;

Host::builder()
    .use_middleware_at::<PreflightAudit>(MiddlewareSlot::BeforeCors)
    .use_middleware::<TenantGuard>()                 // 默认 AfterAuth
    .use_middleware_with_at(MiddlewareSlot::AfterSpa, || Arc::new(Hook::new()))
    .build();
```

直接注册进 DI 的 `IMiddleware` 也进入 AfterAuth 槽（排在 builder 注册之后）。

### 4. OpenAPI 元数据以 `TypeId` 为键（自定义 entry 需要跟进）

`#[derive(WebxRequestMeta)]` 与路由宏自动完成；只有**手写** `RouteEntry` /
`RequestParamEntry` 的代码需要更新：

```rust
// 旧式 entry 仍可用（按名字回退），但希望参数元数据精确匹配时：
RouteEntry::with_type_id(method, path, TypeId::of::<MyRequest>(), ...)
```

迁移后限定路径（`impl IRequest<T> for module::MyRequest`）与跨 crate 同名 Request
不再丢参数或串台。

## 行为变化（通常无需改代码，但客户端/部署要知悉）

| 变化 | 0.8 | 0.9 |
|------|-----|-----|
| `APP_ENV` 未设置 | 默认 `Development` | **默认 `Production`**；本机开发显式 `APP_ENV=Development` |
| 无效/过期 token | 全站 401（登录接口也会被锁死） | 视为未认证：公开端点继续，受保护端点仍 401 |
| 401 响应 | 无 `WWW-Authenticate`，四种文案 | 统一 `application/problem+json` + `WWW-Authenticate: Bearer realm="rust-webx"`，detail 稳定为 `authentication required` |
| 405 响应 | 无 `Allow` | 带 `Allow: GET, PUT, HEAD` |
| plain `OPTIONS` | 任意路径 204 | 已知路径 204 + `Allow`，未知路径 404；仅真预检（`Origin` + `Access-Control-Request-Method`）由 CORS 短路 |
| 5xx 响应 detail | 内部错误原文（含文件路径） | 稳定文案；内部原文只进日志（`tracing::error!`） |
| 响应序列化失败 | 400 | 500（`Error::ResponseSerialization`） |
| SPA `index.html` 回退 | 路由之前，任何 GET | 路由未命中之后，且仅 `Accept: text/html` |
| 413 超大请求 | 跳过整条中间件管道 | 管道内错误：CORS/安全头/日志/指标均可见 |
| `IHostedService::stop` | 与启动同序、无超时 | **逆序**停止、每个 30s 超时；`start` 失败逆序回滚 |
| `IHostedService::start/stop` 的 DI | 拿不到 scope（Scoped 依赖落 root） | 每次调用一个独立 scope，结束后释放 |
| 内嵌资源压缩 | brotli quality 11（大资产构建极慢） | 默认 quality 9，`WEBX_BROTLI_QUALITY=11` 可恢复 |

## 新增能力（可选使用）

- `IdentityUser::{tenant, require_tenant}`、`RequestUser::{permissions, has_permission, require_role, require_tenant}`；
- `RequestContext::identity()`：审计/持久化层读同一份 `Arc<IdentityUser>`；
- `IHttpRequest::remote_addr()`：真实客户端地址（限流/审计），默认 `None` 兼容测试宿主；
- `IHttpContext::take_early_error()`：自定义宿主可提前记录请求级拒绝；
- `Error::public_detail()`：4xx 可公开文案，5xx 返回 `None`；
- `add_pipeline_with`：`IPipelineBehavior` 支持构造注入：

  ```rust
  // 简单场景：闭包捕获依赖
  svc = svc.add_pipeline_with(move |_| {
      Arc::new(AuditBehavior::new(Arc::clone(&audit_sink))) as Arc<dyn IPipelineBehavior>
  });

  // 走 DI 解析（注意：DI 构建期校验时工厂会被预先执行，
  // 此时只能用 get_any(type_name::<T>()) + downcast，不能用 provider_arc()/get()）
  svc = svc.add_pipeline_with(|resolver| {
      let dep = resolver
          .get_any(std::any::type_name::<Arc<MyDependency>>())
          .and_then(|any| any.downcast::<Arc<MyDependency>>().ok())
          .expect("MyDependency must be registered");
      Arc::new(AuditBehavior::new(dep)) as Arc<dyn IPipelineBehavior>
  });
  ```

  行为短路时必须返回与 Handler 相同的响应类型；返回 `Box::new(())` 会得到
  “Response type mismatch: … expected `…`” 的明确错误（500），而不是泛化文案；
- `MiddlewareSlot`：`use_middleware_at` / `use_middleware_with_at` 显式选择中间件槽位。

## 升级检查清单

```text
grep -rn "#\[fromclaim\]" src | grep -i "option<identityuser"   # 改为 #[fromclaim(optional)]
grep -rn "RequestContext::run" src                              # 改为 Arc<IdentityUser>
grep -rn "use_middleware" src                                   # 检查顺序假设；必要时 *_at(.., slot)
grep -rn "RouteEntry::new\|RequestParamEntry" src               # 自定义元数据改用 with_type_id
```

部署侧：
1. 显式设置 `APP_ENV=Production`（未设置现在也是 Production，但显式更清楚）；
2. 本机/CI 设 `APP_ENV=Development`，否则生产守卫（弱 JWT secret、CORS `*`）会按生产处理；
3. 客户端若依赖“坏 token 一定 401”，改为“受保护端点 401、公开端点匿名”；
4. 反向代理/网关若解析 405/OPTIONS 行为，更新预期（`Allow` 头、非预检 OPTIONS 404）。
