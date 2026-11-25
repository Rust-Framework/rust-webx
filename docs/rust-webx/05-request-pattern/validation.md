# 声明式校验（#[validate]）

绑定负责"值能解析成声明的类型"，`#[validate(...)]` 负责"值符合业务约束"。两者共用一套
`400 problem+json` 的 `errors` 聚合：一次请求的绑定错误和校验错误一起返回，客户端不必分两轮修。

```rust
#[derive(WebxRequestMeta)]
pub struct CreateUserRequest {
    #[validate(length(min = 3, max = 64))]
    pub name: String,
    #[validate(email)]
    pub email: String,
    #[validate(range(min = 18, max = 120))]
    pub age: u8,
    #[validate(length(min = 1, max = 20))]
    pub tags: Vec<String>,
    #[validate(custom = "validate_slug")]
    pub slug: String,
}

fn validate_slug(value: &String) -> std::result::Result<(), String> {
    if value.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-') {
        Ok(())
    } else {
        Err("The `slug` field may contain only letters, digits and `-`.".to_string())
    }
}
```

`custom` 函数返回 `std::result::Result<(), String>`——写全路径，避免与 `webx::Result` 别名冲突。

## 规则

| 规则 | 适用类型 | 语义 |
|------|----------|------|
| `length(min = N, max = N)` | `String`、`Vec<T>` | 字符串按**字符数**（不是字节数）；集合按**元素个数**；两个边界都可省略其一 |
| `range(min = N, max = N)` | 数值标量（整数/浮点） | 闭区间；边界必须是数字字面量 |
| `email` | `String` | 邮箱形状检查（单个 `@`、非空 local、带点域名、无空白、总长 ≤254）；**不**验证可投递性 |
| `custom = "path::to::fn"` | 任意字段值 | `fn(&T) -> Result<(), String>`，返回的 `Err` 文案直接进入 `errors` |

- 一个字段可以写多个 `#[validate(...)]`，规则按声明顺序执行。
- `Option<T>` 字段**只在有值时校验**（`None` 合法）；必填性由类型/绑定决定，不需要专门的
  `required` 规则。
- 校验在绑定之后执行；绑定已失败的字段不会再产生校验噪音。
- 校验是同步纯函数，不做 IO、不查库——需要查库或跨字段一致性时放在 Handler 里（见下）。

## 错误响应

```json
{
  "title": "Validation Failed",
  "status": 400,
  "detail": "One or more validation errors occurred",
  "errors": {
    "name": ["The `name` field must be at least 3 characters."],
    "email": ["The `email` field must be a valid email address."],
    "age": ["The `age` field must be at least 18."],
    "tags": ["The `tags` field must be at least 1 item."]
  }
}
```

`errors` 的键是**字段名**（与绑定错误一致），每个字符串是已命名的完整句子，可直接展示给终端用户。
规则失败**不会**进入 Handler，也不会返回 500：它们是请求错误（400）。

## OpenAPI 同步

同一份声明会写进 `param_meta()`，OpenAPI 生成器把它翻译成 schema 关键字，文档与运行时不会
各说各话：

| 规则 | OpenAPI 关键字 |
|------|----------------|
| `length` on `String` | `minLength` / `maxLength` |
| `length` on collection | `minItems` / `maxItems` |
| `range` | `minimum` / `maximum` |
| `email` | `format: "email"` |

`custom` 规则没有可声明的形状，不进入 OpenAPI——文档需要时用端点描述补充语义。

## 不要放进声明式校验的

- **跨字段规则**（`password_confirm == password`、`end > start`）：字段级规则看不到其它字段
  的最终值，放在 Handler 第一段（或领域用例）里，用 `OperationResult::failed`/`Error::validation`
  返回同样的 400 信封。
- **依赖数据库/外部服务的唯一性**：声明式校验是纯函数；唯一性在事务内由唯一索引兜底并映射
  409。
- **正则/复杂格式**：框架不带正则依赖；用 `custom` 写一个函数，把正则需求留在业务 crate。
- **权限/租户边界**：那是授权与身份问题，不属于请求值校验。

## 与绑定的关系

| 阶段 | 负责 | 失败结果 |
|------|------|----------|
| 绑定 | 类型、必填、来源（query/body/...） | 400，`errors` 按字段聚合 |
| `#[validate]` | 业务约束（长度、范围、格式、自定义） | 400，同一 `errors` 信封，与绑定错误合并 |
| Handler | 跨字段、外部依赖、权限 | 400/403/409 等，自行返回 |

下一节：[用户身份与多租户](user-identity.md)
