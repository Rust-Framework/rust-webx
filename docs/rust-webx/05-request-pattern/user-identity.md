# IdentityUser：让 Request 天然携带认证主体

## 从一个真实场景说起

多租户 SaaS 的「创建订单」接口有四条要求：

1. 必须登录；
2. 只能写入当前租户的数据；
3. 记录操作人；
4. 管理员可以代下单。

没有统一身份载体时，每个请求都要重复声明 `sub` / `tenant_id` / `roles` 三个字段，并在 Handler 里做空值判断：

```rust
#[derive(WebxRequestMeta)]
pub struct CreateOrderRequest {
    #[fromclaim("sub")]
    pub user_id: String,
    #[fromclaim("tenant_id")]
    pub tenant_id: String,
    #[fromclaim("roles")]
    pub roles: Vec<String>,
    pub sku: String,
    pub quantity: u32,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateOrderRequest, OrderView> for CreateOrderHandler {
    async fn handle(&mut self, req: CreateOrderRequest) -> Result<OrderView> {
        if req.user_id.is_empty() {
            return Err(Error::Unauthorized("login required".into()));
        }
        if req.tenant_id.is_empty() {
            return Err(Error::Forbidden("tenant missing".into()));
        }
        let is_admin = req.roles.iter().any(|role| role == "admin");
        // ……真正开始写业务
    }
}
```

每个需要身份的 Handler 都要重复这段；一旦某个查询忘了带 `tenant` 条件，就是数据越权。

把身份声明成 Request 的普通字段后：

```rust
#[derive(WebxRequestMeta)]
pub struct CreateOrderRequest {
    #[fromclaim]
    pub user: IdentityUser,   // 绑定阶段从认证主体填充；未登录 → 401
    pub sku: String,         // JSON body
    pub quantity: u32,
}
```

Handler 只剩业务：

```rust
#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateOrderRequest, OrderView> for CreateOrderHandler {
    async fn handle(&mut self, req: CreateOrderRequest) -> Result<OrderView> {
        let user = req.require_user()?;   // &IdentityUser
        let order = self
            .orders
            .create(CreateOrder {
                tenant_id: user.tenant_id.clone(), // 租户随请求走
                created_by: user.id.clone(),
                sku: req.sku,
                quantity: req.quantity,
            })
            .await?;
        Ok(order)
    }
}
```

## 字段与来源

| 字段 | 来源 claim | 说明 |
|------|-----------|------|
| `id` | `sub` | 用户唯一标识 |
| `name` | `name` | 显示名，缺失时为空串 |
| `email` | `email` | 邮箱，缺失时为空串 |
| `tenant_id` | `tenant_id` / `tenant` | 租户标识，单租户应用为空串 |
| `roles` | `roles` | 角色数组 |
| `permissions` | `permissions` | 权限数组 |

辅助方法：

```rust
user.is_in_role("admin");            // 角色判断
user.has_permission("order:write");  // 权限判断
user.display_name();                 // name，缺失时回退到 id
```

## 场景一：必须登录

`IdentityUser`（非 `Option`）表示身份必填：未认证的请求在绑定阶段直接 `401`，Handler 不会被调用。

```rust
#[derive(WebxRequestMeta)]
pub struct UpdateProfileRequest {
    #[fromclaim]
    pub user: IdentityUser,
    pub display_name: String,
}

#[put("/api/profile")]
impl IRequest<ProfileView> for UpdateProfileRequest {}
```

配合 `#[authorize]` 时，认证与授权先由路由层检查，身份字段仍可直接使用。

## 场景二：匿名可访问

商品详情页既服务游客，也服务登录用户（例如显示「已收藏」）：

```rust
#[derive(WebxRequestMeta)]
pub struct ProductDetailRequest {
    #[frompath]
    pub id: String,
    #[fromclaim]
    pub viewer: Option<IdentityUser>,   // 游客为 None，不返回 401
}
```

```rust
let favorited = match req.viewer {
    Some(user) => self.favorites.contains(&user.id, &req.id).await?,
    None => false,
};
```

`Option<IdentityUser>` 只在「身份可选」时使用；需要登录的接口不要用它来“兼容”匿名。

## 场景三：角色与权限分支

路由级规则用 `#[authorize(role = "admin")]` 拦截；进入 Handler 后仍可能需要按角色走不同业务：

```rust
#[get("/api/orders")]
#[authorize]
impl IRequest<Vec<OrderView>> for ListOrdersRequest {}

#[derive(WebxRequestMeta)]
pub struct ListOrdersRequest {
    #[fromclaim]
    pub user: IdentityUser,
    pub status: Option<String>,
}
```

```rust
let scope = if req.user.is_in_role("admin") {
    OrderScope::AllTenants
} else {
    OrderScope::Tenant(req.user.tenant_id.clone())
};
```

## 场景四：多租户数据隔离

租户标识必须来自令牌，不能来自请求体或 query（那等于让客户端自选租户）：

```rust
// ✅ tenant 来自身份
let tenant = &req.user.tenant_id;

// ❌ 不要从客户端输入取租户
// let tenant = req.tenant_id;
```

把 `tenant_id` 放进 `IdentityUser` 后，仓储层可以统一加过滤条件，业务代码不需要每个查询都记得传。

## 收益对照

| 维度 | 手动解析 claims | `IdentityUser` 字段 |
|------|----------------|--------------------|
| 每个 Handler 样板 | 8–15 行 | 0 行 |
| 未登录处理 | 每个 Handler 手写 401 | 绑定层统一 401 |
| 空值/缺失 claim | 各自 `unwrap_or_default` | 统一为空串/空数组 |
| 测试 | 构造 `IHttpContext` + claims | 直接构造 `IdentityUser` |
| 类型安全 | 字符串键，拼错运行时才发现 | 编译期字段访问 |
| 多租户一致性 | 依赖每个查询自觉 | 身份随 Request 传递 |
| OpenAPI | 可能把身份写成普通参数 | 身份不进入参数表 |

## 和框架现有身份能力的关系

框架没有第二套用户模型。`IdentityUser` 是现有 `IClaims` 的**只读投影**，各层职责如下：

| 能力 | 层级 | 用途 |
|------|------|------|
| `IClaims` | 认证核心 | principal 的底层接口：`subject()`、`roles()`、`permissions()`、原始 map |
| `IClaimsExt` / `IHttpContext` | 中间件/基础设施 | 在认证中间件或框架扩展里读写 claims |
| `IdentityUser` | 请求字段（绑定层） | 业务 Handler 的默认身份载体：id/name/email/tenant/roles/permissions |

`IdentityUser` **不是持久化实体**，也不对应数据库表——不要把它和 ASP.NET Core Identity 的 EF 实体混为一谈；
它只回答“这个请求是谁发的”。需要落库时，用应用自己的 `User`/`UserEntity`，通过 `user.id` 关联。

## 和原始 claims 的分工

- **优先 `IdentityUser`**：覆盖 90% 的登录、租户、角色、操作人场景。
- **单个特殊 claim**：用 `#[fromclaim("scope")] scope: String`。
- **需要完整 claim map**：在中间件/基础设施层通过 `IClaimsExt` 读取；业务 Handler 不应依赖原始 map。

## 测试建议

身份是普通数据，Handler 测试不需要启动 HTTP：

```rust
let req = CreateOrderRequest {
    user: IdentityUser {
        id: "u-1".into(),
        tenant_id: "t-1".into(),
        roles: vec!["admin".into()],
        ..Default::default()
    },
    sku: "SKU-1".into(),
    quantity: 2,
};
let order = handler.handle(req).await?;
```

端到端测试则签发带 `sub` / `name` / `email` / `tenant_id` / `roles` 的 JWT，断言 `401`、租户隔离与角色分支。

## 常见错误

| 现象 | 原因 | 修正 |
|------|------|------|
| 编译错误提示 `IdentityUser` 必须标记来源 | 忘记 `#[fromclaim]` | 加 `#[fromclaim]` |
| 身份字段始终为空 | 路由没启用认证 / 令牌没有对应 claim | 检查 `add_authentication()` 与令牌内容 |
| 客户端能读到别的租户数据 | 租户取自请求参数 | 一律取 `user.tenant_id` |
| 匿名接口返回 401 | 用了 `IdentityUser` 而不是 `Option<IdentityUser>` | 按场景选择类型 |

## 小结

`IdentityUser` 把「认证身份」从上下文细节变成 Request 的数据结构：绑定层负责 401 与字段填充，
业务层只消费 `user.id` / `user.tenant_id` / `user.roles`。身份不再是每个 Handler 的重复劳动，
而是请求契约的一部分。
