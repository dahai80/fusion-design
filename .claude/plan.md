# P0 实施计划：基础整合

<!--
  Importers/callers: This plan guides the implementation of P0 from docs/design-plan-ar.md.
  Affected API: fd-canvas-core public API (LayoutMode/FlexParams/GridParams/ComponentSlot added to NodeStyle/PenNode),
                fd-host-web public API (mount enhanced, BridgeCommand/BridgeEvent messages),
                fd-design-system public API (to_css_custom_properties/resolve_reference added),
                fd-ai-adapter public API (DesignSkill trait/SkillContext/SkillOutput/SkillRegistry added, DesignSkills deprecated).
  Data schemas: LayoutMode, FlexParams, GridParams, TrackSizing, ComponentSlot, SkillContext, SkillOutput, BridgeCommand, BridgeEvent.
  User instruction verbatim: "现在开始实施，你只能改fusion-design和fusion-studio，其他的有需求给他们提issue和pr"
-->

## 目标
让 fusion-design wasm 画布在 Fusion-Studio WKWebView 中可加载、可交互。

## 当前状态分析

### fusion-design（Rust workspace）
- **fd-canvas-core**：PenDocument/PenNode/Page/NodeStyle 已完整，缺 LayoutMode/Flex/Grid/ComponentSlot
- **fd-host-web**：已有 WebShell::mount() + Canvas 2D 渲染 + 消息桥接，缺 DOM 渲染和交互事件
- **fd-ai-adapter**：FusionMlxClient + DesignSkills 已完整（text_to_ui/image_to_ui/partial_edit/multi_variants）
- **fd-design-system**：3 套内置规范 + Token 管理 + Registry 已完整，缺 CSS Custom Properties 生成
- **fd-codegen**：HTML/React+Tailwind/TailwindOnly 已完整，缺 SwiftUI target
- **fd-export**：HTML/SVG/JSON 导出已完整
- **fd-ecosystem**：本地文件 IPC 已完整
- **fd-host-desk**：HostMessage/HostBridgeConfig 已完整
- **fd-cli**：6 个子命令已完整

### fusion-studio（SwiftUI）
- **WebViewContainer**：WKWebView + fusionBridge messageHandler + fusionStudio JS API 已完整
- **DesignBridge**：AI 对话流式响应 + artifact 解析 + 多页管理 已完整
- **DesignChatPanel**：SwiftUI 对话面板已有
- **DesignInspectorView**：属性检查器已有
- **DesignTokenPanel**：Token 面板已有

## 实施任务

### Task 1: fd-canvas-core 扩展布局模型
**改动**：`crates/fd-canvas-core/src/lib.rs` + `crates/fd-canvas-core/Cargo.toml`

新增：
- `LayoutMode` enum（Free/Flex/Grid）
- `FlexParams` struct（direction/align_items/justify_content/wrap/gap/padding）
- `GridParams` struct（columns/rows/gap/areas）
- `TrackSizing` enum（Fixed/Auto/Flex/Percent）
- `ComponentSlot` struct（component_id/variant/overrides）
- `NodeStyle` 新增字段：`layout`/`component_slot`/`design_token_refs`
- `PenNode` 新增字段：`rotation`/`z_index`
- 引入 taffy 0.7 依赖
- 新增 `PenDocument::compute_layout()` 方法（Taffy 布局计算）
- 测试覆盖新类型 serde roundtrip + 布局计算

### Task 2: fd-host-web DOM 渲染 + 交互事件
**改动**：`crates/fd-host-web/src/lib.rs`

新增：
- `mount()` 改为同时支持 canvas 和 DOM 模式（data-render-mode="dom"）
- DOM 渲染管线：PenDocument → Taffy 布局 → DOM 元素（div/svg）
- 交互事件：mousedown/mousemove/mouseup → 转换为 Selection/Mutation 事件
- `send_to_host` 改为通过 `window.webkit.messageHandlers.fusionBridge.postMessage()` 发送
- 新增 BridgeCommand 处理：`select`/`mutate`/`token_update`
- 新增 BridgeEvent 发送：`selection_changed`/`mutation`/`layout_computed`
- 测试覆盖消息格式 + DOM 结构

### Task 3: fd-design-system CSS Custom Properties 生成
**改动**：`crates/fd-design-system/src/lib.rs`

新增：
- `DesignSystem::to_css_custom_properties()` 方法
- `DesignSystem::resolve_reference()` 方法（解析 `{token.name}` 引用链）
- `TokenValue::to_css_value()` 方法
- 测试覆盖 CSS 输出格式

### Task 4: fd-ai-adapter Skill 系统 trait 化
**改动**：`crates/fd-ai-adapter/src/lib.rs` + 新增 `crates/fd-ai-adapter/src/skill.rs`

重构：
- 提取 `DesignSkill` trait（name/description/build_prompt/parse_response）
- `SkillContext` struct（user_input/current_document/active_tokens/selection/reference_image）
- `SkillOutput` enum（NewDocument/MutateNodes/MultiVariant/TokenUpdate）
- `SkillRegistry` struct（register/execute）
- 将现有 DesignSkills 方法重构为独立 Skill 实现（TextToUiSkill/ImageToUiSkill/LocalEditSkill/MultiVariantSkill）
- Token 注入机制：`token_inject` Skill（自动在每次 AI 调用前注入当前 DesignSystem Token）
- 测试覆盖 trait 注册 + 执行 + Token 注入

### Task 5: Fusion-Studio WebViewContainer 加载 wasm 画布
**改动**：fusion-studio 中 `WebViewContainer.swift` + 新增 `DesignCanvasView.swift`

新增：
- `DesignCanvasView`：组合 WebViewContainer + 工具栏
- WebViewContainer 支持加载 wasm 打包的 HTML（file:// 协议）
- 桥接协议：BridgeCommand/BridgeEvent JSON schema
- 选中节点 → DesignInspectorView 联动
- 消息路由：wasm → SwiftUI 事件分发
- wasm 构建脚本：`wasm-pack build --target web --out-dir pkg`

### Task 6: 集成验证
- `cargo build -p fd-host-web --target wasm32-unknown-unknown` 成功
- `cargo test --workspace` 全部通过
- wasm 画布 HTML 可在浏览器中加载
- 桥接消息格式端到端验证

## 依赖关系

```
Task 1 (布局模型) ← Task 2 (DOM 渲染) ← Task 5 (SwiftUI 加载)
                                    ← Task 3 (CSS 生成)
Task 4 (Skill trait) 独立
Task 6 (集成验证) 依赖全部完成
```

## 执行顺序

1. Task 1：fd-canvas-core 布局扩展（先做数据模型，一切依赖它）
2. Task 3：fd-design-system CSS 生成（独立，可与 Task 1 并行）
3. Task 4：fd-ai-adapter Skill trait 化（独立，可与 Task 1/3 并行）
4. Task 2：fd-host-web DOM 渲染（依赖 Task 1 的布局模型）
5. Task 5：Fusion-Studio 加载 wasm（依赖 Task 2 的渲染管线）
6. Task 6：集成验证（依赖全部）

## 约束
- 只改 fusion-design 和 fusion-studio 两个项目
- 其他项目有需求提 issue/PR
- 4 的倍数缩进
- 不生成 docstring
- 代码必须带日志
- 现有测试不能 break
