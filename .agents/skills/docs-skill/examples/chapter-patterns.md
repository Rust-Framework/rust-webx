# 章节与小节结构范例

通用结构模式；具体措辞与 API 须替换为**当前项目** Profile 中的框架名与约定。

## 范例 1：概念章 INDEX

```markdown
# 第一章 认识 {框架名}

本章建立**第一印象**：它是什么、解决什么问题、由哪些部分组成。

## 本章小节

| 小节 | 内容 |
|------|------|
| [什么是 {框架名}](what-is-{slug}.md) | 定位、核心能力 |
| [适用场景与边界](who-should-use.md) | 何时选用、何时不选 |
| [生态与模块全景](ecosystem-overview.md) | 包/Crate/模块结构 |

## 学习目标

读完本章，你应能回答：

1. {框架名} 与同类方案的本质区别是什么？
2. 核心开发模式的一句话定义是什么？
3. 主要模块/包及职责是什么？

## 下一步

可跳至 [第二章 快速上手](../02-quickstart/INDEX.md)；建议先读完本章再动手。
```

**要点：** 可检验的学习目标；允许非线性跳章并给出建议。

---

## 范例 2：动手章 INDEX

```markdown
# 第二章 快速上手

本章**从零到运行**：创建项目、首个示例、典型用例、验证。

## 本章小节

| 小节 | 内容 |
|------|------|
| [创建项目与依赖](create-project.md) | 依赖与目录 |
| [Hello World 详解](hello-world.md) | 最小示例逐行解读 |
| [第一个完整示例](first-example.md) | 典型场景端到端 |
| [运行与验证](run-and-debug.md) | 命令行验证、常见问题 |

## 预计时间

约 30–45 分钟。

## 前置要求

- （从项目 README 读取：语言版本、工具链）

## 下一步

从 [创建项目与依赖](create-project.md) 开始。
```

---

## 范例 3：教程小节结构（骨架）

顺序：**完整代码 → 分步表 → 机制图 → 变体 → 小结**

```markdown
# Hello World 详解

## 完整代码

{使用 Profile 中的 import 与 API}

## 分步解读

### 第二步：{步骤名}

| 元素 | 含义 |
|------|------|

## 背后发生了什么

```mermaid
sequenceDiagram
    ...
```

## 常见变体

## 小结

{一句话核心契约}

下一节：[...](...)
```

代码块内容必须从**当前仓库**提取，勿粘贴他项目示例。

---

## 范例 4：INDEX.json section 条目

```json
{
  "id": "hello-world",
  "title": "Hello World 详解",
  "summary": "最小启动与首个 API",
  "readingTime": 12,
  "difficulty": "beginner"
}
```

`id` 必须等于 `{section-id}.md` 的文件名。

---

## 范例 5：边界声明表

```markdown
### {框架}负责

| 职责 | 说明 |
|------|------|

### 应用负责

| 职责 | 说明 |

### 刻意不内置

- {能力}（通过 {扩展点} 自行集成）

详见 [{扩展章标题}](../NN-extensibility/INDEX.md)。
```

---

## 新增小节流程（任意项目）

1. 确认 Profile 中 `{slug}`、`{chapterId}`
2. 创建 `docs/{slug}/{chapterId}/{sectionId}.md`
3. 更新章 `INDEX.md`、根 `INDEX.md`、`INDEX.json`
4. 调整相邻小节「下一节」链接
5. **披露检查**：该主题 Level 是否与本章定位一致？过高则后移或合并为「变体」

完整 16 章落地参考 → [rust-webx-reference.md](rust-webx-reference.md)
