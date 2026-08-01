# Fusion-Design V1.0 重构落地计划

> 基线：V0.2 (d9b7b52) → 目标：V1.0 PRD 全面落地
> 范围：仅修改 fusion-design 工程代码；上下游变更走 issue→PR 流程

---

## 现状差距分析

### V0.2 已实现 ✅

| 模块 | 能力 | 状态 |
|------|------|------|
| fd-canvas-core | PenDocument/PenNode/Page/NodeStyle + Flex/Grid布局 + Taffy计算 + ComponentSlot + Token引用 | ✅ 完整 |
| fd-ai-adapter | FusionMlxClient + ChatProvider trait + 6个Skill (text-to-ui/image-to-ui/partial-edit/local-edit/sim-panel/multi-variants) | ✅ 完整 |
| fd-ai-adapter | DesignSkills 便捷API + html_to_pen_document HTML解析器 | ✅ 完整 |
| fd-codegen | 4目标代码生成 (Html/ReactTailwind/TailwindOnly/SwiftUi) + DesignSystem Token注入 | ✅ 完整 |
| fd-export | HTML/SVG/JSON 导出 + PenDocument直接导出 + NodeStyle全字段 | ✅ 完整 |
| fd-design-system | 3套内置规范 + Token管理 + CSS Custom Properties输出 + 引用解析 | ✅ 完整 |
| fd-design-lint | 13条检查规则 + Linter API + 设计系统集成 | ✅ 完整 |
| fd-ecosystem | EcosystemLink IPC + LinkMessage + 模板保存/搜索 + MutateNodeCommand | ✅ 完整 |
| fd-host-desk | HostMessage/HostBridgeConfig + 离线校验 + 外网拦截 | ✅ 完整 |
| fd-host-web | WebShell.mount() + DOM/Canvas双渲染 + 消息桥 + 事件委托 + 视口剔除 + Plan预览 + 缩放/平移/框选 | ✅ 完整 |
| fd-cli | 13个子命令 (list-design-systems/activate/export/export-batch/generate/check-frontend/check-mlx/parse-html/token-css/lint/codegen) | ✅ 完整 |

### V1.0 PRD 需求差距 ❌

| PRD 需求 | 缺失点 | 优先级 |
|----------|--------|--------|
| **流式输出 (Streaming)** | fd-ai-adapter 只有同步/异步全量返回，无 SSE 流式 token 推送 | P1 |
| **版本对比 API** | fd-canvas-core 无 diff/patch 数据结构，无法计算两个 PenDocument 的差异 | P2 |
| **撤销/重做 API** | fd-canvas-core 无 undo/redo 栈，无法支持画布操作回退 | P0 |
| **节点锁定** | NodeStyle 缺少 `locked: bool` 字段，图层管理无法锁定元素 | P0 |
| **节点显隐** | NodeStyle 缺少 `visible: bool` 字段，图层管理无法隐藏元素 | P0 |
| **健康检查** | fd-ai-adapter 无 MLX 服务健康探测 API（GUI 状态灯需要） | P1 |
| **Token 应用到文档** | fd-design-system 有 to_css_custom_properties 但无 apply_tokens_to_document API | P3 |
| **Lint 一键修复** | fd-design-lint 只检测不修复，缺少 auto_fix 能力 | P3 |
| **模板搜索 API** | fd-ecosystem 有 save_template/search_templates 但搜索能力弱（无标签索引） | P4 |
| **IPC 监听** | fd-ecosystem 只有主动 list/consume，无 watch/subscribe 异步监听 | P4 |
| **PNG/PDF 导出** | fd-export 标记 NotImplemented | V2.0 |
| **批量导出增强** | fd-export batch 支持但 CLI 的 export-batch 缺少多格式组合 | P4 |

---

## 重构任务清单（按 PRD P0→P1→P2 顺序）

### Phase 1: P0 画布可用 — 撤销/重做 + 节点锁定/显隐

#### Task 1.1: fd-canvas-core — NodeStyle 增加 locked/visible 字段

- `NodeStyle` 新增 `locked: bool`（默认 false）和 `visible: bool`（默认 true）
- serde 用 `#[serde(default)]` 保持向后兼容
- 新增测试：locked/visible serde roundtrip + 向后兼容旧 JSON
- 影响：fd-host-web 的 `mutate_node`/`set_node_visibility` 已有 visible 支持，需同步 locked

#### Task 1.2: fd-canvas-core — UndoRedoStack 撤销/重做栈

- 新增 `UndoRedoStack` 结构体，保存 `PenDocument` 快照
- `UndoRedoStack::push(snapshot)`, `undo()`, `redo()`, `can_undo()`, `can_redo()`
- 最大栈深度 50，超出时丢弃最旧快照
- 新增 `PenDocument::snapshot()` 便捷方法（clone 自身）
- 测试：push→undo→redo 往返 + 栈深度限制 + 空 undo 安全

#### Task 1.3: fd-host-web — 支持节点锁定/显隐的 Bridge 命令

- 新增 `BridgeCommand::SetNodeLocked { node_id, locked }`
- 已有 `SetNodeVisibility`，确认 DOM 渲染正确处理 locked（locked 节点禁止拖拽）
- 锁定节点视觉反馈：虚线边框 + 锁图标叠加

#### Task 1.4: fd-cli — 新增 undo/redo 子命令

- `fusion-design undo --input <doc.json>` 返回上一步文档
- `fusion-design redo --input <doc.json>` 返回下一步文档
- 内部使用 UndoRedoStack 实现

### Phase 2: P1 AI 生成 — 流式输出 + 健康检查

#### Task 2.1: fd-ai-adapter — SSE 流式推理 API

- 新增 `FusionMlxClient::chat_stream()` 方法
- 返回 `impl Stream<Item = Result<String>>` (使用 `tokio_stream` 或 `futures::Stream`)
- 解析 fusion-mlx `/v1/chat/completions` 的 SSE `data: {...}` 流
- 新增 `MlxStreamDelta` / `MlxStreamDone` 结构体
- SkillRegistry 新增 `execute_stream()` 异步流式执行
- 测试：mock SSE 响应解析 + 流式 token 拼接验证

#### Task 2.2: fd-ai-adapter — MLX 健康检查 API

- 新增 `FusionMlxClient::health_check()` → `Result<HealthStatus>`
- `HealthStatus { available: bool, model: Option<String>, gpu: Option<String> }`
- 请求 `GET /v1/models` 或 `GET /health`
- 超时 3s，用于 GUI 状态灯轮询
- 测试：mock 成功/失败/超时

#### Task 2.3: fd-ai-adapter — 截图 base64 多模态请求

- 当前 `image_to_ui` 只传路径文字，不传图片内容
- 新增 `FusionMlxClient::chat_with_image()` 支持 OpenAI vision 格式
- 请求体 `content` 从纯文本改为 `[{type:"text",...},{type:"image_url",image_url:{url:"data:image/png;base64,..."}}]`
- `DesignSkills::screenshot_to_ui()` 读取图片→base64→多模态推理
- 测试：base64 编码 + 请求体结构验证

#### Task 2.4: fd-cli — 新增 health 子命令

- `fusion-design health` 输出 JSON：`{"available":true,"model":"...","gpu":"..."}`
- 供 Swift GUI 调用 `Process()` 获取 MLX 状态

#### Task 2.5: fd-cli — generate 子命令支持流式输出

- `fusion-design generate --prompt "..." --stream` 输出 SSE 到 stdout
- 供 Swift GUI 通过 `Pipe()` 读取流式 token

### Phase 3: P2 交互增强 — 版本对比 + 主题管理 API

#### Task 3.1: fd-canvas-core — PenDocument diff/patch

- 新增 `PenDocumentDiff` 结构体：节点级增删改列表
- `PenDocument::diff(&self, other: &PenDocument) -> PenDocumentDiff`
- `PenDocument::apply_patch(&mut self, patch: &PenDocumentDiff)`
- `DiffEntry { node_id, change_type: Added/Removed/Modified, field, old_value, new_value }`
- 测试：同文档 diff 空 + 单节点修改 diff + 复杂 diff + patch 往返

#### Task 3.2: fd-design-system — 主题（明/暗）Token 双套支持

- `DesignSystem` 新增 `dark_tokens: Option<Vec<Token>>` 字段
- `DesignSystem::to_css_custom_properties_for_theme(theme: Theme)` — Light/Dark 切换
- `DesignSystemRegistry` 增加 `activate_theme()` 方法
- Token 的 `ThemedEntry` 已存在于 fd-canvas-core，对接 fd-design-system 的主题系统
- 测试：dark tokens serde + 双主题 CSS 输出

#### Task 3.3: fd-cli — 新增 diff 子命令

- `fusion-design diff --old v1.json --new v2.json` 输出差异 JSON
- 供 Swift `ArtifactVersionDiff` 调用

#### Task 3.4: fd-cli — 新增 theme 子命令

- `fusion-design theme --system <id> --mode light|dark` 输出对应主题的 CSS
- 供 Swift `ThemeSwitcher` 调用

### Phase 4: P3 设计系统增强 — Token 应用 + Lint 修复

#### Task 4.1: fd-design-system — apply_tokens_to_document

- 新增 `apply_tokens_to_document(doc: &mut PenDocument, system: &DesignSystem)`
- 遍历 doc 中所有 NodeStyle，将硬编码色值/字号/间距匹配到 Token 并替换为 `design_token_refs`
- 测试：简单文档应用 + 未匹配值保留 + 已有引用跳过

#### Task 4.2: fd-design-lint — auto_fix 能力

- `Linter` 新增 `auto_fix(doc: &mut PenDocument, violations: &[LintViolation]) -> Vec<FixResult>`
- 每条 LintRule 实现 `fn fix(node: &mut PenNode, violation: &LintViolation) -> FixResult`
- `FixResult { rule, node_id, applied: bool, description }`
- 可自动修复的规则：ContrastCheck（调色）、HardcodedSpacing（替换Token）、UnnamedNode（命名）
- 不可自动修复的规则返回 `applied: false` + 建议描述
- 测试：对比度修复 + 间距修复 + 不可修复规则

#### Task 4.3: fd-cli — lint 子命令增加 --fix

- `fusion-design lint --input doc.json --fix` 自动修复并输出修复后的文档
- `--fix --dry-run` 仅输出修复建议不实际修改

### Phase 5: P4 生态增强 — IPC 监听 + 模板搜索增强

#### Task 5.1: fd-ecosystem — 异步文件监听

- 新增 `EcosystemLink::watch(target, callback)` 方法
- 使用 `notify` crate 监听 IPC 目录文件变更
- `watch()` 返回 `JoinHandle`，可取消
- 测试：创建文件触发回调 + 取消监听

#### Task 5.2: fd-ecosystem — 模板标签索引

- `DesignTemplate` 新增 `tags: Vec<String>` 字段
- 搜索支持按标签 + 名称模糊匹配
- 新增 `search_templates_advanced()` 方法
- 测试：标签搜索 + 名称搜索 + 组合搜索

---

## 上游/下游依赖变更（需提 Issue + PR）

### 上游：fusion-mlx

**Issue 1**: 请求暴露 `/v1/models` 或 `/health` 端点
- 目的：GUI 状态灯需要健康检查
- 当前状态：fusion-mlx 可能已有此端点，需确认
- 如果已有，fd-ai-adapter 直接对接；如果没有，需提 issue

**Issue 2**: 请求支持 SSE 流式输出 (`stream: true`)
- 目的：流式 token 推送，提升 UX
- `/v1/chat/completions` 请求体新增 `stream: true`
- 返回 `data: {"choices":[{"delta":{"content":"..."}}]}` SSE 格式

**Issue 3**: 请求支持多模态 vision 请求
- 目的：截图/草图 → UI 解析
- 请求体 content 支持图片 URL（base64 data URL）
- 依赖 fusion-mlx 加载多模态模型

### 下游：fusion-studio

**Issue 1**: 请求 DesignBridge 对接新版 fd-ai-adapter 流式 API
- 流式 token 通过 CLI `--stream` + Pipe 读取
- 健康检查通过 `fusion-design health` 调用

**Issue 2**: 请求 DesignBridge 对接 fd-canvas-core 新增字段
- NodeStyle.locked / visible → 图层面板锁定/显隐
- UndoRedoStack → 撤销/重做按钮

**Issue 3**: 请求 ArtifactVersionDiff 对接 diff API
- `fusion-design diff` → 版本对比可视化

---

## 执行顺序

```
Week 1-2: Phase 1 (P0 画布可用)
  1.1 NodeStyle locked/visible
  1.2 UndoRedoStack
  1.3 fd-host-web Bridge 更新
  1.4 fd-cli undo/redo
  → 提交上游 issue (fusion-mlx SSE + health + vision)

Week 3-4: Phase 2 (P1 AI 生成)
  2.1 流式推理 API
  2.2 健康检查
  2.3 截图多模态
  2.4 fd-cli health
  2.5 fd-cli generate --stream

Week 5-6: Phase 3 (P2 交互增强)
  3.1 PenDocument diff/patch
  3.2 主题双套 Token
  3.3 fd-cli diff
  3.4 fd-cli theme

Week 7-8: Phase 4 (P3 设计系统)
  4.1 apply_tokens_to_document
  4.2 lint auto_fix
  4.3 fd-cli lint --fix

Week 9-10: Phase 5 (P4 生态)
  5.1 IPC 文件监听
  5.2 模板搜索增强
  → 提交下游 issue (fusion-studio 对接新 API)
```

## 验收标准

- [ ] `cargo test --workspace` 全部通过
- [ ] `cargo check --workspace` 无错误
- [ ] 所有新增 API 有 tracing 日志
- [ ] 所有新增 API 有测试
- [ ] fd-host-web WASM 可编译 (`--target wasm32-unknown-unknown`)
- [ ] 向后兼容：旧 .fusiondesign 文件可加载
- [ ] README.md 更新
