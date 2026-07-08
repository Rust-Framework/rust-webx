# 路由宏详解

## 基本用法

路由宏标注在 `impl IRequest<T>` 块上：

```rust
#[get("/api/users")]
impl IRequest<Vec<UserDto>> for ListUsersRequest {}

#[get("/api/users/{id}")]
impl IRequest<UserDto> for GetUserRequest {}

#[post("/api/users")]
impl IRequest<UserDto> for CreateUserRequest {}

#[put("/api/users/{id}")]
impl IRequest<UserDto> for UpdateUserRequest {}

#[delete("/api/users/{id}")]
impl IRequest<()> for DeleteUserRequest {}
```

## 可用宏

| 宏 | HTTP 方法 |
|----|----------|
| `#[get("/path")]` | GET |
| `#[post("/path")]` | POST |
| `#[put("/path")]` | PUT |
| `#[delete("/path")]` | DELETE |

## 路径参数

`{name}` 语法定义动态段：

```rust
struct GetUserRequest {
    pub id: String,       // 绑定 {id}
}

#[get("/api/users/{id}")]
impl IRequest<UserDto> for GetUserRequest {}
```

框架在路由匹配后，将提取的值填入同名字段。

### 通配符路径

```rust
struct GetDocContentRequest {
    pub work: String,
    pub any: String,      // 绑定 {*any} 剩余路径
}

#[get("/api/docs/{work}/content/{*any}")]
impl IRequest<DocContent> for GetDocContentRequest {}
```

## 授权宏

```rust
// 需要认证（任意已登录用户）
#[get("/api/auth/me")]
#[authorize]
impl IRequest<UserView> for AuthMeRequest {}

// 需要 admin 角色
#[post("/api/users")]
#[authorize(role = "admin")]
impl IRequest<UserDto> for CreateUserRequest {}

// 需要特定权限
#[put("/api/settings")]
#[authorize(permission = "settings:write")]
impl IRequest<SettingsDto> for UpdateSettingsRequest {}
```

授权元数据在编译期收集，`add_authentication()` 启用后由 `resource_auth_middleware` 执行。

## 常见错误

### ❌ 标注在 struct 上

```rust
#[get("/hello")]  // 错误！
struct HelloRequest;
```

### ✅ 标注在 impl 块上

```rust
struct HelloRequest;

#[get("/hello")]  // 正确
impl IRequest<String> for HelloRequest {}
```

### ❌ 路径不匹配

```rust
struct GetUserRequest { id: String }

#[get("/api/users")]  // 缺少 {id}，字段无法绑定
impl IRequest<UserDto> for GetUserRequest {}
```

## 小结

路由宏是「请求即端点」的语法糖。一行 `#[get]` 替代传统框架中路由表 + 处理器映射的两处定义。

下一节：[Handler 注册策略](handler-registration.md)
