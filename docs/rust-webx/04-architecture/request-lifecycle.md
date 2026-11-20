# 请求生命周期

## 全景流程图

```mermaid
sequenceDiagram
    participant Client
    participant Engine as 2.0 Engine worker
    participant Pipeline as 中间件管道
    participant Router as matchit Router
    participant Target as RouteTarget
    participant Handler as #[handler]
    participant DI as ServiceProvider

    Client->>Engine: HTTP Request
    Engine->>Pipeline: EngineContext（移动 Parts，惰性 body）
    loop 每个中间件
        Pipeline->>Pipeline: IMiddleware::invoke
        Note over Pipeline: 可短路（设置 response status）
    end
    Pipeline->>Router: match_route(method, path)
    alt 匹配成功
        Router->>Target: 路由 pattern + route_params
        Target->>Target: #[authorize] / max_body_size 守卫
        alt 无 typed behavior
            Target->>Handler: 直接 bind + handle(request)
        else 有 IPipelineBehavior
            Target->>DI: mediated fallback（HandlerCache + scope）
        end
        Handler-->>Target: EndpointResult::{Empty, Json, Action}
        Target-->>Engine: 状态/头/Bytes
    else 未匹配
        Router-->>Engine: 404/405/OPTIONS/SPA fallback
    end
    Engine->>Pipeline: 逆序 after hooks（已写入真实错误状态）
    Pipeline-->>Client: finalize + hyper 响应
```

## 阶段详解

### 1. 连接与请求读取

2.0 起 `Host::run()` 启动的是 `rust-webx-engine` 的 worker pool：一个 acceptor 在调用方 runtime 上 accept，把连接通过有界 channel 交给 N 个各自拥有 `current_thread` runtime 的 worker（安全 `into_std`/`from_std`，无 `unsafe`），每个 worker 用一个 hyper `auto::Builder` 服务连接。连接不会跨核迁移。

`EngineContext::new()` 不读取 body；body 由 `IHttpRequest::body_bytes()` 按需读取，并强制 `App.MaxBodySize` 限制（默认 10MB）。请求头、method、URI 直接移动/借用 hyper 的 `Parts`，query 只在被访问时解析。

### 2. 中间件管道

中间件按**注册顺序**依次调用。每个中间件可：

- 读取/修改请求头
- 设置认证 claims（JWT 中间件）
- 检查 CORS 预检
- 限流拒绝
- **短路**：设置 response status 后返回，跳过后续处理

### 3. 路由匹配

`Router` 基于 matchit（radix 树）匹配 HTTP method + path：

- 静态段精确匹配
- `{param}` 动态段提取到 `route_params`
- 匹配成功后设置 `route_pattern()`（如 `/api/users/{id}`），供授权使用
- 405/OPTIONS 由 engine 直接写 `Allow`，SPA 只接管真正的 miss（不会把 POST-only 路径伪装成 200）

### 4. 端点处理

`#[handler]` 生成的 `HandlerDispatch` 按 `(请求类型, 响应类型)` 的 `TypeId` 注册；`Host` 启动时把它和 `#[get]` 等路由 join：

1. 先执行静态 `#[authorize]` 规则，再运行已注册的 `IDynamicAuthorizer`（仅限有 `#[authorize]` 的路由）；全部通过才继续
2. 从 route params、query、body 绑定 Request struct
3. 注入 claims，进入 `RequestContext`
4. 若没有注册任何 `IPipelineBehavior`：直接构造 handler 并 `handle(request)`（不经过 `Mediator`、`Box<dyn Any>`、`JsonResult` 装箱）；有 behavior 时回退到 mediated 路径
5. `#[handler(inject)]` 按请求开一个 DI scope；普通 handler 不创建 scope
6. 响应转为 `EndpointResult::{Empty, Json(Bytes), Action(Response)}`，由 engine 写入状态/头/body

### 5. 错误处理

若 Handler 返回 `Err(Error::...)`、中间件失败或触发 panic，engine 在 **after hooks 之前** 把错误转换成响应，使 metrics/tracing/compression 看到真实状态；随后由单一错误渲染器写入统一的 `problem+json`，并清除成功表示特有的 `content-encoding`、`etag`、`cache-control` 等头：

```json
{
  "type": "https://httpstatuses.com/404",
  "title": "Not Found",
  "status": 404,
  "detail": "User abc not found"
}
```

响应头为 `content-type: application/problem+json`（RFC 7807）。

### 6. 响应发送

`EngineContext::finalize()` 生成 `hyper::Response`，通过已建立的连接返回客户端；HEAD/204/304、Range/ETag/SSE 语义在此统一处理。

## 与 Mediator 的关系

普通 HTTP 快路径不经过 `Mediator`：`#[handler]` 生成按 `(Req, Rsp)` 键控的直接 dispatch，Host 在启动时与路由 join。注册了任意 `IPipelineBehavior` 时，Host 保守回退到 mediated 路径，保证行为链语义不变。需要纯进程内调用的 handler 应使用 `#[handler(in_process)]`：它只注册 `IMediator` 路径，不要求响应实现 `Serialize`，也不会被“无路由”启动诊断拒绝。

## DispatchRuntime

每个 `Host` 持有实例级 `DispatchRuntime`（`ServiceProvider` + `HandlerCache`），它只是显式句柄：
HTTP 请求通过 `IHttpContext::dispatch_runtime()` 取得该句柄；直接 dispatch 只在需要时打开 scope，
typed-pipeline fallback 才用其中的 provider + registry 构造 `Mediator`。没有 task-local 全局入口。

多 Host 集成测试：`host.dispatch_runtime()` / `host.provider()` 直接使用；后台任务通过注入的
`Arc<dyn ScopeFactory>`（rust-dix 内建）显式 `create_scope()`。

## 启动与关闭生命周期

```mermaid
graph TD
    A[Host::build] --> B[加载 appsettings]
    B --> C[扫描 inventory 路由]
    C --> D[构建 DI 容器]
    D --> E[注册中间件]
    E --> F[构建 Router]
    F --> G[Host::run]
    G --> H[IHostedService::start × N]
    H --> I[启动 HTTP 监听]
    I --> J[等待 shutdown 信号]
    J --> K[IHostedService::stop × N 逆序]
    K --> L[关闭连接]
```

## 小结

一次 HTTP 请求经历：中间件管道 → 路由匹配 → 端点调度 → Handler 执行 → 序列化响应。理解这条链路是调试和扩展的基础。

下一节：[分层模型与依赖方向](layering-model.md)
