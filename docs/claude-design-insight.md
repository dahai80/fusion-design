# Claude Design Insight — 深度洞察与 Fusion-Design 超越策略

> 分析日期：2026-07-28 | 基于 Claude.ai Design System 实际使用 + 官方文档 + 竞品分析
> 目标：解构 Claude Design 能力边界，设计 Fusion-Design 本地化超越路径

---

## 一、Claude Design 是什么

### 1.1 产品定位

Claude Design 是 Anthropic 在 claude.ai 平台内嵌的设计系统管理能力，通过 `/design-sync` 技能与 Claude Code 协同工作。核心模式：

```
Claude Code（编码 Agent）
    ↕ /design-sync
Claude.ai Design System Pane（可视化组件库管理）
    ↕ @dsCard annotation
HTML Preview Cards（组件预览卡片）
```

### 1.2 核心能力清单

| 能力 | 描述 | 实现方式 |
|------|------|---------|
| **Design System 项目** | 在 claude.ai 创建 design-system 类型项目 | `create_project` API |
| **组件预览卡片** | 每个组件一张卡片，按 group 分组显示 | `@dsCard group="Buttons"` HTML 注释 |
| **Design-Sync 技能** | Claude Code 内 `/design-sync` 命令同步本地组件库 | `finalize_plan` → `write_files` → `register_assets` |
| **增量同步** | 逐组件同步，不整包替换 | 单文件 `write_files` + `register_assets` |
| **Token/Style 管理** | CSS Custom Properties 为载体的设计 Token | HTML `<style>` 内 CSS 变量 |
| **多组件分组** | Buttons / Forms / Navigation 等逻辑分组 | `@dsCard group=` 注解 |
| **版本管理** | 项目级文件结构，支持 `list_files` / `get_file` | API 级文件 CRUD |
| **实时预览** | 浏览器内渲染 HTML 组件 | iframe sandbox |

### 1.3 技术架构

```
claude.ai Web App
├── Design System Pane（侧边栏）
│   ├── Card Index（_ds_manifest.json 自动生成）
│   ├── Preview iframe（渲染组件 HTML）
│   └── Group 导航（按 @dsCard group 分组）
├── Design-Sync API
│   ├── list_projects / get_project / create_project
│   ├── list_files / get_file
│   ├── finalize_plan → write_files / delete_files
│   └── register_assets / unregister_assets
└── Claude Code 集成
    ├── /design-login（授权）
    └── /design-sync（同步技能）
```

### 1.4 @dsCard 注解规范

```html
<!-- @dsCard group="Buttons" name="Primary Button" -->
<button class="btn btn-primary">Click me</button>
```

- `group`：分组标签（Buttons / Forms / Navigation / Colors / Spacing / Components / Brand）
- `name`：卡片显示名
- `_ds_manifest.json`：由 app 自检从首行注释编译，替代手动 `register_assets`

---

## 二、Claude Design 的致命短板

### 2.1 架构级缺陷

| 短板 | 具体表现 | 严重程度 |
|------|---------|---------|
| **强制云端** | 所有设计资产存储在 claude.ai 服务器，无本地模式 | 🔴 致命 |
| **无画布** | 没有可视化画布，只有代码预览卡片，无法拖拽/布局 | 🔴 致命 |
| **无矢量设计** | 纯 HTML/CSS 文本，不支持 SVG/矢量绘制 | 🟠 严重 |
| **无交互原型** | 组件是静态 HTML 预览，无页面跳转/动效/交互流程 | 🟠 严重 |
| **无 AI 生成** | Claude Design 本身不生成 UI，只是管理 Claude Code 生成的组件 | 🟠 严重 |
| **无设计系统约束执行** | Design System 是纯参考，不强制 AI 生成遵守 | 🟡 中等 |
| **无导出能力** | 没有内置导出 PNG/SVG/PDF，只能复制 HTML 代码 | 🟡 中等 |
| **无 Figma 集成** | 不支持导入 Figma 文件 | 🟡 中等 |
| **无多人协作** | 单人使用，无实时协作 | 🟢 低（单人场景可接受） |
| **Token 消耗大** | 每次同步/生成都消耗 Claude API token，成本高 | 🟠 严重 |

### 2.2 用户体验缺陷

1. **设计↔代码断裂**：设计师无法在 Claude Design 中直接操作 UI，只能写描述让 Claude Code 生成 HTML
2. **无即时反馈**：修改 Token 后需重新 `/design-sync` 才能看到效果
3. **无组件实例化**：组件只是代码片段，无法在画布上实例化、拖拽、调整属性
4. **无布局系统**：没有 Flex/Grid 布局，组件只是独立预览
5. **平台锁定**：深度绑定 claude.ai 生态，无法迁移

### 2.3 Claude Design 本质判断

**Claude Design 不是一个设计工具，而是一个「代码组件库管理面板」**。它解决的是"AI 生成的 UI 代码如何可视化管理和复用"的问题，但不解决"如何设计 UI"本身。

这为 Fusion-Design 提供了清晰的差异化定位空间。

---

## 三、Fusion-Design vs Claude Design 竞争力对比

### 3.1 能力对比矩阵

| 能力维度 | Claude Design | Fusion-Design 目标 | 差异化强度 |
|---------|---------------|-------------------|-----------|
| **本地离线** | ❌ 强制云端 | ✅ 100% 本地 | ⭐⭐⭐⭐⭐ |
| **可视化画布** | ❌ 无画布 | ✅ 矢量画布+拖拽 | ⭐⭐⭐⭐⭐ |
| **AI 生成 UI** | ⚠️ 依赖 Claude Code 写 HTML | ✅ 内置 AI 对话直接生成 | ⭐⭐⭐⭐⭐ |
| **设计系统强制执行** | ❌ 纯参考 | ✅ AI 生成自动遵循 Token | ⭐⭐⭐⭐ |
| **交互原型** | ❌ 静态预览 | ✅ 跳转/弹窗/表单交互 | ⭐⭐⭐⭐ |
| **代码导出** | ⚠️ 复制 HTML 片段 | ✅ React/HTML/Tailwind/SwiftUI | ⭐⭐⭐⭐ |
| **Token 管理** | ⚠️ CSS 变量手动写 | ✅ 可视化 Token 面板 | ⭐⭐⭐ |
| **组件库管理** | ✅ @dsCard 分组预览 | ✅ 设计系统+组件实例化 | ⭐⭐ |
| **生态联动** | ⚠️ 仅 Claude Code | ✅ Fusion 全系（Code/Sim/KB/Desk） | ⭐⭐⭐⭐⭐ |
| **成本** | 🔴 按 token 计费 | ✅ 零 API 费用 | ⭐⭐⭐⭐⭐ |
| **隐私** | 🔴 设计资产上云 | ✅ 全程本地 | ⭐⭐⭐⭐⭐ |
| **Flex/Grid 布局** | ❌ 无 | ✅ 原生 CSS 布局 | ⭐⭐⭐⭐ |
| **图生 UI** | ❌ 不支持 | ✅ 上传草图→界面 | ⭐⭐⭐⭐ |
| **批量导出** | ❌ 不支持 | ✅ PNG/SVG/PDF/HTML 批量 | ⭐⭐⭐ |
| **Figma 导入** | ❌ 不支持 | ✅ V0.2 op-figma | ⭐⭐⭐ |

### 3.2 核心差异化定位

```
Claude Design = 「AI 写的 HTML 组件库管理面板」
Fusion-Design = 「本地离线 AI 可视化设计工作台」

Claude Design 解决：代码组件怎么管
Fusion-Design 解决：UI 怎么设计 + 怎么生成 + 怎么导出 + 怎么联动
```

### 3.3 超越策略：五维打击

1. **离线维度**：Claude Design 无法离线 → Fusion-Design 100% 本地，零网络依赖
2. **画布维度**：Claude Design 无画布 → Fusion-Design 矢量画布 + 拖拽 + Flex/Grid
3. **AI 维度**：Claude Design 依赖外部 Claude Code → Fusion-Design 内置 AI 对话直接生成
4. **生态维度**：Claude Design 仅联动 Claude Code → Fusion-Design 联动全系 Fusion 工具
5. **成本维度**：Claude Design 按 token 计费 → Fusion-Design 本地推理零 API 费用

---

## 四、借鉴 Claude Design 的可取之处

### 4.1 Design-Sync 模式

Claude Design 的 `/design-sync` 增量同步模式值得借鉴：

- **增量同步**：逐组件同步，不整包替换 → Fusion-Design 的 `fd-ecosystem` 与 Fusion Code 双向同步应采用此模式
- **Plan 机制**：`finalize_plan` → `write_files` → `register_assets` 三步事务 → Fusion-Design 代码导出应有类似的"预览→确认→写入"流程
- **@dsCard 注解**：组件元数据注解 → Fusion-Design 的 `.fusiondesign` 文件可内置类似的组件元数据标记

### 4.2 Token→CSS Custom Properties

Claude Design 的 Token 体系基于 CSS Custom Properties：

```css
:root {
    --color-primary: #007AFF;
    --spacing-md: 16px;
    --radius-md: 8px;
}
```

Fusion-Design 的 `fd-design-system` 已有 `TokenValue` 枚举，但需补齐：
- Token→CSS Custom Properties 自动生成
- Token 引用链解析（`{color.primary}` → `#007AFF`）
- 主题切换无闪烁

### 4.3 组件分组预览

Claude Design 的分组预览模式（Buttons / Forms / Navigation）简洁有效：

- `@dsCard group=` 注解 → `_ds_manifest.json` 自动索引
- Fusion-Design 应在 `FusionDesignSystem.swift` 的 `ComponentCategory` 基础上增加预览卡片视图

---

## 五、Fusion-Design 超越 Claude Design 的技术路线

### 5.1 画布层：Claude Design 没有，Fusion-Design 必须做到极致

| 特性 | 实现方案 | 参考来源 |
|------|---------|---------|
| 矢量画布 | `fd-canvas-core` PenDocument/PenNode | OpenPencil |
| CSS Flex/Grid | Taffy crate（Rust 原生） | Penpot 算法+Taffy 实现 |
| 拖拽交互 | `fd-host-web` wasm 事件处理 | OpenPencil op-host-web |
| 吸附对齐 | 自研 snap 算法 | Penpot snap.cljs 思路 |
| 图层管理 | PenDocument 树结构 | OpenPencil |
| 组件实例化 | fd-canvas-core ComponentSlot | Penpot + Open Design |

### 5.2 AI 层：Claude Design 依赖外部，Fusion-Design 内置闭环

| 特性 | 实现方案 | 参考来源 |
|------|---------|---------|
| 文生 UI | `fd-ai-adapter` → fusion-mlx | OpenPencil op-ai-skills |
| 图生 UI | 多模态模型视觉理解 | Screenshot-to-code 思路 |
| 局部编辑 | 框选+自然语言修改 | OpenPencil |
| 多方案对比 | 一次生成 3 套变体 | Open Design Skill |
| Token 注入 | AI 提示词自动注入当前 Design Token | Open Design DESIGN.md |
| Skill 系统 | 可组合技能注册 | Open Design Skill |

### 5.3 设计系统层：Claude Design 纯参考，Fusion-Design 强制执行

| 特性 | 实现方案 | 参考来源 |
|------|---------|---------|
| Token 定义 | fd-design-system `TokenValue` 枚举 | Stitches 概念 |
| Token→CSS | 自动生成 CSS Custom Properties | Claude Design + Stitches |
| 3 套预设规范 | Apple HIG / 后台 / 机器人仿真 | fd-design-system 已实现 |
| AI 遵循 Token | 提示词注入 + 生成后校验 | Open Design DESIGN.md |
| 设计规范检测 | lint + 自动修复 | OpenPencil op-design-lint |
| 主题切换 | 运行时无闪烁切换 | Stitches |

### 5.4 代码导出层：Claude Design 复制粘贴，Fusion-Design 一键导出

| 特性 | 实现方案 | 参考来源 |
|------|---------|---------|
| React/HTML/Tailwind | fd-codegen | OpenPencil op-codegen |
| SwiftUI 导出 | 新增 SwiftUI target | Plasmic 多框架思路 |
| 反向同步 | fd-ecosystem 监听文件变更 | Claude Design design-sync 模式 |
| 批量导出 | fd-export PNG/SVG/PDF/HTML | Penpot exporter |
| 预览→确认→写入 | Plan 机制 | Claude Design finalize_plan |

### 5.5 生态层：Claude Design 单线，Fusion-Design 网状

| 联动目标 | 协议 | 对 Claude Design 优势 |
|---------|------|---------------------|
| Fusion Code | MCP + 本地文件 IPC | Claude Design 仅单向，Fusion 双向 |
| Fusion Simulation | MCP | Claude Design 无仿真联动 |
| Fusion KB | MCP | Claude Design 无本地知识库 |
| Fusion Model Hub | 本地接口 | Claude Design 无模型切换 |
| Fusion Desk | WKWebView 内嵌 | Claude Design 无桌面集成 |
| Fusion CLI | CLI 子命令 | Claude Design 无 CLI |

---

## 六、Fusion-Studio 前端 GUI 定位

### 6.1 当前状态

Fusion-Studio (`~/fusion/fusion-studio/`) 已实现：
- ✅ SwiftUI 原生 macOS App（4 栏布局）
- ✅ Design 模块：`DesignBridge.swift`（AI 对话）、`FusionDesignSystem.swift`（组件库）、`WebViewContainer.swift`（WKWebView）
- ✅ WKWebView 桥接：`fusionBridge` messageHandler
- ✅ AI 对话流式响应：直接调用 fusion-mlx `/v1/chat/completions`
- ✅ Token 面板：`DesignTokenPanel.swift`
- ✅ 代码导出：`ReactVueExporter.swift`、`SwiftUIExporter.swift`
- ✅ 截图导入：`ScreenshotImporter.swift`

### 6.2 Design 模块在 Fusion-Studio 中的定位

```
Fusion-Studio（SwiftUI 宿主）
├── IconRailView（模块切换栏）
├── FusionSidebarView（侧边栏）
├── WorkspaceArea（工作区）
│   └── Design Module
│       ├── WKWebView 画布（fd-host-web wasm）
│       ├── DesignChatPanel（AI 对话面板）
│       ├── DesignInspectorView（属性检查器）
│       ├── DesignTokenPanel（Token 管理面板）
│       ├── DesignPreviewView（代码预览）
│       └── DesignCodeLink（Fusion Code 联动）
└── InspectorPanel（检查器面板）
```

### 6.3 前端分工

| 层 | 技术 | 职责 | 项目 |
|----|------|------|------|
| 宿主壳 | SwiftUI | 窗口管理、导航、设置、AI 对话面板 | fusion-studio |
| 画布渲染 | Rust→wasm (fd-host-web) | 矢量画布、图层操作、Flex/Grid 渲染 | fusion-design |
| 桥接 | WKWebView messageHandlers | SwiftUI↔wasm 双向通信 | fusion-studio + fusion-design |
| 后端服务 | Rust crates | AI 推理、代码导出、文件管理、生态联动 | fusion-design |

---

## 七、核心结论

1. **Claude Design 是组件库管理面板，不是设计工具** — 这给了 Fusion-Design 完整的设计工具定位空间
2. **Claude Design 五大致命短板**：强制云端、无画布、无 AI 直接生成、无交互原型、高 token 成本
3. **Fusion-Design 五维超越**：离线、画布、AI 内置、生态网状、零成本
4. **借鉴三点**：Design-Sync 增量同步模式、@dsCard 组件注解、Token→CSS Custom Properties
5. **前端放 fusion-studio**：SwiftUI 宿主壳 + WKWebView 承载 fusion-design wasm 画布，分工清晰
