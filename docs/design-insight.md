# Design Insight — ~/design 开源软件深度分析报告

> 分析日期：2026-07-28 | 分析范围：~/design 目录下 9 个开源项目
> 目标：为 Fusion-Design V0.1 MVP 提供选型决策、可复用模块识别、风险预判

---

## 一、项目总览

| 项目 | 语言 | 定位 | 本地状态 | 对 Fusion-Design 价值 |
|------|------|------|---------|---------------------|
| **OpenPencil** | Rust | AI 原生矢量设计工具 | ✅ 完整 | ⭐⭐⭐⭐⭐ 主底座 |
| **Open Design** | TypeScript | Claude Design 开源替代 | ✅ 完整 | ⭐⭐⭐⭐ 直接竞品参考 |
| **Penpot** | Clojure/ClojureScript | 开源设计平台 | ✅ 完整 | ⭐⭐⭐ 布局/组件参考 |
| **Plasmic** | TypeScript | 可视化低代码构建器 | ✅ 完整 | ⭐⭐⭐ 代码生成参考 |
| **OpenUI** | Python/TypeScript | 文生 UI 组件工具 | ⚠️ 可能损坏 | ⭐⭐⭐ 文生 UI 参考 |
| **Stitches** | TypeScript | CSS 设计 Token 引擎 | ✅ 完整 | ⭐⭐ Token 管理备选 |
| **Archify** | TypeScript | 代码→架构图 Agent Skill | ✅ 完整 | ⭐⭐ 生态联动参考 |
| **Screenshot-to-code** | Python | 截图→代码神经网络 | ✅ 完整 | ⭐⭐ 图生 UI 参考 |
| **Figma-Context-MCP** | TypeScript | Figma→MCP 桥接 | ✅ 完整 | ⭐⭐ Figma 导入参考 |

---

## 二、逐项目深度分析

### 2.1 OpenPencil — ⭐⭐⭐⭐⭐ 主底座

**基本信息**

| 项 | 值 |
|----|-----|
| 仓库 | https://github.com/ZSeven-W/openpencil |
| 语言 | Rust（workspace，edition 2021） |
| 版本 | 0.8.1 |
| 定位 | "The world's first open-source AI-native vector design tool" |
| 核心卖点 | Concurrent Agent Teams · Design-as-Code · Built-in MCP Server · Multi-model Intelligence |

**架构分析（27 个 crate）**

```
openpencil/
├── 核心引擎层
│   ├── op-editor-core        ← 矢量画布内核（PenDocument/PenNode/图层树）
│   ├── op-editor-host-core   ← 编辑器宿主抽象
│   ├── op-editor-ui          ← 编辑器 UI 组件（jian UI 框架）
│   ├── op-acp                ← Agent Client Protocol 实现
│   ├── op-orchestrator       ← 多 Agent 并发编排器
│   └── op-rpc-transport      ← RPC 传输层
│
├── AI 能力层
│   ├── op-ai                 ← ChatProvider trait + 模型抽象（wasm-clean）
│   ├── op-ai-skills          ← AI 设计技能定义（文生 UI/图生 UI）
│   └── op-process-io         ← AI 进程 I/O
│
├── 代码导出层
│   ├── op-codegen            ← Codegen trait + 9 框架导出
│   └── op-pen-loader         ← .op 文件加载器
│
├── 设计系统层
│   ├── op-design-lint        ← 设计规范检测 + 自动修复
│   └── op-config-store       ← 配置/Token 存储
│
├── 生态集成层
│   ├── op-mcp                ← MCP 协议 Server（完整 tool 注册）
│   ├── op-figma              ← Figma 文件解析
│   ├── op-git                ← Git 集成
│   └── op-i18n               ← 国际化
│
├── 宿主层
│   ├── op-host-desktop       ← 原生桌面壳（macOS/Windows/Linux）
│   ├── op-host-native        ← 原生渲染宿主
│   ├── op-host-web           ← Web 宿主 (wasm)
│   ├── op-host-web-server    ← 本地 Web 服务
│   └── op-host-services      ← 宿主共享服务
│
├── SDK/CLI
│   ├── op-cli                ← 命令行工具
│   ├── op-web-sdk            ← 嵌入式查看器 SDK
│   └── op-opmerge            ← .op 文件合并工具
│
└── 测试
    └── op-smoke              ← 冒烟测试
```

**关键 trait 和 API**

1. **`ChatProvider`**（`op-ai/src/chat_provider.rs`）
   - 四种后端：BuiltIn（agent-rs 进程内）/ Subprocess（Claude Code 等 CLI）/ HttpServer（Codex/OpenCode）/ Acp（Agent Client Protocol）
   - `CliName` 枚举：ClaudeCode / Gemini / Copilot / Codex / OpenCode / Antigravity / GrokBuild
   - **Fusion-Design 改造点**：只需实现 BuiltIn 路径，对接 fusion-mlx；删除 Subprocess/HttpServer/Acp 三条路径

2. **`Codegen`** trait（`op-codegen/src/lib.rs`）
   - `fn target_label(&self) -> &'static str`
   - `fn generate(&self, doc: &PenDocument) -> String`
   - 已实现 9 种导出：CSS Variables / React / HTML / Vue / Svelte / Flutter / SwiftUI / Compose / React Native
   - **关键依赖**：消费 `jian_ops_schema::PenDocument`，使用 `op_editor_core::PenNodeExt`
   - **Fusion-Design 改造点**：需替换 `jian_ops_schema` 依赖为 `fd-canvas-core::PenDocument`，或保留 jian schema 做桥接

3. **`op-design-lint`**（设计规范检测）
   - 纯检测器 + 修复器，wasm-clean
   - 检测项：颜色对比度、空路径、过度效果、不可见容器、兄弟节点不一致、文本背景对比、文本圆角、文本效果、文本显式高度、文本描边、异常旋转、未标记输入、缺失进度环
   - `detect_and_fix` 一键检测+修复
   - `IssueCategory` / `IssueSeverity` / `FixReport` 结构化输出
   - **Fusion-Design 改造点**：同样依赖 `jian_ops_schema`，需桥接

4. **`op-mcp`**（MCP Server）
   - 完整的 MCP tool 注册：batch_design / batch_get / codegen_tools / component_tools / conversion_tools / bulk_vars
   - 消费 `op_editor_core::EditorState` / `EditorCommand`
   - **Fusion-Design 改造点**：替换 EditorState 为 fd-canvas-core 模型

5. **`op-orchestrator`**（多 Agent 编排）
   - Concurrent Agent Teams：将复杂页面分解为空间子任务，多 Agent 并行工作
   - **Fusion-Design 改造点**：V0.1 单人模式可暂不引入，V0.2 可考虑

**核心依赖问题：`jian-ops-schema`**

OpenPencil 的 `op-editor-core`、`op-codegen`、`op-design-lint`、`op-mcp` 全部依赖 `jian_ops_schema`（来自 `vendor/jian/` 私有 UI 框架）。这是 Fusion-Design 当前无法直接复用这些 crate 的**根本障碍**。

三种解决路径：
- **路径 A**：引入 `vendor/jian/` 子模块，接受 GPU-Skia 依赖（体积大，违背轻量化）
- **路径 B**：自研轻量 `PenDocument` 替代 `jian_ops_schema`（当前 fd-canvas-core 的方案）
- **路径 C**：仅复用 `op-ai`（无 jian 依赖），其余自研（当前实际路径）

**优势**

- Rust 原生，与 fusion-mlx 生态语言对齐
- 一体化 crate 设计，画布+AI+代码导出+MCP 全覆盖
- `op-ai` wasm-clean，可直接在 web 宿主中使用
- MCP Server 成熟，tool 注册丰富
- 多模型适配（Claude/GPT/Gemini/Ollama 等自动切换提示词策略）
- `.op` 文件 JSON 格式，Git-friendly

**劣势**

- `jian_ops_schema` 私有依赖阻断大部分 crate 直接复用
- 体积大（27 个 crate，含 GPU-Skia 渲染引擎）
- 多人协作/Agent 编排冗余（V0.1 不需要）
- 云端模型调用需全部剥离
- `op-codegen` 目前仅 CSS Variables 生成完整，其他 8 种为 Rust 端口进行中

---

### 2.2 Open Design — ⭐⭐⭐⭐ 直接竞品参考

**基本信息**

| 项 | 值 |
|----|-----|
| 仓库 | https://github.com/nexu-io/open-design |
| 语言 | TypeScript |
| License | Apache 2.0 |
| 版本 | 0.13.0 |
| 定位 | "The open-source Claude Design alternative" |
| 核心卖点 | Composable skills · Brand-grade DESIGN.md · Plugin 系统 · 多 Agent 支持 |

**架构分析**

```
open-design/
├── apps/                     ← 桌面应用（Electron/Tauri）
├── charts/                   ← 图表组件
├── clipper/                  ← 截图工具
├── assets/                   ← 静态资源
├── .claude/                  ← Claude Code 集成
├── .claude-plugin/           ← Claude 插件
└── .looper-attachments/      ← 循环附件
```

**核心特性**

1. **Skill 系统**：可组合的 AI 设计技能，类似 OpenPencil 的 `op-ai-skills` 但更灵活
2. **DESIGN.md 设计系统**：将品牌规范定义为 Markdown 文件，AI 生成时自动遵循
3. **Plugin 生态**：可安装、分发的插件系统
4. **多宿主支持**：Claude Code / Codex / Cursor / OpenCode / Qwen / Copilot / Amp 等 22+ 本地 CLI
5. **多格式导出**：HTML / PDF / PPTX / MP4 / PNG / SVG
6. **HyperFrames**：动效图形生成
7. **Automation**：可调度的自动化工作流
8. **Open Design Cloud**：官方模型服务（20+ 旗舰模型，按 token 计费）

**与 Fusion-Design 的对比**

| 维度 | Open Design | Fusion-Design |
|------|------------|---------------|
| 语言 | TypeScript | Rust |
| 离线能力 | 依赖云端模型 | 100% 本地离线 |
| AI 后端 | Claude/GPT/Gemini/DeepSeek | fusion-mlx |
| 画布类型 | HTML/CSS 实时渲染 | 矢量画布（OpenPencil 底座） |
| 设计系统 | DESIGN.md（Markdown） | fd-design-system（Rust struct） |
| MCP 支持 | ✅ | ✅（op-mcp） |
| 代码导出 | HTML/PDF/PPTX/MP4 | React/HTML/Tailwind |

**可借鉴点**

- **Skill 系统设计**：可组合的技能注册模式，比 op-ai-skills 的固定技能更灵活
- **DESIGN.md 模式**：用 Markdown 定义设计规范，人机可读，值得 fd-design-system 参考
- **Plugin 架构**：可安装/分发的插件机制，V0.2 可考虑
- **多格式导出**：PPTX/MP4 导出实现，fd-export 可参考
- **Automation 工作流**：可调度的自动化设计流程

**不可借鉴点**

- 云端模型依赖与 Fusion-Design 离线原则冲突
- TypeScript 技术栈与 Rust 生态不对齐
- Electron/Tauri 桌面壳与 WKWebView 方案不同

---

### 2.3 Penpot — ⭐⭐⭐ 布局/组件参考

**基本信息**

| 项 | 值 |
|----|-----|
| 仓库 | https://github.com/penpot/penpot |
| 语言 | Clojure（后端）+ ClojureScript（前端） |
| License | MPL-2.0 |
| 文件数 | 5885 |
| 定位 | "Open-source design platform for teams" |
| 核心卖点 | CSS Grid/Flex Layout · Design Tokens · MCP Server · 实时协作 |

**架构分析**

```
penpot/
├── frontend/                 ← ClojureScript SPA
│   └── src/app/main/
│       ├── data/            ← 数据模型
│       ├── features/        ← 功能模块（画布/组件/布局/导出）
│       ├── ui/              ← UI 组件
│       ├── render.cljs      ← 渲染引擎（28K）
│       ├── style.clj        ← 样式系统
│       ├── snap.cljs        ← 吸附/对齐
│       └── store.cljs       ← 状态管理
├── backend/                  ← Clojure 后端
│   └── src/app/
│       ├── data_readers.clj
│       └── ...
├── common/                   ← 前后端共享
├── docker/                   ← Docker 部署
├── exporter/                 ← 导出服务
├── library/                  ← 组件库
├── mcp/                      ← MCP Server
├── plugins/                  ← 插件系统
├── render-wasm/              ← WebAssembly 渲染
└── tools/                    ← 工具集
```

**核心特性**

1. **CSS Grid & Flex Layout**：原生支持 CSS 布局模型，设计即代码
2. **Design Tokens**：设计与开发之间的唯一真相源
3. **MCP Server**：支持多向工作流（设计↔代码）
4. **SVG-native**：基于 SVG 的设计文件，开放标准
5. **实时协作**：多人同时编辑
6. **Plugin 系统**：可编程工作区
7. **render-wasm**：WebAssembly 渲染引擎

**可借鉴点**

- **Flex/Grid 布局计算**：Penpot 的 CSS 布局实现是开源领域最成熟的，fd-canvas-core 的 `NodeStyle` 可参考其布局模型
- **Design Token 体系**：成熟的 Token 定义/切换/同步机制
- **SVG 渲染管线**：render-wasm 的 WASM 渲染方案与 fd-host-web 架构相似
- **吸附系统**：snap.cljs（13K）的对齐/吸附逻辑可直接移植思路

**不可借鉴点**

- Clojure/ClojureScript 语言栈与 Rust 完全不对齐，无法直接复用代码
- 工程庞大（5885 文件），自带完整后端/数据库/协作系统
- 整包集成引入的冗余远大于收益

---

### 2.4 Plasmic — ⭐⭐⭐ 代码生成参考

**基本信息**

| 项 | 值 |
|----|-----|
| 仓库 | https://github.com/plasmicapp/plasmic |
| 语言 | TypeScript |
| 文件数 | 7674 |
| 定位 | "The open-source visual builder for your codebase" |
| 核心卖点 | 拖拽式可视化构建 · React/Next.js 代码生成 · 组件系统 |

**架构分析**

```
plasmic/
└── packages/
    ├── plasmic/              ← 核心 SDK（空壳，逻辑在子包）
    ├── react-web/            ← React Web 渲染器
    │   └── src/
    │       ├── render/       ← 渲染引擎（style-tokens/PlasmicSlot/PlasmicIcon）
    │       ├── auth/         ← 认证
    │       ├── host/         ← 宿主集成
    │       └── plume/        ← Plume 组件系统
    ├── react-web-runtime/    ← React Web 运行时
    ├── cli/                  ← CLI 工具（plasmic sync）
    ├── loader-core/          ← 加载器核心
    ├── loader-react/         ← React 加载器
    ├── loader-nextjs/        ← Next.js 加载器
    ├── loader-svelte/        ← Svelte 加载器
    ├── loader-vue/           ← Vue 加载器
    ├── loader-gatsby/        ← Gatsby 加载器
    ├── host/                 ← 宿主层
    ├── watcher/              ← 文件监听
    ├── prepass/              ← SSR 预渲染
    ├── query/                ← 数据查询
    ├── data-sources/         ← 数据源集成
    ├── data-sources-context/ ← 数据源上下文
    ├── auth-api/             ← 认证 API
    ├── auth-react/           ← React 认证
    ├── create-plasmic-app/   ← 项目脚手架
    ├── nextjs-app-router/    ← Next.js App Router 集成
    └── loader-splits/        ← A/B 测试加载器
```

**核心特性**

1. **代码生成**：Plasmic 设计稿 → React/Next.js 代码（`plasmic sync`）
2. **组件系统**：`react-web/src/plume/` — Plume 组件引擎
3. **样式 Token**：`react-web/src/render/style-tokens.tsx` — Token→CSS 映射
4. **多框架加载**：React/Vue/Svelte/Next.js/Gatsby 加载器
5. **Headless Codegen**：CLI 驱动的代码生成，不依赖 Plasmic 云端

**可借鉴点**

- **style-tokens.tsx**：Token→CSS 的映射逻辑，fd-design-system 可参考
- **plume/ 组件引擎**：组件注册/渲染/组合模式
- **loader 架构**：多框架加载器的设计模式
- **CLI codegen**：`plasmic sync` 的代码生成流程

**不可借鉴点**

- 低代码平台定位，大量 UI 拖拽/表单逻辑与 Fusion-Design 无关
- 云端托管核心，删除云端逻辑工作量巨大
- TypeScript 与 Rust 不可直接复用

---

### 2.5 OpenUI — ⭐⭐⭐ 文生 UI 参考

**基本信息**

| 项 | 值 |
|----|-----|
| 仓库 | https://github.com/wandb/openui |
| 语言 | Python（后端）+ TypeScript（前端） |
| License | Apache 2.0 |
| 定位 | "Describe UI using your imagination, then see it rendered live" |
| 核心卖点 | 自然语言→UI · 实时渲染 · 多框架转换 |

**架构分析**

```
openui/
├── backend/                  ← Python FastAPI 后端
│   └── openui/              ← 核心：LLM 调用 + HTML 生成
├── frontend/                 ← TypeScript 前端
│   └── public/annotator/    ← 标注工具
└── docker-compose.yaml      ← Docker 部署
```

**核心特性**

1. **多 LLM 支持**：OpenAI / Groq / Gemini / Anthropic / Cohere / Mistral / Ollama / LiteLLM 兼容
2. **实时渲染**：描述即渲染，所见即所得
3. **多框架转换**：HTML → React / Svelte / Web Components
4. **Ollama 本地模型**：支持本地 Ollama 推理

**可借鉴点**

- **LLM 调用模式**：LiteLLM 统一多模型接口的思路，fd-ai-adapter 可参考
- **HTML→多框架转换**：先生成 HTML 再转换的策略，比直接生成多框架更稳定
- **实时渲染交互**：描述→渲染的交互流程设计
- **Ollama 集成**：本地模型集成的实现参考

**不可借鉴点**

- Python 后端与 Rust 技术栈不对齐
- 本地 git 仓库状态可能损坏
- 无画布概念，仅 HTML 渲染，与矢量画布需求差距大

---

### 2.6 Stitches — ⭐⭐ Token 管理备选

**基本信息**

| 项 | 值 |
|----|-----|
| 仓库 | https://github.com/modulz/stitches |
| 语言 | TypeScript |
| 定位 | CSS-in-JS 设计 Token 引擎 |
| 核心卖点 | 主题切换 · 响应式 Token · CSS 变量生成 |

**架构分析**

```
stitches/
└── packages/
    ├── stitches/             ← 核心 CSS-in-JS 引擎
    └── ...
```

**核心特性**

1. **Token 系统**：颜色/间距/字体/圆角/阴影统一定义
2. **主题切换**：运行时主题切换，基于 CSS 变量
3. **响应式 Token**：不同断点不同值
4. **CSS 变量生成**：Token → CSS Custom Properties

**可借鉴点**

- **Token→CSS 变量映射**：fd-design-system 的 `TokenValue`→CSS 生成可参考此模式
- **主题切换机制**：运行时无闪烁主题切换的实现思路
- **响应式 Token**：不同断点使用不同值的设计

**不可借鉴点**

- CSS-in-JS 运行时与 Rust 编译时不匹配
- 项目已归档（不再维护），不作为主选
- fd-design-system 当前实现已覆盖核心需求

---

### 2.7 Archify — ⭐⭐ 生态联动参考

**基本信息**

| 项 | 值 |
|----|-----|
| 仓库 | https://github.com/tt-a1i/archify |
| 语言 | TypeScript |
| 版本 | 2.12.0 |
| License | MIT |
| 定位 | "Turn a codebase or system description into a polished, interactive system map" |
| 核心卖点 | Agent Skill · 5 种图表类型 · Before/Delta/After 对比 · 自包含 HTML |

**核心特性**

1. **Agent Skill 模式**：npx skills add 安装，被 Claude Code/Cursor/Codex 等调用
2. **5 种图表**：Architecture / Sequence / Workflow / Topology / Deployment
3. **Before/Delta/After**：架构变更对比视图
4. **确定性检查**：typed JSON IR + 验证 = 可信赖的输出
5. **自包含 HTML**：单文件输出，可分享

**可借鉴点**

- **Agent Skill 注册模式**：fd-ecosystem 可参考此模式暴露 Fusion-Design 能力给其他 Agent
- **确定性 IR**：typed JSON 中间表示 + 验证的模式，可用于 .fusiondesign 文件校验
- **Before/After 对比**：设计变更的可视化对比，Fusion-Design 代码双向同步时有用

**不可借鉴点**

- 与设计画布无关，纯架构可视化工具
- TypeScript 不可直接复用

---

### 2.8 Screenshot-to-code — ⭐⭐ 图生 UI 参考

**基本信息**

| 项 | 值 |
|----|-----|
| 仓库 | https://github.com/emilwallner/Screenshot-to-code |
| 语言 | Python（Keras/TensorFlow） |
| 定位 | "Turning design mockups into code with deep learning" |
| 核心卖点 | 截图→HTML/CSS · 神经网络 · Bootstrap 97% 准确率 |

**架构分析**

```
Screenshot-to-code/
├── Bootstrap/                ← 最终版本（97% 准确率）
│   ├── compiler/            ← Token→HTML/CSS 编译器
│   └── resources/           ← 训练/测试数据
├── Hello_world/              ← 入门版本
└── HTML/                     ← HTML 版本
```

**核心技术**

1. **CNN + LSTM/GRU**：图像→序列生成（image captioning 范式）
2. **16 个领域 Token**：将 HTML/CSS 抽象为 16 个语义 Token，再编译为完整代码
3. **三阶段迭代**：Hello World → HTML → Bootstrap（逐步增加约束降低搜索空间）

**可借鉴点**

- **Token 化思路**：将 UI 抽象为少量语义 Token 再展开，比直接生成完整 HTML 更稳定
- **约束降低搜索空间**：Bootstrap 版本用领域约束达到 97% 准确率，Fusion-Design 可用设计系统 Token 做同样的事
- **迭代方法论**：简单→复杂的三阶段训练策略

**不可借鉴点**

- 传统 CNN+RNN 方案已被大模型超越，fusion-mlx 多模态能力远优于此
- 训练数据集小且同质化，泛化能力有限
- Python/Keras 与 Rust 不对齐

---

### 2.9 Figma-Context-MCP — ⭐⭐ Figma 导入参考

**基本信息**

| 项 | 值 |
|----|-----|
| 仓库 | https://github.com/GLips/Figma-Context-MCP |
| 语言 | TypeScript |
| License | MIT |
| 定位 | "Give your coding agent access to your Figma data" |
| 核心卖点 | Figma API→MCP 桥接 · 简化布局信息 · Cursor 集成 |

**核心特性**

1. **Figma API→MCP**：将 Figma 文件数据通过 MCP 协议暴露给 AI Agent
2. **信息简化**：只提取最相关的布局和样式信息，减少 token 消耗
3. **Cursor 集成**：直接在 Cursor 中粘贴 Figma 链接实现设计

**可借鉴点**

- **Figma 数据简化策略**：提取哪些布局/样式字段、如何减少 token 的思路
- **MCP Tool 设计**：Figma 数据作为 MCP tool 的接口设计
- **op-figma 补充**：Figma-Context-MCP 的简化逻辑可与 op-figma 互补

**不可借鉴点**

- 依赖 Figma 云端 API，与离线原则冲突
- 仅读取 Figma 数据，不解析本地 .fig 文件
- V0.1 不涉及 Figma 导入（V0.2 才启用 op-figma）

---

## 三、选型决策矩阵（更新版）

| 能力需求 | 首选 | 备选 | 决策依据 |
|---------|------|------|---------|
| 矢量画布 | **fd-canvas-core（自研）** | OpenPencil op-editor-core | jian 依赖阻断，自研更可控 |
| AI 文生 UI | **fd-ai-adapter + op-ai** | OpenUI | op-ai wasm-clean，fd-ai-adapter 已实现 |
| AI 图生 UI | **fd-ai-adapter** | Screenshot-to-code | 大模型直接生成优于传统 CNN |
| 设计系统/Token | **fd-design-system（自研）** | Stitches/Penpot | 自研已覆盖核心，Token→CSS 可参考 Stitches |
| 设计规范检测 | **fd-design-system** | OpenPencil op-design-lint | op-design-lint 依赖 jian，检测逻辑可移植 |
| 代码导出 | **fd-codegen（自研）** | OpenPencil op-codegen / Plasmic | op-codegen 依赖 jian，Plasmic 可参考转换思路 |
| MCP 生态联动 | **op-mcp（复用）** | — | op-mcp 无 jian 依赖，可直接复用 |
| Figma 导入 | V0.2 启用 op-figma | Figma-Context-MCP | op-figma 已有，Context-MCP 的简化策略可参考 |
| 画布渲染 | **fd-host-web（wasm）** | Penpot render-wasm | fd-host-web 已实现基础渲染 |
| 布局计算 | **fd-canvas-core 扩展** | Penpot Flex/Grid | Penpot 布局逻辑需用 Rust 重写 |
| Skill 系统 | V0.2 参考 Open Design | OpenPencil op-ai-skills | Open Design 的 Skill 更灵活 |
| 变更对比 | V0.2 参考 Archify | — | Before/Delta/After 模式 |

---

## 四、风险预判与规避

### 4.1 高风险

| 风险 | 影响 | 规避方案 |
|------|------|---------|
| OpenPencil jian 依赖阻断 | 无法直接复用 op-editor-core/op-codegen/op-design-lint | ✅ 已执行：自研 fd-canvas-core/fd-codegen/fd-design-system |
| Open Design 快速迭代 | 竞品功能持续扩展，差异化压力 | 坚守离线+MLX 差异化，不追功能量 |
| Penpot 布局移植成本 | Clojure→Rust 重写工作量大 | V0.1 用简化布局，V0.2 按需移植核心算法 |

### 4.2 中风险

| 风险 | 影响 | 规避方案 |
|------|------|---------|
| op-mcp 与 fd-canvas-core 模型不匹配 | MCP tool 需要 EditorState，fd 用 PenDocument | 写桥接层将 PenDocument→EditorState |
| OpenUI 本地损坏 | 无法参考文生 UI 交互设计 | op-ai-skills 已覆盖，可忽略 |
| Stitches 归档无维护 | 参考价值递减 | fd-design-system 已自研，Stitches 仅作概念参考 |

### 4.3 低风险

| 风险 | 影响 | 规避方案 |
|------|------|---------|
| Screenshot-to-code 技术过时 | CNN+RNN 已被大模型超越 | 不采用其技术，仅借鉴 Token 化思路 |
| Archify 与设计无关 | 无直接影响 | 仅参考 Agent Skill 注册模式 |
| Figma-Context-MCP 依赖云端 | 与离线原则冲突 | 仅参考数据简化策略，不引入 |

---

## 五、行动建议（按优先级）

### P0 — 立即执行

1. **保持当前自研路线**：fd-canvas-core/fd-codegen/fd-design-system/fd-ai-adapter 已覆盖 MVP 核心需求
2. **op-ai 继续作为唯一复用的 OpenPencil crate**：wasm-clean，无 jian 依赖，fd-ai-adapter 已对接
3. **op-mcp 评估引入**：检查是否可脱离 jian 依赖独立使用，若可以则作为 MCP 生态层引入

### P1 — 近期执行

1. **参考 Open Design Skill 系统**：设计 fd-ai-adapter 的 Skill 注册机制，使文生 UI/图生 UI 可扩展
2. **参考 Penpot 布局模型**：在 fd-canvas-core 中扩展 Flex/Grid 布局支持
3. **参考 Stitches Token→CSS**：在 fd-design-system 中完善 Token→CSS Custom Properties 生成

### P2 — 远期规划

1. **参考 Open Design Plugin 架构**：V0.2 引入可安装的插件系统
2. **参考 Archify Before/After**：Fusion Code 双向同步的变更可视化
3. **评估 op-figma 引入**：V0.2 Figma 文件导入
4. **评估 Penpot render-wasm**：若 fd-host-web 渲染性能不足，参考其 WASM 渲染优化

---

## 六、总结

**核心结论**：Fusion-Design 的自研路线是正确的。OpenPencil 的 `jian_ops_schema` 依赖是硬阻断，直接复用除 `op-ai` 外的其他 crate 成本高于自研。9 个开源项目中，**Open Design 是最直接的竞品**（功能对标 Claude Design），**Penpot 的布局算法**和**Open Design 的 Skill 系统**是最有价值的参考点。当前 fd-* crate 体系已覆盖 MVP 全部核心需求，应继续沿此路线推进，按需从开源项目中移植特定算法和设计模式。
