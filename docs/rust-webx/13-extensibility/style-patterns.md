# 样式与约定封装

## 命名约定

| 类型 | 命名 | 示例 |
|------|------|------|
| Request | `{Action}{Entity}Request` | `CreateUserRequest` |
| Handler | `{Action}{Entity}Handler` | `CreateUserHandler` |
| Response DTO | `{Entity}Dto` / `{Entity}View` | `UserDto`, `UserView` |
| Service | `{Entity}Service` | `DocService` |
| Entity | `{Entity}Entity` | `UserEntity` |
| Event | `{Entity}{Action}Event` | `UserCreatedEvent` |

## API 路径约定

```
/api/{resource}           # 集合
/api/{resource}/{id}      # 单个资源
/api/{resource}/{id}/{sub} # 子资源
/api/auth/{action}        # 认证相关
```

## 错误消息风格

```rust
// ✅ 面向用户
Error::NotFound(format!("User '{}' not found", id))
Error::Validation("Email is required".into())

// ❌ 暴露内部
Error::Internal(format!("SQL error: {}", db_err))
```

## DTO 设计

```rust
// 请求 DTO：只包含输入字段
#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub name: String,
    pub email: String,
    // 不包含 id、created_at 等系统字段
}

// 响应 DTO：包含输出字段
#[derive(Serialize)]
pub struct UserDto {
    pub id: String,
    pub name: String,
    pub email: String,
    pub created_at: String,
}
```

## 版本化

```rust
// 方式一：路径版本
#[get("/api/v1/users")]
impl IRequest<Vec<UserDto>> for ListUsersV1Request {}

// 方式二：controller 宏
#[controller("/api/v2")]
mod v2_routes { ... }
```

## 项目级封装

在团队中创建 `style-guide.md` 和 `CLAUDE.md` / `.cursor/rules`，统一 AI 生成代码的风格。

## 小结

一致的命名和约定降低认知负担，使团队代码像一个人写的。

下一节：[第三方库集成](third-party-integration.md)
