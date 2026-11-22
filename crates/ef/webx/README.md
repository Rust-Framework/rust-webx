# rust-webx-ef

The `rust-webx` boundary for `rust-ef`.

`rust-ef` is Web-agnostic (since 2.6 it does not even have a `webx` feature), so
translating `EFError` into `webx::Error` belongs at the application boundary.
This crate owns that decision once:

```rust
use webx_ef::EfResultExt;

let user = ctx
    .set::<User>()
    .filter(linq!(|u: User| u.id == id))
    .first_or_default()
    .await
    .map_ef()?; // EFError -> webx::Error

ctx.save_changes().await.map_ef()?;
```

| `EFError` | HTTP | Client-visible detail |
|---|---|---|
| `NotFound` | 404 | the caller-authored message |
| `ModelValidation` | 400 | the caller-authored message |
| `ConcurrencyConflict` | 409 | the caller-authored message |
| `Query` / `TypeConversion` | 400 | fixed text (SQL/column names never echoed) |
| `UniqueViolation` | 409 | fixed text (constraint/table never echoed) |
| `Connection` / `Transaction` | 503 | fixed text, retryable |
| everything else | 500 | none (`public_detail() = None`); message + source chain stay in the operator log |

`rust-webx-core` and the `rust-webx` umbrella never link this crate: add
`rust-webx-ef` to the application (or use the reference apps' `db.rs` helpers)
and call `.map_ef()?` at persistence call sites.

## 设计边界

- **单体/多实例**：纯函数，无状态、无连接、无后台任务；同一映射在任何部署形态下一致。
- **容器化**：不读取配置、不建立连接；错误脱敏在进程内完成，日志里保留操作者细节。
- **性能**：成功路径零开销（`map_ef` 只在 `Err` 分支分配）；脱敏文案是静态常量，
  5xx 只在日志侧拼接 source 链。
