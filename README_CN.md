# Fusion-Design

> macOS 本地离线 AI 可视化设计工作台 — 基于 OpenPencil (Rust) 底座 + fusion-mlx 本地多模态推理，原生嵌入 Fusion-Desk WKWebView。

**状态**：V0.2 — 11 个 crate + vendored op-ai，321 个测试通过，WASM 构建验证通过。

## 📋 概览

Fusion-Design 是 Fusion-MLX「一核九端」产品矩阵的旗舰主力之一，定位为**纯本地离线、AI 对话式 UI/原型设计工作台**。

| 维度 | 值 |
|------|-----|
| 对标产品 | Claude Design（云端闭源）、Figma、Penpot |
| 推理引擎 | fusion-mlx（本地多模态大模型推理，禁止云端模型） |
| 运行平台 | macOS Apple Silicon（M 系列，Metal/ANE 硬件加速） |
| 开发底座 | OpenPencil (Rust) 二次封装 + 自研 fusion-mlx 适配层 |
| 核心定位 | 与 Fusion Code 双向打通，设计一键生成本地可运行代码 |

### 核心差异化（对比 Claude Design）

- ✅ **100% 本地离线** — 所有 AI 生成、设计文件、素材存储本机，零云端上传
- ✅ **原生 Apple MLX** — 本地多模态视觉模型驱动，无 API 付费、无网络依赖
- ✅ **深度打通 Fusion 生态** — 联动仿真、自动化、知识库、代码工具
- ✅ **桌面原生应用** — 内嵌 Fusion-Desk WKWebView，无需独立网页
- ✅ **开源底座可私有化** — 无闭源厂商锁定

## 🚀 V0.2 功能矩阵

1. **无限矢量画布** — 缩放/平移/网格/多画板/图层管理，CSS Flex/Grid 布局，Taffy 布局引擎
2. **对话式 AI 设计生成** — 文生 UI、图生 UI、SSE 流式输出、多模态视觉输入
3. **本地设计系统** — 三套内置规范（Apple HIG / 后台管理 / 机器人仿真），Light/Dark 主题，Token 全局同步
4. **设计规范检测 + 自动修复** — 13 条 Lint 规则 + auto_fix Token 引用 / 空值清理 / 自动命名
5. **Fusion Code 双向打通** — 画布一键导出 React/HTML/Tailwind，反向同步样式
6. **原型交互与交付** — PNG/SVG/PDF/HTML 导出，批量导出
7. **生态联动** — Simulation / Desk / KB / CLI 全系打通，异步文件监听，模板标签检索
8. **撤销/重做 + Diff** — 快照栈（50 层）+ 节点级 Diff/Patch
9. **CLI 全功能** — generate / export / export-batch / lint --fix / undo / redo / health / diff / theme
10. **素材库管理** — 素材分类/标签/标注/颜色提取/设计系统 Token 绑定，19 项测试
11. **设计规范文档生成** — AI 自动生成交互规范/组件规范/页面架构文档（SpecDocSkill）
12. **页面流程批量生成** — AI 按流程描述批量生成多页面，统一风格（PageFlowSkill）
13. **命名版本管理** — 版本快照/切换/重命名/删除/相邻 diff 对比，11 项测试
14. **内置场景模板** — 4 类预设（移动端/B 端后台/营销网站/小程序），一键安装

## 🗂️ 项目结构（V0.2 — 11 个 crate + vendored op-ai，321 个测试）

```
fusion-design/
├── PRD.md
├── README.md
├── README_CN.md
├── docs/
│   ├── OPENS_SOURCE_REFERENCES.md
│   └── INTEGRATION_PLAN.md
├── crates/                     ← Fusion-Design 自研 Rust crate（workspace）
│   ├── fd-canvas-core/         ← 自研画布数据模型（PenDocument/PenNode + UndoRedo/Diff/Taffy + VersionedDocument）
│   ├── fd-ai-adapter/          ← op-ai → fusion-mlx 适配层（SSE 流式/多模态视觉/7 个 Skill）
│   ├── fd-codegen/             ← HTML/React+Tailwind 代码导出
│   ├── fd-design-system/       ← 两套内置设计规范 + Token + Light/Dark 主题
│   ├── fd-design-lint/         ← 13 条 Lint 规则 + auto_fix（Token 引用/空值清理/自动命名）
│   ├── fd-ecosystem/           ← 生态联动（IPC + 异步文件监听 + 模板标签检索 + 4 类内置场景模板）
│   ├── fd-asset/               ← 素材库管理（分类/标签/标注/颜色提取/Token 绑定）
│   ├── fd-host-desk/           ← Fusion-Desk WKWebView 宿主桥
│   ├── fd-host-web/            ← WASM 前端渲染（WebShell + BridgeCommand）
│   ├── fd-export/              ← PNG/SVG/PDF/HTML 批量导出
│   └── fd-cli/                 ← 命令行（generate/export/lint/undo/redo/health/diff/theme）
├── vendor/
│   └── openpencil/             ← 裁剪后的 OpenPencil 子集（Git subtree）
└── frontend/
    └── dist/                   ← 编译后静态文件（供 WKWebView 加载）
```

## 🛠️ 技术栈

- **底座**：OpenPencil (Rust workspace) — `op-ai` (vendored)
- **AI 推理**：fusion-mlx（本地多模态，Metal/ANE 加速）
- **宿主**：Fusion-Desk WKWebView（macOS 原生）
- **前端**：WASM (wasm32-unknown-unknown) + DOM/Canvas 渲染
- **通信**：本地私有 HTTP (127.0.0.1) + WKWebView Bridge
- **异步**：tokio + notify（文件监听）+ futures（SSE 流）

## 🧪 验证

```bash
cargo check --workspace                                        # ✅ 全套编译通过
cargo test --workspace                                         # ✅ 321 个测试全通过
cargo build -p fd-host-web --target wasm32-unknown-unknown    # ✅ WASM 编译通过
cargo run -p fd-cli -- --help                                  # ✅ CLI 可用
```

测试覆盖：
- `fd-canvas-core`：53+ 测试（PenDocument CRUD + UndoRedo + Diff/Patch + JSON 往返 + 布局 + 命名版本管理）
- `fd-ai-adapter`：60+ 测试（mock HTTP + SSE 流 + 健康检查 + 多模态视觉 + SpecDoc/PageFlow Skill）
- `fd-design-lint`：41 测试（13 条 Lint 规则 + auto_fix + apply_tokens + FixResult 序列化）
- `fd-design-system`：10+ 测试（Token + Theme + Registry + CSS 输出）
- `fd-ecosystem`：16 测试（IPC + sync_to_code + 模板标签检索 + 内置场景模板）
- `fd-asset`：19 测试（素材库 CRUD + 分类/标签/标注/颜色提取/Token 绑定）
- `fd-codegen` / `fd-host-web` / `fd-export` / `fd-host-desk`：各 4-10 测试
- `fd-cli`：5+ 测试（CLI 参数解析 + 子命令分发）

## 📄 许可证

MIT — [Fusion-MLX](https://github.com/fusion-mlx) Apple Silicon 本地 AI 生态的一部分。
