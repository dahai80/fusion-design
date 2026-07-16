# Fusion-Design PRD V0.1 MVP

> 本地 AI 可视化设计工作台 — 基于 OpenPencil (Rust) 底座 + fusion-mlx 本地多模态推理，原生嵌入 Fusion-Desk WKWebView。

---

## 一、文档基础信息

| 项 | 值 |
|----|-----|
| 产品名称 | Fusion-Design 本地 AI 可视化设计工作台 |
| 对标产品 | Claude Design（云端闭源）、Figma、Penpot |
| 所属生态 | Fusion-MLX「一核九端」产品矩阵 |
| 底层内核 | fusion-mlx（本地多模态大模型推理，**禁止调用第三方云端模型**） |
| 运行平台 | macOS Apple Silicon（M 系列芯片，Metal/ANE 硬件加速） |
| 开发模式 | 基于 OpenPencil (Rust) 二次封装，自研本地 AI 设计链路 |
| 核心定位 | 纯本地离线、AI 对话式 UI / 原型设计工作台，与 Fusion Code 双向打通，设计一键生成本地可运行代码 |

**Slogan**: Fusion-Design — Mac 本地离线 AI 设计工作台，设计与本地代码、机器人仿真无缝闭环。

---

## 二、产品定位与生态层级

### 2.1 层级关系（一核九端完整矩阵）

```
底层内核：fusion-mlx
├─ 旗舰主力
│   ├─ Fusion-MLX Agent Studio（智能体开发）
│   ├─ Fusion Code（本地 Vibe Coding）
│   └ Fusion-Design（本地 AI 设计画布）← 本产品
├─ 基础配套
│   ├─ Fusion Model Hub、Fusion CLI
└─ 垂直场景 / 技术工具
    ├─ Fusion Doc、Fusion Desk、Fusion-Simulation
    └ Fusion Bench、Fusion-KB
```

### 2.2 核心价值（差异化，区别 Claude Design）

- **100% 本地离线**：所有 AI 生成、设计文件、素材存储本机，不上传云端；Claude Design 强制云端，数据有泄露风险
- **原生适配 Apple MLX**：本地多模态视觉模型驱动设计，无需 API 付费、无网络依赖
- **深度打通全 Fusion 生态**：设计稿联动仿真、自动化、知识库、代码工具，Claude 仅联动 Claude Code
- **轻量化桌面原生 App**：内嵌 Fusion-Desk WKWebView 浏览器画布，无需独立网页访问
- **开源底座可私有化改造**，无闭源厂商锁定

### 2.3 解决核心痛点

- 现有 AI 设计工具依赖云端 API，断网无法使用、消耗高额 token 费用
- 设计与代码工具割裂，设计稿转前端需要人工二次调整
- Mac 本地没有基于 MLX 的原生 AI 设计画布，只能使用 Figma / 云端工具
- 机器人仿真、本地自动化场景缺少配套原型设计工具
- 设计资产云端存储，隐私、企业数据合规不满足

### 2.4 目标用户

- Mac 独立开发者、全栈工程师（核心用户）
- 机器人 / AI 仿真研发人员（搭配 Fusion-Simulation）
- 产品经理、独立设计师，需要快速产出低保真原型
- 本地 AI 研究者，需要离线可视化 UI 生成工具

---

## 三、V0.1 MVP 核心功能（P0 必做）

### 模块 1：无限矢量画布引擎（基于 OpenPencil `op-editor-core`）

- 基础画布操作：缩放 10%~800%、平移、网格辅助、多画板、图层管理
- 基础矢量元素：矩形、圆形、线条、文本、图片、组件容器
- 图层分组、锁定、隐藏、复制、对齐、分布布局
- 原生 CSS Flex/Grid 布局（生成代码无偏差）
- 画布文件本地存储 `.fusiondesign` JSON 格式，支持 Git 版本管理

### 模块 2：对话式 AI 设计生成（核心，基于 fusion-mlx 多模态）

- 文本生成界面：输入自然语言描述，本地 MLX 直接渲染 UI 页面
- 支持：后台仪表盘、移动端 APP、登录页、表单、机器人仿真控制面板、落地页
- 上传参考图 / 手绘草图逆向生成界面（图生 UI）
- 局部指令修改：框选画布元素，自然语言调整颜色、尺寸、布局、文案
- 多方案对比：一键生成 3 套不同风格设计稿并存画布
- 内置设计系统约束：自动匹配全局色值、字体、圆角 Token，杜绝不规范元素

### 模块 3：本地设计系统与组件库

- 内置三套基础规范：Apple HIG、极简后台、机器人仿真控制台
- 自定义 Token 管理：颜色、字号、间距、阴影，全局一键同步所有页面
- 可复用组件库：表单、按钮、卡片、表格、导航、仿真控制面板组件
- 导入 / 导出组件库 JSON，团队本地共享设计规范

### 模块 4：Fusion Code 双向打通（对标 Claude Design ↔ Claude Code）

- 画布设计稿一键导出 React/HTML/Tailwind 代码，直接同步到 Fusion Code 工程
- Fusion Code 修改组件样式后，反向同步更新 Design 画布图层样式
- 代码预览面板：画布侧边实时渲染生成页面效果
- 导出干净无冗余代码，适配 Mac 本地前端开发

### 模块 5：原型交互与交付

- 简单页面跳转、弹窗、表单交互原型（低保真）
- 画布导出：PNG、SVG、PDF、本地 HTML 静态预览文件
- 批量导出多画板图片，用于 Fusion-Doc 文档插入
- 截图工具：画布 / 仿真面板一键截图存入本地素材库

### 模块 6：生态联动能力（Fusion 全系打通）

- **Fusion-Simulation**：一键生成机器人仿真控制面板 UI，直接对接仿真环境
- **Fusion-Desk**：画布内嵌在 Desk 内置 WKWebView 浏览器，统一工作台
- **Fusion-KB**：保存常用界面模板、设计方案至本地知识库，支持语义检索
- **Fusion Model Hub**：本地多模态模型一键切换，切换生成画质风格
- **Fusion CLI**：支持命令行批量生成 UI 页面、导出代码、批量导出图片

### 模块 7：基础素材与本地资源管理

- 本地图片素材库管理，无云端素材库
- 素材全部缓存本机，支持文件夹导入图片
- 内置极简图标库，矢量图标可修改颜色尺寸

---

## 四、MVP 明确不做（边界控制）

- ❌ 不做重度专业插画、像素级修图、复杂动效（V0.2 迭代）
- ❌ 不支持云端多人实时协作（专注本地单人，企业版后续迭代）
- ❌ 不接入第三方云端大模型，仅使用 fusion-mlx 本地推理
- ❌ 不支持 Figma 完整插件生态，仅支持基础 Figma 文件导入（`op-figma` crate 复用）
- ❌ 不做云端素材市场、付费资产商店

---

## 五、技术架构（V0.1）

### 5.1 分层模型

```
┌─────────────────────────────────────────────────────┐
│  展示层：Fusion-Desk 内置 WKWebView 渲染无限矢量画布  │
├─────────────────────────────────────────────────────┤
│  业务层：OpenPencil editor-core + AI 对话 + 设计 Token │
│          + 代码导出（op-codegen）                     │
├─────────────────────────────────────────────────────┤
│  桥接层：op-ai 适配器 → fusion-mlx 多模态推理          │
│          op-mcp MCP 协议网关供生态联动                 │
├─────────────────────────────────────────────────────┤
│  底座层：fusion-mlx 本地文生图 / 图生 UI 多模态推理     │
│          Metal/ANE 苹果硬件加速                       │
└─────────────────────────────────────────────────────┘
```

### 5.2 底座选型决策（关键变更）

**选定 OpenPencil (Rust) 作为主底座**，而非 PRD 原方案的「tldraw + OpenUI + Plasmic 拼装」。理由：

| 维度 | OpenPencil (Rust) | tldraw + OpenUI + Plasmic 拼装 |
|------|-------------------|--------------------------------|
| 语言栈 | Rust（与 fusion-mlx 生态对齐） | TypeScript（需跨语言桥接） |
| 一体化 | 自带画布 + AI + 代码导出 + MCP + Figma 导入 | 需 3 个项目手工拼接 |
| 关键 crate | `op-editor-core`/`op-ai`/`op-codegen`/`op-mcp`/`op-figma`/`op-design-lint` | 各项目能力分散 |
| 体积 | 裁剪后可控（workspace 按需编译） | 3 库合并臃肿，Tree-Shaking 难 |
| 离线化 | 原生离线，仅替换 `op-ai` 后端 | 需删 OpenUI/Plasmic 全部云端逻辑 |

**OpenPencil 已有可直接复用的 crate**（见 `~/design/openpencil/crates/`）：

| Crate | 用途 | Fusion-Design 复用方式 |
|-------|------|----------------------|
| `op-editor-core` | 矢量画布内核 | 模块 1 画布底层，裁剪非核心 |
| `op-ai` | AI 调用抽象 | 模块 2 桥接层，替换后端为 fusion-mlx |
| `op-ai-skills` | AI 设计技能 | 模块 2 文生 UI / 图生 UI 技能定义 |
| `op-codegen` | 代码生成 | 模块 4 导出 React/HTML/Tailwind |
| `op-design-lint` | 设计规范检查 | 模块 3 设计系统约束执行 |
| `op-mcp` | MCP 协议 Server | 模块 6 生态联动（对接 Fusion-KB/CLI 等） |
| `op-figma` | Figma 文件解析 | V0.2 Figma 文件导入（MVP 可缓） |
| `op-host-desktop` | 桌面宿主 | 嵌入 Fusion-Desk WKWebView 的桥 |
| `op-host-web` | Web 宿主 (wasm) | 编译 wasm 供 WKWebView 加载 |
| `op-cli` | 命令行 | 模块 6 Fusion CLI 联动 |

### 5.3 核心集成原则（硬性约束）

1. **只复用能力，不复用整套工程**：剥离开源项目的独立服务端、账号系统、云端接口、数据库，仅抽取核心源码模块
2. **全链路离线**：删除所有网络请求、云端上报、更新检测、CDN 资源，静态资源本地托管
3. **统一通信协议**：所有模块通过内部 IPC / 本地 HTTP 私有接口通信，不暴露公网端口
4. **强绑定 fusion-mlx**：所有文生 UI、图生 UI、局部编辑的 AI 能力，全部替换原有开源 LLM / 云端模型，禁用原模型调用逻辑
5. **轻量化适配**：针对 WKWebView 裁剪体积、优化渲染性能，适配 macOS 触控 / 键鼠操作

---

## 六、性能验收指标

| 指标 | 目标值 |
|------|--------|
| 画布打开加载时间 | < 300ms（M3） |
| 文本生成完整 UI 页面耗时 | < 2s（本地 MLX 8GB 内存） |
| 图层拖拽、缩放 | 无卡顿，支持单画布 100+ 图层流畅操作 |
| AI 局部修改响应延迟 | < 500ms |
| 画布文件占用内存 | 低，长时间运行无内存泄漏 |

---

## 七、安全规范

- 所有设计文件、图片、模型推理全程本地存储，无任何外网上传
- AI 推理沙箱隔离，无法访问系统敏感文件
- 画布导出代码仅本地读写，禁止网络请求自动注入
- 无埋点、无用户行为数据采集，完全离线隐私保护

---

## 八、迭代路线

| 版本 | 内容 |
|------|------|
| **V0.1（当前 MVP）** | 本地画布、MLX 文生 UI、设计系统、Fusion Code 双向打通、联动仿真 |
| **V0.2** | Figma 文件导入导出（`op-figma`）、完整交互原型、批量生成页面、组件版本管理 |
| **V1.0** | 轻量化多人本地局域网协作、高级动效、设计报表、完整团队设计管控 |

---

## 九、工程目录结构（V0.1 落地）

```
fusion-design/
├── PRD.md                      ← 本文件
├── README.md
├── docs/
│   ├── OPENS_SOURCE_REFERENCES.md   ← 开源软件参考清单
│   └ INTEGRATION_PLAN.md            ← 集成方案（分阶段实施）
├── crates/                     ← Fusion-Design 自研 Rust crate（workspace）
│   ├── fd-ai-adapter/          ← op-ai → fusion-mlx 适配层
│   ├── fd-design-system/       ← 三套内置设计规范 + Token 管理
│   ├── fd-ecosystem/           ← 对接 Fusion Code/Simulation/KB/CLI
│   ├
    ├── fd-host-desk/           ← Fusion-Desk WKWebView 宿主桥
│   └ fd-export/               ← PNG/SVG/PDF/HTML 批量导出
└── vendor/openpencil/          ← 裁剪后的 OpenPencil 子集（Git subtree）
```

---

## 十、相关文档

- [docs/OPENS_SOURCE_REFERENCES.md](docs/OPENS_SOURCE_REFERENCES.md) — 开源软件参考清单（含真实状态核实）
- [docs/INTEGRATION_PLAN.md](docs/INTEGRATION_PLAN.md) — 集成方案（分阶段实施 + 关键风险规避）
