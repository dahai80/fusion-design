---
doc_id: fusion-design/architecture
product: fusion-design
class: platform
entry: developer
doc_type: architecture
title: { zh: "架构", en: "Architecture" }
audience: developer
status: draft
version: 0.1.26
updated: 2026-11-07
owner: "@dahai"
source_of_truth:
  - Cargo.toml
  - README.md
  - crates
---

# 架构 / Architecture

Fusion-Design 是一个 Rust workspace：13 个自研 `fd-*` crate + 1 个 vendored OpenPencil 子集（`op-ai`）。版本 `0.1.26`，edition 2021，rust-version 1.94。全部出站推理经 `fd-ai-adapter` 指向本地 fusion-mlx / fusion-gateway；`fd-server` 仅做回环入站监听（127.0.0.1:11493）。

## Crate 分层

```
                ┌──────────── 宿主 / 服务面 ────────────┐
  fusion-studio │ fd-host-desk (WKWebView 桥)           │
  web 工作台     │ fd-server (HTTP :11493 + WASM 托管)   │
                │ fd-host-web (wasm32 渲染层)           │
                └──────┬────────────────────────────────┘
                       │
  CLI                  │ fd-cli (20+ 子命令, 文件日志)
                       │
                ┌──────┴─────────── AI 层 ──────────────┐
                │ fd-ai-adapter (op-ai ChatProvider →   │
                │   fusion-mlx, SSE/多模态/重试护栏)     │
                │ fd-skills (SkillContext trait + 7     │
                │   skill + 注册中心, 零 HTTP)          │
                │ op-ai (vendored OpenPencil 子集)      │
                └──────┬────────────────────────────────┘
                       │
                ┌──────┴───────── 领域内核 ─────────────┐
                │ fd-canvas-core (PenDocument/PenNode,  │
                │   UndoRedo, Diff/Patch, Flex/Grid)    │
                │ fd-design-system (3 预设+Token+主题)  │
                │ fd-design-lint (13 规则+自动修复)     │
                │ fd-html-parser (HTML→PenDocument)     │
                └──────┬────────────────────────────────┘
                       │
                ┌──────┴───────── 出口 ─────────────────┐
                │ fd-codegen (→HTML/React+Tailwind)     │
                │ fd-export (PNG/SVG/PDF/HTML 批量)     │
                │ fd-ecosystem (IPC/模板检索/场景模板)  │
                │ fd-css-utils (颜色解析+值净化, 叶子)  │
                └───────────────────────────────────────┘
```

## 各 crate 职责（Cargo.toml description 摘录）

| Crate | 职责 | 关键事实 |
|-------|------|----------|
| `fd-canvas-core` | 画布内核，替身 OpenPencil jian-ops-schema | 46 测试：CRUD/UndoRedo/Diff/JSON round-trip/安全护栏/perf |
| `fd-ai-adapter` | op-ai → fusion-mlx 适配，本地多模态推理 | 85 测试；SSE 跨 chunk CJK 切分；endpoint 解析 |
| `fd-skills` | Skill 系统：trait 化能力 + 注册中心，零 HTTP | ARCH-11 自 fd-ai-adapter 迁出，解耦推理客户端 |
| `fd-canvas-core` 依赖 `fd-css-utils` | CSS 安全工具叶子 crate | 无 fd-* 依赖 |
| `fd-html-parser` | HTML → PenDocument | 叶子 crate，仅依赖 fd-canvas-core + scraper |
| `fd-design-system` | 三套内置规范 + Token + 明暗主题 | Apple HIG / 极简仪表盘 / robot-sim |
| `fd-design-lint` | 设计规范检测 | 13 规则 + 自动修复（Token 引用 / null 清理 / 自动命名） |
| `fd-codegen` | PenDocument → HTML / React+Tailwind | 布局感知 Flex/Grid CSS |
| `fd-export` | PNG/SVG/PDF/HTML 批量导出 | 消费 fd-canvas-core |
| `fd-ecosystem` | 生态联动 | 文件 IPC、模板标签检索、4 类内置场景模板 |
| `fd-host-web` | wasm32-unknown-unknown 浏览器渲染入口 | 零 jian 依赖；`node.select`/`node.click` 桥事件 |
| `fd-host-desk` | Fusion-Desk WKWebView 宿主桥 | 通信层 |
| `fd-cli` | CLI 批量生成 / 导出 | 文件日志（OPS-13 tracing-appender 日轮转） |
| `fd-server` | web 服务面 | 生成/迭代/还原/导出 API + WASM 托管 + 工作台 |
| `op-ai`（vendored） | OpenPencil AI 子集，standalone manifest | 不入 workspace members（`exclude`） |

## 关键决策记录（Cargo.toml 注释溯源）

- **op-* 替身策略**：`op-editor-core` → fd-canvas-core、`op-codegen` → fd-codegen、`op-design-lint` → fd-design-lint；`op-mcp` 未替代（H-A3/P2-2 移除不合规自建 MCP 层）。
- **fd-asset 已删除**（H-A12/P1-6）：整 crate 死代码、零生产调用。
- **命名版本 API 已删除**（H-A14/P2-4）：`VersionedDocument`/`NamedVersion` 跨 crate 零消费者；undo/redo 由 `UndoRedoStack` 独立提供。
- **纯 Rust TLS**：reqwest `default-features=false` + `rustls-tls`，避免链接系统 OpenSSL（离线约束 + CI 镜像差异）。
- **fd-server 入站栈**：hyper 1.x 仅 `server/http1` 特性，零客户端能力；`deny-external-http` 对其白名单入站、禁 reqwest。

## 数据流

1. 生成：CLI/fd-server → `fd-ai-adapter` → gateway `127.0.0.1:11432` → fusion-mlx `127.0.0.1:11434` → PenDocument JSON。
2. 迭代：`/api/iterate` 可带 `region:[node_id,…]`，只发送选中节点，稳定前缀提示词（系统提示 → TOC → changelog → 基线 JSON → 指令），结果确定性合并回文档（v0.1.24）。
3. 出口：`send-to-code` 写 fusion-code IPC 目录；`save-artifact` REST 为主、IPC 为降级回退（v0.1.22 修复双写）。
4. 渲染：fd-host-web 产出 `fd_host_web_bg.wasm` + `fd_host_web.js`；fusion-studio 从 `target/wasm32-unknown-unknown/{release,debug}/` 拉取。

## 质量门禁（.github/workflows/ci.yml）

push/PR 到 main：`cargo fmt` + `clippy -D warnings` + `cargo test --workspace` + wasm build + `deny-panic`（查 `panic!`/`unimplemented!`/`todo!`/`unreachable!`）+ `deny-unwrap-expect`（裸 `.unwrap()`/`.expect()`，allowlist 基线 `Scripts/unwrap-expect-allowlist.txt`）+ `deny-external-http`；main 通过后 macos-15 产出 release tarball。README 记录 546+ 测试通过。
