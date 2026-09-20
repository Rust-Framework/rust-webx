# 第一个 CRUD API

本节实现一个内存中的用户 CRUD，展示 GET/POST/PUT/DELETE 四种路由模式。

## 数据模型

`src/contracts/user.rs`：

```rust
use webx::*;
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
use webx::*;
use tokio::sync::RwLock;
use crate::contracts::user::*;

// ── 共享存储：注册为单例，由 DI 注入 ──
#[derive(Default)]
pub struct UserStore {
    users: RwLock<HashMap<String, UserDto>>,
}

impl UserStore {
    pub async fn insert(&self, id: String, user: UserDto) {
        self.users.write().await.insert(id, user);
    }

    pub async fn get(&self, id: &str) -> Option<UserDto> {
        self.users.read().await.get(id).cloned()
    }
}

// ── List ──
#[derive(Default)]
struct ListUsersHandler;

#[handler]
#[async_trait]
impl IRequestHandler<ListUsersRequest, Vec<UserDto>> for ListUsersHandler {
    async fn handle(&mut self, _req: ListUsersRequest) -> Result<Vec<UserDto>> {
        // 实际项目中通过 DI 注入 store
        Ok(vec![])
    }
}
```

带依赖注入的完整版本，用 `#[derive(Inject)]` 声明 Handler 依赖，并用 `#[handler(inject)]` 标记注入式构造：

```rust
#[derive(Inject)]
struct CreateUserHandler {
    #[inject]
    store: Arc<UserStore>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<CreateUserRequest, UserDto> for CreateUserHandler {
    async fn handle(&mut self, req: CreateUserRequest) -> Result<UserDto> {
        let id = uuid::Uuid::new_v4().to_string();
        let user = UserDto {
            id: id.clone(),
            name: req.name,
            email: req.email,
        };
        self.store.insert(id, user.clone()).await;
        Ok(user)
    }
}
```

## 注册共享依赖

Handler 由 `#[handler]` / `#[handler(inject)]` 自动向 `inventory` 注册，HTTP 分发通过 `HandlerCache` 查找，**不**经过 DI 查找 `dyn IRequestHandler`。`main.rs` 只需注册 Handler 依赖的服务：

```rust
#[webx::main]
async fn main() {
    Host::builder()
        .register(|svc| {
            svc.singleton::<UserStore>(|_| Arc::new(UserStore::default()));
        })
        .build()
        .run()
        .await
        .expect("Server failed");
}
```

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
async fn handle(&mut self, req: GetUserRequest) -> Result<UserDto> {
    self.store
        .get(&req.id)
        .await
        .ok_or_else(|| Error::NotFound(format!("User {} not found", req.id)))
}
```

`Error::NotFound` 自动映射为 HTTP 404，响应体为 RFC 7807 `application/problem+json`（`type` / `title` / `status` / `detail`）。

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
