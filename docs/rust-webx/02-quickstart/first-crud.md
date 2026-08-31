# 第一个 CRUD API

本节实现一个内存中的用户 CRUD，展示 GET/POST/PUT/DELETE 四种路由模式。

## 数据模型

`src/contracts/user.rs`：

```rust
use rust_webx::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct UserDto {
    pub id: String,
    pub name: String,
    pub email: String,
}

// ── List ──
pub struct ListUsersRequest;

#[get("/api/users")]
impl IRequest<Vec<UserDto>> for ListUsersRequest {}

// ── Get by ID ──
pub struct GetUserRequest {
    pub id: String,
}

#[get("/api/users/{id}")]
impl IRequest<UserDto> for GetUserRequest {}

// ── Create ──
#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub name: String,
    pub email: String,
}

#[post("/api/users")]
impl IRequest<UserDto> for CreateUserRequest {}

// ── Update ──
#[derive(Deserialize)]
pub struct UpdateUserRequest {
    pub id: String,
    pub name: String,
    pub email: String,
}

#[put("/api/users/{id}")]
impl IRequest<UserDto> for UpdateUserRequest {}

// ── Delete ──
pub struct DeleteUserRequest {
    pub id: String,
}

#[delete("/api/users/{id}")]
impl IRequest<()> for DeleteUserRequest {}
```

## 共享存储

`src/handlers/user.rs`：

```rust
use std::collections::HashMap;
use std::sync::Arc;
use rust_webx::*;
use tokio::sync::RwLock;
use crate::contracts::user::*;

type UserStore = Arc<RwLock<HashMap<String, UserDto>>>;

// ── List ──
#[derive(Default)]
struct ListUsersHandler;

#[handler]
#[async_trait]
impl IRequestHandler<ListUsersRequest, Vec<UserDto>> for ListUsersHandler {
    async fn handle(&self, _req: ListUsersRequest) -> Result<Vec<UserDto>> {
        // 实际项目中通过 DI 注入 store
        Ok(vec![])
    }
}
```

带依赖注入的完整版本：

```rust
struct CreateUserHandler {
    store: UserStore,
}

#[async_trait]
impl IRequestHandler<CreateUserRequest, UserDto> for CreateUserHandler {
    async fn handle(&self, req: CreateUserRequest) -> Result<UserDto> {
        let id = uuid::Uuid::new_v4().to_string();
        let user = UserDto {
            id: id.clone(),
            name: req.name,
            email: req.email,
        };
        self.store.write().await.insert(id, user.clone());
        Ok(user)
    }
}
```

## 注册带依赖的 Handler

在 `main.rs` 中：

```rust
#[tokio::main]
async fn main() {
    let store: UserStore = Arc::new(RwLock::new(HashMap::new()));

    Host::builder()
        .register(move |svc| {
            let store = Arc::clone(&store);
            svc.singleton::<dyn IRequestHandler<CreateUserRequest, UserDto>>(
                move |_| Arc::new(CreateUserHandler { store: Arc::clone(&store) })
            );
            // 其他 Handler 同理...
        })
        .build()
        .run()
        .await
        .expect("Server failed");
}
```

HTTP 端点应使用 `#[handler]` / `#[handler(inject)]`（inventory 自动注册）。以下 **`register_handlers!` 已弃用**，仅适用于非 HTTP 的 Mediator 场景：

```rust
.register(|svc| {
    register_handlers!(svc,
        ListUsersRequest => Vec<UserDto> => ListUsersHandler,
        GetUserRequest => UserDto => GetUserHandler,
    )
})
```

手动 `.singleton::<dyn IRequestHandler<…>>()` 同理：HTTP `RouteDispatch` 不经过 DI 查找 Handler，请勿将其作为主路径。

## 测试 API

```bash
# 创建用户
curl -X POST http://localhost:5000/api/users \
  -H "Content-Type: application/json" \
  -d '{"name":"Alice","email":"alice@example.com"}'

# 获取列表
curl http://localhost:5000/api/users

# 获取单个
curl http://localhost:5000/api/users/{id}

# 更新
curl -X PUT http://localhost:5000/api/users/{id} \
  -H "Content-Type: application/json" \
  -d '{"name":"Alice Updated","email":"alice@example.com"}'

# 删除
curl -X DELETE http://localhost:5000/api/users/{id}
```

## 错误处理示例

```rust
async fn handle(&self, req: GetUserRequest) -> Result<UserDto> {
    let store = self.store.read().await;
    store
        .get(&req.id)
        .cloned()
        .ok_or_else(|| Error::NotFound(format!("User {} not found", req.id)))
}
```

`Error::NotFound` 自动映射为 HTTP 404，响应体 `{"error":"...","status":404}`。

## 设计要点

| 实践 | 说明 |
|------|------|
| Request 放 contracts | 路由元数据与 DTO 定义集中，便于查阅 API 契约 |
| Handler 放 handlers | 纯业务逻辑，通过 DI 获取依赖 |
| 路径参数同名字段 | `{id}` 自动绑定到 `GetUserRequest.id` |
| Body 字段 Deserialize | POST/PUT 的 Request 需 `#[derive(Deserialize)]` |

## 小结

CRUD 是检验框架能力的试金石。rust-webx 用统一的四步模式覆盖全部 HTTP 方法，无需为每种方法学习不同 API。

下一节：[运行、调试与验证](run-and-debug.md)
