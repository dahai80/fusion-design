# Integration Plan — Fusion-Design 集成方案

> 基于 OpenPencil (Rust) 主底座 + fusion-mlx 本地推理，分阶段落地 V0.1 MVP。

## 一、整体集成总架构（V0.1 最终落地架构）

### 1.1 分层模型（从上到下）

```
Fusion-Desk（macOS 原生 SwiftUI）
    ↓
WKWebView 内嵌容器（承载 Fusion-Design 前端应用）
    ↓
【前端层】OpenPencil op-host-web (wasm) 编译产物
  ├─ 画布内核：op-editor-core（矢量图层、Flex/Grid、组件）
  ├─ AI对话面板：op-ai-skills（文生 UI / 图生 UI 交互）
  ├─ 设计系统：op-design-lint（Token / 样式规范约束）
  ├─ 代码导出：op-codegen（React/HTML/Tailwind 生成）
  └：MCP 网关：op-mcp（生态联动协议层）
    ↓
【前后端桥接层】op-host-web-server（本地私有接口，仅 127.0.0.1）
  接收前端指令 → 转发至本地 AI 服务 → 返回结果给前端
    ↓
【本地后端服务层】Fusion-Design 自研 crate
  ├─ fd-ai-adapter：op-ai 抽象 → fusion-mlx 调用适配
  ├─ fd-design-system：三套内置规范 + Token 管理
  ├─ fd-ecosystem：对接 Fusion Code/Simulation/KB/CLI
  ├─ fd-host-desk：Fusion-Desk WKWebView 宿主桥
  └ fd-export：PNG/SVG/PDF/HTML 批量导出
    ↓
【底层底座】fusion-mlx 多模态推理引擎 + Metal/ANE 硬件加速
```

### 1.2 核心集成原则（硬性约束）

1. **只复用能力，不复用整套工程**：剥离 OpenPencil 的独立桌面壳、账号系统、协作逻辑，仅抽取核心 crate
2. **全链路离线**：删除所有网络请求、云端上报、更新检测、CDN 资源，静态资源本地托管
3. **统一通信协议**：所有模块通过本地 HTTP 私有接口（127.0.0.1）通信，不暴露公网端口
4. **强绑定 fusion-mlx**：所有 AI 能力替换 OpenPencil 原有 LLM 调用为 fusion-mlx 本地推理
5. **轻量化适配**：针对 WKWebView 裁剪 wasm 体积、优化渲染性能，适配 macOS 触控 / 键鼠

---

## 二、分模块详细整合方案

### 模块 1：矢量画布引擎（OpenPencil `op-editor-core`）

**集成步骤**：
1. 将 `~/design/openpencil/crates/op-editor-core` 源码抽取到 `vendor/openpencil/` 子集
2. 裁剪：删除非核心图层类型、插件市场、多人协作逻辑
3. 编译 `op-host-web` 为 wasm，输出静态资源到 `frontend/dist/`
4. 在 Fusion-Desk WKWebView 中以 `file://` 协议加载，禁止网络访问
5. 适配 macOS 键鼠、滚轮缩放、右键菜单
6. 扩展「Fusion 专用组件」：仿真面板控件、后台表单、机器人状态指示灯

**Penpot 布局移植**（若 `op-editor-core` Flex/Grid 不足）：
- 仅摘抄 Penpot 的 Flex/Grid 布局计算、组件实例、样式继承逻辑
- 移植到 OpenPencil，补齐专业布局能力
- 不引入 Penpot 的 Clojure 后端/数据库

### 模块 2：AI 对话 & 文生 UI（OpenPencil `op-ai` + `op-ai-skills`）

**核心改造：替换模型后端为 fusion-mlx**

1. 移除 `op-ai` 中所有 Anthropic/OpenAI/云端 API 请求代码
2. 自研 `fd-ai-adapter` crate，实现 `op-ai` 的 trait 接口，底层调用 fusion-mlx
3. `op-ai-skills` 中定义文生 UI / 图生 UI / 局部编辑的 skill 描述
4. 数据流：
   ```
   用户输入文案/上传草图 → op-ai-skills 前端交互
           ↓
   fd-ai-adapter（本地 127.0.0.1 私有接口）
           ↓
   fusion-mlx 多模态推理 → 输出结构化 UI 数据
           ↓
   数据回传给 op-editor-core 画布 → 自动生成图形、文本、布局
   ```

### 模块 3：设计系统 & 样式 Token（OpenPencil `op-design-lint`）

1. 基于 `op-design-lint` 实现全局 Token 管理（颜色、字号、间距、圆角、阴影）
2. 自研 `fd-design-system` crate，预定义三套 Fusion 内置规范：
   - Apple HIG
   - 极简后台系统
   - 机器人仿真面板
3. 双向绑定：
   - 手动修改画布样式 → 同步更新 Token
   - 切换全局设计规范 → 画布所有元素批量刷新
4. 对接 AI 生成：在 `op-ai-skills` 的提示词中自动注入当前 Token，让 AI 生成的 UI 天然符合规范

**Stitches 备用**：若 `op-design-lint` Token 管理能力不足，移植 Stitches 的样式管控逻辑。

### 模块 4：画布转代码（OpenPencil `op-codegen`）

**正向导出（画布 → Fusion Code）**：
1. 画布选中页面/组件 → 调用 `op-codegen` 解析图层树
2. 生成 React/HTML/Tailwind 代码
3. 通过 `fd-ecosystem` 推送到 Fusion Code 工程目录，自动创建文件

**反向同步（Fusion Code → 画布）**：
1. `fd-ecosystem` 监听 Fusion Code 工程文件变更
2. 解析样式/结构 → 转化为画布图层数据
3. 刷新 Fusion-Design 界面

**Plasmic 备用**：若 `op-codegen` 代码生成质量不足，移植 Plasmic 的图层解析 + Tailwind 生成逻辑。

### 模块 5：原型交互与导出

1. 基础原型跳转、弹窗、表单交互（低保真）复用 `op-editor-core`
2. 自研 `fd-export` crate：PNG/SVG/PDF/HTML 批量导出
3. 截图工具：画布/仿真面板一键截图存入本地素材库

### 模块 6：生态联动（OpenPencil `op-mcp` + 自研 `fd-ecosystem`）

| 联动目标 | 协议 | 实现 |
|---------|------|------|
| Fusion-Simulation | MCP Tools | `op-mcp` 暴露「生成仿真控制面板」工具 |
| Fusion-Desk | WKWebView 嵌入 | `fd-host-desk` 宿主桥 |
| Fusion-KB | MCP Tools | `op-mcp` 暴露「保存/检索设计模板」工具 |
| Fusion Model Hub | 本地接口 | `fd-ai-adapter` 支持运行时切换 MLX 模型 |
| Fusion CLI | CLI 子命令 | 复用 `op-cli` 扩展批量生成/导出命令 |

### 模块 7：素材与本地资源管理

1. 本地图片素材库，无云端素材库
2. 文件夹导入图片，全部缓存本机
3. 内置极简图标库，矢量图标可修改颜色尺寸

---

## 三、统一通信层（自研核心）

### 3.1 前端通信层（wasm 内）

- 统一事件总线：画布操作、AI 指令、导出请求、文件操作全走统一事件
- 封装本地请求工具：只请求 127.0.0.1 本地服务，拦截所有外网请求
- 数据格式统一：定义标准图层结构、AI 请求/返回结构体

### 3.2 后端本地服务（Rust，与 fusion-mlx 生态对齐）

- 接收前端各类请求，做路由分发（复用 `op-host-web-server`）
- 封装 fusion-mlx SDK，对外提供统一 AI 推理接口（`fd-ai-adapter`）
- 文件管理：`.fusiondesign` 工程文件、素材、导出代码本地读写
- 生态联动接口：对接 Fusion-CLI / Fusion-KB / Fusion-Simulation（`fd-ecosystem`）
- 任务队列：AI 生成、大图解析、批量导出异步执行，避免界面卡顿

---

## 四、工程目录结构（整合后标准目录）

```
fusion-design/
├── PRD.md
├── README.md
├── docs/
│   ├── OPENS_SOURCE_REFERENCES.md
│   └ INTEGRATION_PLAN.md           ← 本文件
├── crates/                         ← Fusion-Design 自研 Rust crate（workspace）
│   ├── fd-ai-adapter/              ← op-ai → fusion-mlx 适配层
│   ├── fd-design-system/           ← 三套内置设计规范 + Token 管理
│   ├── fd-ecosystem/               ← 对接 Fusion Code/Simulation/KB/CLI
│   ├── fd-host-desk/               ← Fusion-Desk WKWebView 宿主桥
│   ├── fd-export/                  ← PNG/SVG/PDF/HTML 批量导出
│   └── fd-cli/                     ← 命令行（扩展 op-cli）
├── vendor/
│   └ openpencil/                   ← 裁剪后的 OpenPencil 子集（Git subtree）
│       ├── crates/op-editor-core/
│       ├── crates/op-ai/
│       ├── crates/op-ai-skills/
│       ├── crates/op-codegen/
│       ├── crates/op-design-lint/
│       ├── crates/op-mcp/
│       ├── crates/op-host-web/
│       ├── crates/op-host-web-server/
│       └ (已删除 op-host-desktop 独立壳、协作、账号、云端)
└── frontend/
    └ dist/                        ← 编译后静态文件（供 WKWebView 加载）
```

---

## 五、分阶段集成实施步骤（按开发顺序，可直接排期）

### 阶段 1：基础底座搭建（P0）

- [ ] 新建 `fusion-design` Rust workspace
- [ ] 用 `git subtree` 引入 OpenPencil 裁剪子集到 `vendor/openpencil/`
- [ ] 删除 OpenPencil 云端接口、数据库、协作、付费、网络依赖
- [ ] 编译 `op-host-web` 为 wasm，在 Fusion-Desk WKWebView 中正常加载、操作
- [ ] 搭建本地后端基础服务（`op-host-web-server`），打通前端 ↔ 后端通信

### 阶段 2：AI 能力替换（P0 核心）

- [ ] 自研 `fd-ai-adapter` crate，实现 `op-ai` trait 接口
- [ ] 底层对接 fusion-mlx 多模态推理
- [ ] 完成 文生 UI、图生 UI 本地离线生成
- [ ] 测试：输入文案/上传草图，画布自动生成界面

### 阶段 3：设计系统集成（P1）

- [ ] 自研 `fd-design-system` crate，配置三套默认设计规范
- [ ] 实现全局样式一键切换、Token 编辑
- [ ] 让 AI 生成内容自动遵循设计规范（提示词注入 Token）

### 阶段 4：代码导出 & Fusion Code 联动（P1）

- [ ] 集成 `op-codegen`，实现画布导出 HTML/React/Tailwind 代码
- [ ] 自研 `fd-ecosystem`，打通与 Fusion Code 的双向同步

### 阶段 5：生态联动 & 收尾适配（P2）

- [ ] 对接 Fusion-Simulation：快速生成仿真控制面板
- [ ] 对接 Fusion-KB：保存/检索设计模板
- [ ] 对接 Fusion-CLI：命令行批量生成、导出
- [ ] 全量性能优化：WKWebView 渲染优化、内存控制、长时间运行稳定性

### 阶段 6：打包发布

- [ ] 所有前端静态资源、后端服务、桥接逻辑打包进 Fusion-Desk 主程序
- [ ] 保证单程序运行，无额外依赖，macOS Apple Silicon 原生编译

---

## 六、关键风险与避坑方案

| 风险 | 问题 | 解决 |
|------|------|------|
| OpenPencil wasm 体积过大 | WKWebView 加载慢 | Tree-Shaking 裁剪非核心 crate，懒加载 |
| WebKit 渲染兼容 | 部分高级特性在 WebKit 下异常 | 优先标准 Web API，针对 WebKit 做样式兼容 |
| AI 推理阻塞界面 | fusion-mlx 推理耗时久，前端卡死 | 后端异步任务队列，前端加载动画+进度提示 |
| 数据格式不统一 | OpenPencil 图层结构与 fusion-mlx 输出差异 | 定义全局统一数据模型，增加转换适配器 |
| OpenPencil 裁剪回归 | 删除依赖导致编译断裂 | 用 `cargo build -p <crate>` 按需验证，保留最小可编译子集 |
| fusion-mlx 接口不匹配 | op-ai trait 与 MLX SDK 签名差异 | fd-ai-adapter 做适配层，不直接耦合 |

---

## 七、极简落地总结（一句话）

**底座**：以 OpenPencil (Rust) 一体化 crate 为画布+AI+代码导出+MCP 底座，裁剪所有云端/协作/独立桌面壳冗余。
**AI**：自研 `fd-ai-adapter` 把 `op-ai` 的模型调用全面替换为 fusion-mlx 本地推理。
**桥接**：通过 `op-host-web-server` 本地私有接口 + WKWebView 通信，实现 Fusion-Desk、Fusion Code、Fusion-Simulation 全生态打通。
**上线**：整体打包为 Fusion 生态内嵌模块，纯本地离线、无外网依赖。
