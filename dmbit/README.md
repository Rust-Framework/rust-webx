# 智算机房管理（dmbit）

直播数据智算机房设备台账平台，基于 rust-webx。覆盖智算机房设备台账与大屏展示。

## 功能

- **ECharts 大屏**（`/`，无 Three.js）：白/蓝/紫层次 KPI（总数与 PB/MW 突出）+ 产品类型 / 运行状态主图 + 规格明细表
- **可展开单产品表后台**（`/admin/index.html`，Ant Design 浏览器版，无构建）
  - 登录 / 修改密码
  - 产品行可展开嵌套商品行（规格列展开展示）
  - 状态、机位、资产编码、机箱/主板/内存/接口/扩展/电源/光模块
  - 按产品名 / 编码 / 品牌搜索
  - 种子：定制算力 / RTX 5090 / RTX 4090 / 存储 四条独立产品

## 功能开发计划

详见 [功能开发计划.md](./功能开发计划.md)（交互版 Canvas：`dmbit-functional-plan`）。

## 运行

```bash
cargo run -p dmbit-host
```

- 大屏：http://localhost:5100/
- 后台：http://localhost:5100/admin/login.html
- 账号：`admin@dmbit.local` / `admin123`

## 结构

```
dmbit/
├── contracts/   # API 契约
├── domain/      # 实体 + 种子数据
├── handlers/    # Handler 实现
├── host/        # 宿主
├── wwwroot/     # 大屏 + 后台静态页
├── appsettings.json
```
