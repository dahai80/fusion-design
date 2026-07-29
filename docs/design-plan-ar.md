# Design Plan AR — Fusion-Design 落地实施方案

> 版本：V1.0 | 日期：2026-07-28
> 依据：[design-insight.md](design-insight.md) + [claude-design-insight.md](claude-design-insight.md)
> 目标：macOS 本地竞争力超越 Claude Design，以 fusion-mlx 为底座，fusion-xx 模块各司其职，前端 GUI 放 fusion-studio

<!--
  Importers/callers: Referenced by README.md and CLAUDE.md as the canonical implementation plan.
  Affected API: No API surface changes — this is a planning document.
  Data schemas: References PenDocument, DesignSystem, Token, SkillOutput, BridgeCommand/BridgeEvent
                 but does not define them (they exist in their respective Rust/Swift source files).
  User instruction (verbatim): "结合~/design目录的开源软件和claude design， 设计实施fusion-design，
  macos本地竞争力超越claude design，注意以~/claude-home/fusion-mlx为底座，~/fusion/fusion-xx模块各司其职，
  前端GUI放在fusion-studio项目，请输出可落地的详细方案,并给出详细落地计划 落地deisign-plan-ar.md"
-->

---

## 一、战略定位

### 1.1 一句话定义

**Fusion-Design = 本地离线 AI 可视化设计工作台**，超越 Claude Design 的「组件库管理面板」定位，提供画布→AI→代码→生态全链路闭环。

### 1.2 竞争力公式

```
Fusion-Design 竞争力 = 离线 ✕ 画布 ✕ AI内置 ✕ 生态联动 ✕ 零成本
Claude Design 竞争力 = 云端组件管理 ✕ Claude Code 单线联动
```

### 1.3 差异化铁律

- **100% 离线**：零云端、零 API、零 token 费用
- **可视化画布**：矢量拖拽 + Flex/Grid，不是静态代码预览
- **AI 内置闭环**：对话即生成，不依赖外部编码 Agent
- **生态网状联动**：Fusion 全系工具闭环，不是单线 Code 联动

---

## 二、系统架构

### 2.1 全局架构图

```
┌──────────────────────────────────────────────────────────────┐
│                   Fusion-Studio（SwiftUI 宿主）               │
│  ┌─────────┐ ┌──────────┐ ┌───────────┐ ┌───────────────┐  │
│  │Icon Rail │ │ Sidebar  │ │ Workspace │ │ Inspector     │  │
│  │(模块切换)│ │(文件/图层)│ │(画布/对话)│ │(属性/Token)   │  │
│  └─────────┘ └──────────┘ └─────┬─────┘ └───────────────┘  │
└─────────────────────────────────┼────────────────────────────┘
                                  │ WKWebView messageHandlers
                                  │ fusionBridge / fusionStudio
┌─────────────────────────────────┼────────────────────────────┐
│              Fusion-Design 后端（Rust Workspace）             │
│                                 │                             │
│  ┌──────────────────┐ ┌────────▼────────┐ ┌──────────────┐  │
│  │  fd-host-web     │ │  fd-ai-adapter  │ │fd-design-sys │  │
│  │  (wasm 画布)     │ │  (→fusion-mlx)  │ │ (Token/规范)  │  │
│  └──────────────────┘ └─────────────────┘ └──────────────┘  │
│  ┌──────────────────┐ ┌─────────────────┐ ┌──────────────┐  │
│  │  fd-canvas-core  │ │  fd-codegen     │ │  fd-export   │  │
│  │  (数据模型)      │ │  (代码导出)      │ │ (PNG/SVG/PDF)│  │
│  └──────────────────┘ └─────────────────┘ └──────────────┘  │
│  ┌──────────────────┐ ┌─────────────────┐ ┌──────────────┐  │
│  │  fd-ecosystem    │ │  fd-host-desk   │ │   fd-cli     │  │
│  │  (生态联动IPC)   │ │  (WKWebView桥)  │ │  (命令行)    │  │
│  └──────────────────┘ └─────────────────┘ └──────────────┘  │
│                                                              │
│  vendor/openpencil/op-ai ← 唯一复用的 OpenPencil crate       │
└─────────────────────────────────┬────────────────────────────┘
                                  │ HTTP 127.0.0.1:8080
┌─────────────────────────────────┼────────────────────────────┐
│              fusion-mlx 底座（本地多模态推理引擎）             │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌───────────────┐  │
│  │   LLM    │ │   VLM    │ │ ImageGen │ │   VideoGen    │  │
│  │ (文生UI) │ │ (图生UI) │ │ (素材)   │ │ (动效预览)   │  │
│  └──────────┘ └──────────┘ └──────────┘ └───────────────┘  │
│  Metal/ANE 加速 · 40+ 量化格式 · Speculative Decode          │
└──────────────────────────────────────────────────────────────┘
```

### 2.2 模块职责与项目归属

| 模块 | 项目 | 语言 | 职责 | 依赖 |
|------|------|------|------|------|
| 宿主壳 | fusion-studio | SwiftUI | 窗口/导航/设置/AI对话面板/Token面板 | WKWebView |
| 画布渲染 | fusion-design/fd-host-web | Rust→wasm | 矢量画布/DOM渲染/事件处理 | fd-canvas-core |
| 画布数据模型 | fusion-design/fd-canvas-core | Rust | PenDocument/PenNode/Page/NodeStyle | 无（leaf） |
| AI 适配 | fusion-design/fd-ai-adapter | Rust | fusion-mlx HTTP 调用 + Skill 注册 | op-ai |
| 设计系统 | fusion-design/fd-design-system | Rust | Token/3套规范/Token→CSS | 无（leaf） |
| 代码导出 | fusion-design/fd-codegen | Rust | HTML/React/Tailwind/SwiftUI 生成 | fd-canvas-core, fd-design-system |
| 文件导出 | fusion-design/fd-export | Rust | PNG/SVG/PDF/HTML 批量导出 | fd-canvas-core |
| 生态联动 | fusion-design/fd-ecosystem | Rust | Fusion Code/Sim/KB/CLI IPC | fd-canvas-core |
| WKWebView 桥 | fusion-design/fd-host-desk | Rust | HostMessage/BridgeConfig | 无 |
| 命令行 | fusion-design/fd-cli | Rust | generate/export/export-batch | 全部 fd-* |
| 推理底座 | fusion-mlx | Python/MLX | 多模态推理/Metal加速/量化 | Apple Silicon |
| 代码编辑 | fusion-code | Python | Vibe Coding/文件监听/反向同步 | fusion-mlx |
| 仿真 | fusion-simulation | Python | 机器人仿真/控制面板UI | fusion-mlx |
| 知识库 | fusion-kb | Python | 设计模板存储/语义检索 | fusion-mlx |
| 模型管理 | fusion-model-hub | Python | 模型下载/切换/量化 | fusion-mlx |
| 桌面自动化 | fusion-desk | Python+Swift | WKWebView 宿主/自动化模板 | fusion-mlx |

---

## 三、核心模块详细设计

### 3.1 画布引擎（fd-canvas-core + fd-host-web）

#### 3.1.1 数据模型扩展

当前 `fd-canvas-core` 已有 PenDocument/Page/PenNode/NodeStyle，需扩展：

```rust
// fd-canvas-core/src/lib.rs 新增

pub enum LayoutMode {
    Free,
    Flex(FlexParams),
    Grid(GridParams),
}

pub struct FlexParams {
    pub direction: FlexDirection,       // Row/RowReverse/Column/ColumnReverse
    pub align_items: AlignItems,        // Start/End/Center/Stretch
    pub justify_content: JustifyContent, // Start/Center/End/SpaceBetween/SpaceAround/SpaceEvenly
    pub wrap: FlexWrap,                 // NoWrap/Wrap
    pub gap: f64,
    pub padding: SideOffsets,
}

pub struct GridParams {
    pub columns: Vec<TrackSizing>,      // Fixed/Auto/Flex(fr)/Percent
    pub rows: Vec<TrackSizing>,
    pub gap: (f64, f64),                // (row-gap, col-gap)
    pub areas: Vec<GridArea>,
}

pub enum TrackSizing {
    Fixed(f64),
    Auto,
    Flex(f64),                          // fr 单位
    Percent(f64),
}

pub struct ComponentSlot {
    pub component_id: String,
    pub variant: String,
    pub overrides: HashMap<String, serde_json::Value>,
}

// NodeStyle 扩展
pub struct NodeStyle {
    // ... 已有字段
    pub layout: LayoutMode,
    pub component_slot: Option<ComponentSlot>,
    pub design_token_refs: HashMap<String, String>,  // 属性→Token 引用
}
```

#### 3.1.2 布局引擎选型：Taffy

```toml
# fd-canvas-core/Cargo.toml
[dependencies]
taffy = "0.7"  # Rust 原生 Flexbox + CSS Grid 布局引擎
```

**理由**：
- Penpot 布局算法在 Clojure 中，移植成本高
- Taffy 是 Bevy/Dioxus 等主流 Rust 框架的布局引擎，成熟可靠
- 直接支持 Flexbox + CSS Grid，与 CSS 规范一致
- 纯 Rust，wasm-clean，无 FFI 开销

#### 3.1.3 wasm 画布渲染（fd-host-web）

```
fd-host-web 渲染管线：

PenDocument (Rust)
    ↓ 序列化
JSON → wasm 侧反序列化
    ↓ Taffy 布局计算
Layout Tree → 绝对坐标
    ↓ 渲染
DOM/Canvas 渲染
    ├─ 基础形状：DOM 元素（div/svg）
    ├─ 文本：DOM 文本节点
    └─ 复杂效果：Canvas 2D API
```

#### 3.1.4 画布交互事件流

```
用户拖拽/点击/缩放
    ↓ JS 事件
wasm 事件处理
    ↓ 转换为 Rust 结构
Mutation（修改 PenDocument）
    ↓ 序列化 JSON
window.webkit.messageHandlers.fusionBridge.postMessage()
    ↓ SwiftUI 侧
DesignBridge 处理 → 更新 InspectorPanel / TokenPanel
```

### 3.2 AI 设计生成（fd-ai-adapter）

#### 3.2.1 Skill 注册系统

借鉴 Open Design 的 Skill 可组合模式：

```rust
// fd-ai-adapter/src/skill.rs

pub trait DesignSkill: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn build_prompt(&self, context: &SkillContext) -> String;
    fn parse_response(&self, raw: &str) -> Result<SkillOutput>;
}

pub struct SkillContext {
    pub user_input: String,
    pub current_document: Option<PenDocument>,
    pub active_tokens: DesignSystem,
    pub selection: Option<Vec<String>>,     // 选中的节点 ID
    pub reference_image: Option<Vec<u8>>,   // 参考图/草图
}

pub enum SkillOutput {
    NewDocument(PenDocument),
    MutateNodes { adds: Vec<PenNode>, updates: Vec<(String, PenNode)>, deletes: Vec<String> },
    MultiVariant(Vec<PenDocument>),         // 多方案对比
    TokenUpdate(DesignSystem),
}

pub struct SkillRegistry {
    skills: HashMap<String, Box<dyn DesignSkill>>,
}

impl SkillRegistry {
    pub fn register(&mut self, skill: Box<dyn DesignSkill>) { ... }
    pub fn execute(&self, name: &str, ctx: &SkillContext, client: &FusionMlxClient) -> Result<SkillOutput> { ... }
}
```

#### 3.2.2 内置 Skill 定义

| Skill | 触发 | 输入 | 输出 |
|-------|------|------|------|
| `text_to_ui` | 对话框输入 | 自然语言描述 | PenDocument |
| `image_to_ui` | 上传图片/截图 | 图片+描述 | PenDocument |
| `local_edit` | 框选+指令 | 选区+修改描述 | MutateNodes |
| `multi_variant` | 对话框输入+`--multi` | 自然语言描述 | MultiVariant(3) |
| `token_inject` | 自动（每次 AI 调用前） | 当前 DesignSystem | 提示词注入 |
| `design_lint` | 手动触发 | 当前 PenDocument | MutateNodes（修复建议） |
| `sim_panel` | 联动 Simulation | 仿真参数描述 | PenDocument（控制面板） |

#### 3.2.3 AI 请求链路

```
DesignChatPanel（SwiftUI）
    ↓ 用户输入
DesignBridge.sendDesignChat()
    ↓ HTTP POST
fd-ai-adapter::FusionMlxClient::chat()
    ↓ POST /v1/chat/completions
fusion-mlx（127.0.0.1:8080）
    ↓ 流式 SSE
DesignBridge 流式解析 artifact 标签
    ↓ 解析 HTML→PenDocument
WebShell.mount() 渲染到 WKWebView
    ↓ 同步
DesignInspectorView / DesignTokenPanel 更新
```

#### 3.2.4 Token 注入机制

借鉴 Open Design DESIGN.md 模式，每次 AI 调用自动注入当前设计系统 Token：

```
[System Prompt]
你是一个 UI 设计生成助手。当前设计系统 Token：

颜色：
- primary: #007AFF
- secondary: #5856D6
- background: #1C1C1E
- surface: #2C2C2E
- text: #FFFFFF
- text-secondary: #8E8E93

间距：
- xs: 4px, sm: 8px, md: 16px, lg: 24px, xl: 32px

圆角：
- sm: 4px, md: 8px, lg: 16px, full: 9999px

字体：
- display: SF Pro Display, 34px, Bold
- headline: SF Pro Display, 24px, Semibold
- body: SF Pro Text, 16px, Regular
- caption: SF Pro Text, 12px, Regular

你必须严格使用以上 Token 生成 UI，不要使用自定义色值/字号/间距。
```

### 3.3 设计系统（fd-design-system）

#### 3.3.1 Token→CSS Custom Properties 生成

```rust
// fd-design-system/src/css_gen.rs

impl DesignSystem {
    pub fn to_css_custom_properties(&self) -> String {
        let mut css = String::from(":root {\n");
        for (name, token) in &self.tokens {
            let value = match &token.value {
                TokenValue::Color(c) => c.clone(),
                TokenValue::Number(n) => format!("{}px", n),
                TokenValue::Shadow(s) => s.clone(),
                TokenValue::String(s) => s.clone(),
            };
            css.push_str(&format!("  --{}: {};\n", name.replace('.', "-"), value));
        }
        css.push_str("}\n");
        css
    }

    pub fn resolve_reference(&self, token_name: &str) -> String {
        let resolved = self.tokens.get(token_name)
            .map(|t| match &t.value {
                TokenValue::String(s) if s.starts_with('{') && s.ends_with('}') => {
                    let ref_name = &s[1..s.len()-1];
                    self.resolve_reference(ref_name)
                }
                other => other.to_css_value(),
            })
            .unwrap_or_default();
        resolved
    }
}
```

#### 3.3.2 三套预设规范扩展

| 规范 | 色系 | 典型场景 | 差异化组件 |
|------|------|---------|-----------|
| Apple HIG | 蓝+白+灰 | 通用 iOS/macOS 应用 | SF Symbol 图标、毛玻璃 |
| 极简后台 | 深灰+绿+白 | 数据仪表盘/管理后台 | 数据卡片、图表、表格 |
| 机器人仿真 | 深蓝+橙+黑 | 仿真控制面板 | 状态灯、仪表盘、3D 控件 |

#### 3.3.3 设计规范检测（lint）

移植 OpenPencil op-design-lint 的 13 个检测器逻辑，替换 jian_ops_schema 为 fd-canvas-core：

| 检测器 | 规则 | 修复方式 |
|--------|------|---------|
| 对比度检测 | 文本/背景对比度 < 4.5:1 | 自动调整文本色 |
| 未标记输入 | 输入框无 label/placeholder | 插入 placeholder |
| 文本效果 | 文本节点有描边/阴影 | 移除效果 |
| 异常旋转 | 旋转角非 90° 倍数 | 对齐到最近 90° |
| 空/过度效果 | 不可见容器/过度效果 | 清理 |
| Token 不一致 | 使用非 Token 色值/字号 | 替换为最近 Token |

### 3.4 代码导出（fd-codegen）

#### 3.4.1 导出目标扩展

```rust
// fd-codegen/src/lib.rs

pub enum CodegenTarget {
    Html,
    ReactTailwind,
    TailwindOnly,
    SwiftUI,           // V0.2 新增
    ReactNative,       // V0.2 新增
}
```

#### 3.4.2 Plan 机制（借鉴 Claude Design finalize_plan）

```
用户点击「导出代码」
    ↓
1. PREVIEW 阶段：生成代码预览，不写文件
    ↓ 用户确认
2. PLAN 阶段：确定写入路径列表（finalize_plan）
    ↓ 用户确认路径
3. WRITE 阶段：写入文件到 Fusion Code 工程目录
    ↓
4. NOTIFY 阶段：通知 Fusion Code 刷新
```

#### 3.4.3 SwiftUI 导出（差异化能力）

Claude Design 无法导出 SwiftUI。Fusion-Design 独有优势：

```rust
// fd-codegen/src/swiftui.rs

impl Codegen for SwiftUICodegen {
    fn target_label(&self) -> &'static str { "SwiftUI" }

    fn generate(&self, doc: &PenDocument) -> String {
        // PenDocument → SwiftUI View 代码
        // 矢量画布中 Flex/Grid → SwiftUI HStack/VStack/Grid
        // Token → SwiftUI Color/Font extension
        // 组件 → SwiftUI View struct
    }
}
```

### 3.5 生态联动（fd-ecosystem）

#### 3.5.1 双向同步机制（借鉴 Claude Design design-sync）

```
正向同步（Design → Code）：
  PenDocument → fd-codegen → React/HTML/Tailwind/SwiftUI 代码
  → fd-ecosystem 写入 Fusion Code 工程目录
  → 增量写入（仅变更的组件/页面）

反向同步（Code → Design）：
  Fusion Code 修改组件样式
  → fd-ecosystem 监听文件变更
  → 解析 AST 提取样式变更
  → 转换为 PenDocument MutateNodes
  → 刷新 WKWebView 画布
```

#### 3.5.2 生态 IPC 协议

| 目标 | 协议 | 接口 | 触发 |
|------|------|------|------|
| Fusion Code | 本地文件 IPC | `fd-ecosystem::sync_to_code()` | 用户点击「同步到 Code」 |
| Fusion Code（反向） | 文件监听 | `fd-ecosystem::watch_code_changes()` | 自动（后台线程） |
| Fusion Simulation | JSON 文件 IPC | `fd-ecosystem::generate_sim_panel()` | 用户点击「生成仿真面板」 |
| Fusion KB | JSON 文件 IPC | `fd-ecosystem::save_template()` | 用户点击「保存模板」 |
| Fusion Model Hub | HTTP (127.0.0.1) | `fd-ai-adapter::switch_model()` | 用户切换模型 |
| Fusion CLI | CLI 子命令 | `fd-cli generate/export` | 命令行 |
| Fusion Desk | WKWebView 内嵌 | `fd-host-desk` 桥接 | 自动 |

### 3.6 Fusion-Studio 前端 GUI

#### 3.6.1 Design 模块布局

```
┌──────────────────────────────────────────────────────────────┐
│  Fusion Studio Toolbar                                        │
├────┬──────────┬───────────────────────────────┬──────────────┤
│    │          │                               │              │
│ I  │ Sidebar  │      Workspace Area           │  Inspector   │
│ c  │          │                               │              │
│ o  │ ┌──────┐ │  ┌─────────────┬────────────┐ │  ┌────────┐  │
│ n  │ │Pages │ │  │             │  AI Chat    │ │  │Props   │  │
│    │ │      │ │  │  WKWebView  │  Panel      │ │  │        │  │
│ R  │ │Layers│ │  │  (画布)     │             │ │  │Token   │  │
│ a  │ │      │ │  │             │  [输入框]   │ │  │Panel   │  │
│ i  │ │Assets│ │  │             │             │ │  │        │  │
│ l  │ └──────┘ │  └─────────────┴────────────┘ │  └────────┘  │
│    │          │  ┌─────────────────────────────┤│              │
│    │          │  │  Code Preview / Export Bar  ││              │
│    │          │  └─────────────────────────────┤│              │
├────┴──────────┴───────────────────────────────┴──────────────┤
│  Status Bar: Model | Memory | Export Status                   │
└──────────────────────────────────────────────────────────────┘
```

#### 3.6.2 SwiftUI↔wasm 通信协议

```swift
// Fusion-Studio → wasm
struct BridgeCommand: Encodable {
    let type: String        // "mount" | "mutate" | "select" | "export" | "token_update"
    let payload: String     // JSON string
}

// wasm → Fusion-Studio
struct BridgeEvent: Decodable {
    let type: String        // "selection_changed" | "mutation" | "export_ready" | "layout_computed"
    let payload: String     // JSON string
}
```

#### 3.6.3 关键 SwiftUI 视图

| 视图 | 文件 | 职责 |
|------|------|------|
| DesignView | Design/DesignView.swift | Design 模块主视图，组合画布+对话+检查器 |
| DesignChatPanel | Design/DesignChatPanel.swift | AI 对话面板，流式响应 |
| DesignInspectorView | Design/DesignInspectorView.swift | 属性检查器，选中节点属性编辑 |
| DesignTokenPanel | Design/DesignTokenPanel.swift | Token 管理面板，可视化编辑+主题切换 |
| DesignPreviewView | Design/DesignPreviewView.swift | 代码预览，导出按钮 |
| DesignCodeLink | Design/DesignCodeLink.swift | Fusion Code 联动状态 |
| WebViewContainer | Design/WebViewContainer.swift | WKWebView 容器，桥接初始化 |

---

## 四、落地计划

### Phase 0：基础整合（2 周）

**目标**：fusion-design wasm 画布在 Fusion-Studio WKWebView 中可加载、可交互

| 任务 | 项目 | 交付物 | 工时 |
|------|------|--------|------|
| P0-1 fd-canvas-core 扩展 LayoutMode/FlexParams/GridParams/ComponentSlot | fusion-design | Rust struct + 测试 | 3d |
| P0-2 引入 Taffy 依赖，实现布局计算 | fusion-design | fd-canvas-core 布局引擎 | 3d |
| P0-3 fd-host-web wasm 渲染管线（PenDocument→DOM） | fusion-design | wasm 画布基础渲染 | 5d |
| P0-4 Fusion-Studio WebViewContainer 加载 fd-host-web wasm | fusion-studio | SwiftUI 宿主画布 | 2d |
| P0-5 桥接协议定义（BridgeCommand/BridgeEvent） | fusion-design + fusion-studio | JSON schema | 1d |

**验收标准**：
- `cargo build -p fd-host-web --target wasm32-unknown-unknown` 成功
- Fusion-Studio Design 模块中 WKWebView 加载 wasm 画布
- 画布可渲染矩形/圆形/文本基础节点
- SwiftUI→wasm 命令可达（`mount` / `mutate`）

### Phase 1：AI 对话生成（2 周）

**目标**：AI 对话直接生成 UI，画布实时渲染

| 任务 | 项目 | 交付物 | 工时 |
|------|------|--------|------|
| P1-1 Skill 系统 trait 定义 + SkillRegistry | fusion-design | Rust trait + 注册表 | 2d |
| P1-2 text_to_ui Skill 实现 | fusion-design | 文生 UI Skill | 3d |
| P1-3 Token 注入机制（DESIGN.md 模式） | fusion-design | 自动注入提示词 | 1d |
| P1-4 DesignChatPanel 流式响应对接 fusion-mlx | fusion-studio | SwiftUI 对话面板 | 3d |
| P1-5 AI 响应 HTML→PenDocument 解析器 | fusion-design | HTML 解析→画布数据 | 3d |
| P1-6 解析结果→wasm 画布渲染 | fusion-design + fusion-studio | 端到端渲染 | 2d |

**验收标准**：
- 在 DesignChatPanel 输入「生成一个登录页面」
- fusion-mlx 流式响应
- 画布实时渲染生成的 UI（矩形/文本/输入框/按钮布局）
- 生成的 UI 自动使用当前 Design System Token

### Phase 2：画布交互（2 周）

**目标**：画布可拖拽、选中、编辑，Inspector 实时联动

| 任务 | 项目 | 交付物 | 工时 |
|------|------|--------|------|
| P2-1 wasm 侧拖拽/选中/缩放事件处理 | fusion-design | JS→Rust 事件流 | 4d |
| P2-2 选中节点→SwiftUI InspectorPanel 联动 | fusion-studio | 属性检查器 | 3d |
| P2-3 Inspector 修改→wasm 画布 MutateNodes | fusion-design + fusion-studio | 双向属性编辑 | 3d |
| P2-4 吸附对齐算法（基础 4 方向 snap） | fusion-design | 简化版 snap | 2d |
| P2-5 图层面板（Layers 侧边栏） | fusion-studio | SwiftUI 图层列表 | 2d |
| P2-6 local_edit Skill（框选+自然语言修改） | fusion-design | 局部编辑 Skill | 2d |

**验收标准**：
- 画布节点可拖拽移动、调整大小
- 选中节点后 InspectorPanel 显示属性，修改即时反映到画布
- 框选节点 + 对话输入「改为蓝色」→ 节点颜色更新

### Phase 3：设计系统+代码导出（2 周）

**目标**：Token 管理、主题切换、代码导出闭环

| 任务 | 项目 | 交付物 | 工时 |
|------|------|--------|------|
| P3-1 Token→CSS Custom Properties 生成 | fusion-design | fd-design-system CSS 生成 | 2d |
| P3-2 DesignTokenPanel 可视化编辑+主题切换 | fusion-studio | SwiftUI Token 面板 | 3d |
| P3-3 fd-codegen HTML/React/Tailwind 导出 | fusion-design | 代码生成器 | 3d |
| P3-4 Plan 机制（预览→确认→写入） | fusion-design + fusion-studio | 导出流程 | 2d |
| P3-5 fd-export PNG/SVG/HTML 批量导出 | fusion-design | 文件导出 | 2d |
| P3-6 design_lint Skill（基础检测器） | fusion-design | 6 个检测器 | 2d |

**验收标准**：
- Token 面板可编辑颜色/间距/字号，修改即时反映画布
- 切换设计规范（Apple HIG ↔ 后台 ↔ 仿真），画布批量更新
- 点击「导出 React」→ 预览代码 → 确认 → 写入文件
- 点击「导出 PNG」→ 生成图片文件

### Phase 4：生态联动（2 周）

**目标**：Fusion Code 双向同步 + Simulation 面板生成 + KB 模板存储

| 任务 | 项目 | 交付物 | 工时 |
|------|------|--------|------|
| P4-1 fd-ecosystem Fusion Code 正向同步 | fusion-design | Design→Code 写入 | 3d |
| P4-2 fd-ecosystem Fusion Code 反向监听 | fusion-design | Code→Design 同步 | 3d |
| P4-3 sim_panel Skill（仿真控制面板生成） | fusion-design | 仿真联动 Skill | 2d |
| P4-4 KB 模板保存/检索 | fusion-design + fusion-kb | 设计模板存储 | 2d |
| P4-5 multi_variant Skill（多方案对比） | fusion-design | 3 套变体生成 | 2d |
| P4-6 fd-cli generate/export 集成 | fusion-design | CLI 子命令 | 1d |

**验收标准**：
- 画布设计→点击「同步到 Code」→ Fusion Code 工程目录出现 React 文件
- Fusion Code 修改组件颜色→Design 画布自动更新
- 对话输入「生成机器人仿真控制面板」→ 生成含状态灯/仪表盘的面板
- 保存设计模板到 KB → 语义检索可找回

### Phase 5：图生UI+高级能力（2 周）

**目标**：截图/草图→UI、组件实例化、设计规范检测增强

| 任务 | 项目 | 交付物 | 工时 |
|------|------|--------|------|
| P5-1 image_to_ui Skill（多模态图生 UI） | fusion-design | 截图/草图→PenDocument | 4d |
| P5-2 ComponentSlot 组件实例化 | fusion-design | 组件实例+覆写 | 3d |
| P5-3 design_lint 完整 13 检测器 | fusion-design | 规范检测+自动修复 | 2d |
| P5-4 SwiftUI 导出 target | fusion-design | SwiftUI 代码生成 | 3d |
| P5-5 Figma 文件导入（op-figma 评估） | fusion-design | 可行性验证 | 2d |

**验收标准**：
- 上传登录页截图→画布生成对应 UI 布局
- 组件实例化：从组件库拖拽按钮→画布创建实例→修改不影响主组件
- 设计规范检测：检测出非 Token 色值→一键替换
- 导出 SwiftUI 代码可在 Xcode 中编译运行

### Phase 6：性能优化+发布（2 周）

**目标**：性能达标、稳定性、打包发布

| 任务 | 项目 | 交付物 | 工时 |
|------|------|--------|------|
| P6-1 画布性能优化（100+ 节点流畅） | fusion-design | 渲染优化 | 3d |
| P6-2 AI 推理异步+进度提示 | fusion-studio | 加载动画 | 2d |
| P6-3 内存泄漏检测+长时间运行稳定性 | fusion-design + fusion-studio | 稳定性验证 | 2d |
| P6-4 完整回归测试 | fusion-design | 全量测试 | 2d |
| P6-5 打包：fusion-design crate→fusion-studio 集成 | 跨项目 | 一体化打包 | 3d |

**验收标准**：
- 画布 100+ 节点拖拽无卡顿
- 文生 UI < 2s（本地 MLX 8GB）
- 连续运行 2h 无内存泄漏
- Fusion-Studio 单 App 运行，Design 模块完整可用

---

## 五、风险与规避

| 风险 | 概率 | 影响 | 规避 |
|------|------|------|------|
| Taffy 布局与 CSS 规范不一致 | 低 | 中 | Taffy 被 Bevy/Dioxus 验证，覆盖率高 |
| wasm 画布性能不足 | 中 | 高 | Phase 6 性能专项优化，必要时参考 Penpot tile 渲染 |
| AI 生成 HTML 解析失败 | 中 | 中 | 限定 AI 输出格式，增加 fallback 解析策略 |
| fusion-mlx 推理延迟过高 | 中 | 高 | 启用 Speculative Decode，量化优化 |
| Fusion Code 反向同步 AST 解析复杂 | 中 | 中 | V0.1 仅同步样式变更，不同步结构变更 |
| fd-host-web wasm 体积过大 | 低 | 中 | Tree-Shaking 裁剪，按需编译 |
| jian_ops_schema 阻断 OpenPencil 复用 | 已规避 | — | 已走自研路线，fd-canvas-core 替代 |

---

## 六、里程碑总览

| Phase | 周期 | 核心交付 | 里程碑 |
|-------|------|---------|--------|
| **P0** | W1-W2 | wasm 画布在 Fusion-Studio 可加载 | 🟢 画布可用 |
| **P1** | W3-W4 | AI 对话生成 UI | 🟢 AI 生成可用 |
| **P2** | W5-W6 | 画布拖拽+Inspector 联动 | 🟢 交互可用 |
| **P3** | W7-W8 | Token 管理+代码导出 | 🟢 设计系统可用 |
| **P4** | W9-W10 | 生态联动闭环 | 🟢 生态可用 |
| **P5** | W11-W12 | 图生 UI+高级能力 | 🟢 高级可用 |
| **P6** | W13-W14 | 性能优化+发布 | 🟢 发布就绪 |

---

## 七、与 Claude Design 的竞争力达成时间线

| 时间点 | 能力 | 对 Claude Design 优势 |
|--------|------|---------------------|
| P0 完成 | 本地画布可加载 | ✅ Claude Design 无画布 |
| P1 完成 | AI 对话直接生成 UI | ✅ Claude Design 依赖外部 Claude Code |
| P2 完成 | 画布交互编辑 | ✅ Claude Design 无可视化编辑 |
| P3 完成 | Token 管理+代码导出 | ✅ Claude Design 仅有 CSS 变量 |
| P4 完成 | 生态联动 | ✅ Claude Design 仅 Claude Code 单线 |
| P5 完成 | 图生 UI+组件实例化 | ✅ Claude Design 不支持 |
| P6 完成 | 性能达标+发布 | ✅ Claude Design 无法离线使用 |

**P3 完成时（W8），Fusion-Design 已在 5/7 维度超越 Claude Design。**
**P6 完成时（W14），Fusion-Design 在所有维度超越 Claude Design。**

---

## 八、技术选型总结

| 领域 | 选型 | 理由 |
|------|------|------|
| 画布数据模型 | fd-canvas-core（自研） | jian_ops_schema 阻断，自研更可控 |
| 布局引擎 | Taffy 0.7 | Rust 原生 Flexbox+Grid，Bevy/Dioxus 验证 |
| 画布渲染 | fd-host-web（wasm） | WKWebView 兼容，Rust 编译 wasm |
| AI 推理 | fusion-mlx（本地） | 离线铁律，Metal 加速 |
| AI 抽象层 | op-ai trait + fd-ai-adapter | 复用 OpenPencil wasm-clean trait |
| Skill 系统 | 自研 DesignSkill trait | 借鉴 Open Design，Rust 实现 |
| 设计系统 | fd-design-system（自研） | TokenValue 枚举 + CSS 生成 |
| 代码导出 | fd-codegen（自研） | HTML/React/Tailwind/SwiftUI |
| 生态联动 | fd-ecosystem（本地文件 IPC） | 离线约束，不引入网络 |
| 前端 GUI | Fusion-Studio（SwiftUI） | macOS 原生，WKWebView 内嵌 |
| 桥接 | WKWebView messageHandlers | 原生通信，无额外依赖 |
| 文件格式 | .fusiondesign（JSON） | Git-friendly，人可读 |
