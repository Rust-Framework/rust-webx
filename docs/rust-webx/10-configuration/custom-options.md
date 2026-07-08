# 自定义配置节

## 定义配置类型

```rust
use rust_webx::IAppOptions;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SiteSection {
    pub title: String,
    pub tagline: String,
    pub author: String,
}

impl IAppOptions for SiteSection {}
```

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
use rust_webx::bind_config;

Host::builder()
    .configure(|app| {
        app.useOptions(|opts| {
            let site: SiteSection = bind_config(opts, "Site");
            println!("Site: {}", site.title);
        });
    })
```

## Docbit 实例

Docbit 的 `Site` 配置节驱动作品集首页的标题、标语和作者信息。

## 小结

任何实现 `Deserialize + Default` 的类型都可作为配置节绑定。

下一章：[生产级能力](../11-production/INDEX.md)
