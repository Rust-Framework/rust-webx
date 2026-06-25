# 书籍目录与文件结构规范

跨项目通用的 **Docbit v2 + Markdown 书籍** 布局。完整落地样例见 [examples/rust-webapp-reference.md](../examples/rust-webapp-reference.md)。

## 顶层布局

```
docs/
├── README.md                 # 维护说明（非书籍正文）
└── {slug}/                   # 一本书；slug = Docbit 作品标识
    ├── FOREWORD.md
    ├── INDEX.md              # 全书 Markdown 目录
    ├── INDEX.json            # 站点导航 + 元数据
    ├── logo.svg              # 可选
    └── {NN-chapter-slug}/
        ├── INDEX.md
        └── {section-id}.md
```

`{slug}`、`{NN-chapter-slug}`、`{section-id}` 均为 **kebab-case**（章目录带两位序号前缀）。

## 命名规则

| 类型 | 格式 | 示例 |
|------|------|------|
| 书目录 | `docs/{slug}/` | `docs/my-framework/` |
| 章目录 | `{两位序号}-{kebab-case}` | `05-core-patterns` |
| 章 INDEX | 固定 `INDEX.md` | `05-core-patterns/INDEX.md` |
| 小节 | `{kebab-case}.md` = `section.id` | `getting-started.md` |
| 前言 | 根目录 `FOREWORD.md`，`meta.foreword` 引用 | — |

**禁止：**

- 章目录无序号（破坏排序与 Docbit 展开顺序）
- 小节文件名与 `INDEX.json` 的 `section.id` 不一致
- 把书籍正文放在 `docs/{slug}/` 之外

## INDEX.md（全书目录）

- 标题：`# {框架名} 开发者手册 · 目录`（或项目既有标题）
- 建议副标题含「渐进式披露」定位
- 结构：**部分（可选）→ 章 → 小节** 三级链接
- 章 → `{chapter}/INDEX.md`；小节 → `{chapter}/{section}.md`
- 部分之间用 `---` 分隔

新增章/小节后**必须**更新根 `INDEX.md`。

## 章 INDEX.md

最低结构：

```markdown
# 第 N 章 {章标题}

{1–2 句导语}

## 本章小节

| 小节 | 内容 |
|------|------|
| [标题](section-id.md) | 一句话摘要 |

## 下一步

从 [首小节](first-section.md) 开始。
```

可选块：

| 块 | 适用 |
|----|------|
| `## 学习目标` + 编号问题 | 概念章 |
| `## 预计时间` | 动手章 |
| `## 前置要求` | 有环境依赖的动手章 |

## INDEX.json（Docbit v2）

结构：`meta` + `parts[]` + 每 part 下 `chapters[]` + 每章 `sections[]`。

### meta

```json
{
  "title": "Short Title",
  "subtitle": "...",
  "docTitle": "完整文档标题",
  "description": "...",
  "foreword": "FOREWORD.md",
  "pathRules": {
    "chapterIndex": "{chapterId}/INDEX.md",
    "sectionFile": "{chapterId}/{sectionId}.md"
  }
}
```

### chapter / section

```json
{
  "id": "05-core-patterns",
  "title": "章标题",
  "subtitle": "可选",
  "sections": [
    {
      "id": "core-concept",
      "title": "小节标题",
      "summary": "导航摘要一句话",
      "readingTime": 12,
      "difficulty": "intermediate"
    }
  ]
}
```

| 字段 | 规则 |
|------|------|
| `chapter.id` | = 章目录名 |
| `section.id` | = 小节文件名（无 `.md`） |
| `difficulty` | `beginner` \| `intermediate` \| `advanced` |
| `readingTime` | 分钟数，与相邻小节量级一致 |

### 新增小节四步同步

1. 创建 `{chapterId}/{sectionId}.md`
2. 更新 `{chapterId}/INDEX.md`
3. 更新根 `INDEX.md`
4. 在 `INDEX.json` 对应 `sections` 追加（顺序 = 阅读顺序）

只改 `.md` 不改 JSON → Docbit 导航缺项。

## 非 Docbit 项目

| 站点 | 索引文件 | 本规范仍适用 |
|------|---------|-------------|
| mdBook | `src/SUMMARY.md` | FOREWORD、章 INDEX、小节结构、渐进式披露 |
| 纯 GitHub MD | 根 `INDEX.md` | 同上，无 JSON |

---

## 反模式

- 三处 INDEX 不同步
- 单节 API 大全（拆节或链到 rustdoc）
- 在 quickstart 章写扩展点实现细节（链到 extensibility 类章节）
- 复制其他 `{slug}` 的章节 id 或正文到当前书
