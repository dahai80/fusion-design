# Fusion-Design

> Mac 本地离线 AI 可视化设计工作台 — 基于 OpenPencil (Rust) 底座 + fusion-mlx 本地多模态推理，原生嵌入 Fusion-Desk WKWebView。

**Status**: V0.1 MVP planning — PRD + 集成方案已就位，代码尚未启动。

## 📋 Overview

`Fusion-Design` 是 Fusion-MLX「一核九端」产品矩阵的旗舰主力之一，定位为**纯本地离线、AI 对话式 UI/原形设计工作台**。

| 维度 | 值 |
|------|-----|
| 对标产品 | Claude Design（云端闭源）、Figma、Penpot |
| 底层内核 | fusion-mlx（本地多模态大模型推理，禁止云端模型） |
| 运行平台 | macOS Apple Silicon（M 系列，Metal/ANE 硬件加速） |
| 开发底座 | OpenPencil (Rust) 二次封装 + 自研 fusion-mlx 适配层 |
| 核心定位 | 与 Fusion Code 双向打通，设计一键生成本地可运行代码 |

### 核心差异化（vs Claude Design）

- ✅ **100% 本地离线** — 所有 AI 生成、设计文件、素材存储本机，零云端上传
- ✅ **原生 Apple MLX** — 本地多模态视觉模型驱动，无 API 付费、无网络依赖
- ✅ **深度打通 Fusion 生态** — 联动仿真、自动化、知识库、代码工具
- ✅ **桌面原生 App** — 内嵌 Fusion-Desk WKWebView，无需独立网页
- ✅ **开源底座可私有化** — 无闭源厂商锁定

## 🚀 V0.1 MVP 核心功能

1. **无限矢量画布** — 缩放/平移/网格/多画板/图层管理，CSS Flex/Grid 布局
2. **对话式 AI 设计生成** — 文生 UI、图生 UI、局部指令修改、多方案对比
3. **本地设计系统** — 三套内置规范（Apple HIG / 后台 / 机器人仿真），Token 全局同步
4. **Fusion Code 双向打通** — 画布一键导出 React/HTML/Tailwind，反向同步样式
5. **原型交互与交付** — 跳转/弹窗/表单原型，PNG/SVG/PDF/HTML 导出
6. **生态联动** — Simulation / Desk / KB / Model Hub / CLI 全系打通
7. **本地素材管理** — 图片库、图标库，全程本地无云端

## 📚 Documentation

| Doc | Content |
|-----|---------|
| [PRD.md](PRD.md) | 完整 PRD V0.1 MVP — 定位、功能模块、架构、性能指标、迭代路线 |
| [docs/OPENS_SOURCE_REFERENCES.md](docs/OPENS_SOURCE_REFERENCES.md) | 开源软件参考清单（含本地状态核实 + 选型决策矩阵） |
| [docs/INTEGRATION_PLAN.md](docs/INTEGRATION_PLAN.md) | 集成方案（OpenPencil 底座裁剪 + fusion-mlx 桥接 + 分阶段实施） |

## 🗂️ Project structure (V0.1 landed — 7 crates + vendored op-ai, 94 tests pass)

```
fusion-design/
├── PRD.md
├── README.md
├── docs/
│   ├── OPENS_SOURCE_REFERENCES.md
│   └ INTEGRATION_PLAN.md
├── crates/                     ← Fusion-Design 自研 Rust crate（workspace）
│   ├── fd-ai-adapter/          ← op-ai → fusion-mlx 适配层
│   ├── fd-design-system/       ← 三套内置设计规范 + Token 管理
│   ├── fd-ecosystem/           ← 对接 Fusion Code/Simulation/KB/CLI
│   ├── fd-host-desk/           ← Fusion-Desk WKWebView 宿主桥
│   ├── fd-export/              ← PNG/SVG/PDF/HTML 批量导出
│   └── fd-cli/                 ← 命令行（扩展 op-cli）
├── vendor/
│   └ openpencil/               ← 裁剪后的 OpenPencil 子集（Git subtree）
└── frontend/
    └ dist/                    ← 编译后静态文件（供 WKWebView 加载）
```

## 🛠️ Tech stack

- **底座**: OpenPencil (Rust workspace) — `op-editor-core` / `op-ai` / `op-codegen` / `op-mcp` / `op-design-lint`
- **AI 推理**: fusion-mlx (本地多模态，Metal/ANE 加速)
- **宿主**: Fusion-Desk WKWebView (macOS 原生)
- **通信**: 本地私有 HTTP (127.0.0.1) + MCP 协议

## 🧪 Verification

```bash
cargo check --workspace   # ✅ 全套编译通过
cargo test --workspace    # ✅ 125 个测试全通过，0 失败
```

测试覆盖：
- `fd-canvas-core`：14 测试（PenDocument/PenNode CRUD + JSON 往返）
- `fd-ai-adapter`：25 测试（含 8 个 mock HTTP server 端到端集成测试，验证 OpenAI 兼容 API 对接链路）
- `fd-codegen`：10 测试（HTML/React/Tailwind 代码导出 + token 解析）
- `fd-design-system` / `fd-host-desk` / `fd-export` / `fd-ecosystem`：各 4-10 测试

## 📄 License

MIT — part of the [Fusion-MLX](https://github.com/fusion-mlx) Apple Silicon local AI ecosystem.
