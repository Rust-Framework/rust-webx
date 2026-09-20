# 自定义配置节

## 定义配置类型

```rust
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct SiteSection {
    pub title: String,
    pub tagline: String,
    pub author: String,
}
```

`appsettings.json` 的键按 ASP.NET 习惯写作 PascalCase，字段名则是 snake_case，所以
`#[serde(rename_all = "PascalCase")]` 是必需的——少了它，节里的内容会全部落回默认值。

## appsettings.json

```json
{
  "Site": {
    "Title": "Start 的作品",
    "Tagline": "Rust · Web · Full Stack",
    "Author": "Start"
  }
}
```

## 绑定

```rust
Host::builder()
    .add_options::<SiteSection>("Site")
    .build()
```

`add_options` 在 build 时读取合并后的 appsettings，将 `Site` 节反序列化为 `SiteSection`，并以 `Arc<SiteSection>` 注册到 DI 容器供 Handler 注入。

## Docbit 实例

Docbit 的 `Site` 配置节驱动作品集首页的标题、标语和作者信息。

## 小结

任何实现 `Deserialize + Default` 的类型都可作为配置节绑定；键名风格与字段名不一致时，用
`#[serde(rename_all = ...)]` 对齐。

下一章：[生产级能力](../11-production/INDEX.md)
