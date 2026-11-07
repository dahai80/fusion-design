---
doc_id: fusion-design/product-page
product: fusion-design
class: platform
entry: developer
doc_type: product-page
title: { zh: "Fusion-Design 产品页", en: "Fusion-Design product page" }
audience: developer
status: draft
version: 0.1.26
updated: 2026-11-07
owner: "@dahai"
platform:
  serves: [fusion-studio, fusion-web-shell, fusion-cli, fusion-cowork]
  builds_on: [fusion-mlx, fusion-gateway, fusion-artifacts-engine, fusion-code, fusion-trainer]
  hosts: [fd-cli, fd-server web workbench, fd-host-web WASM canvas]
  integration_surface: [HTTP API on 127.0.0.1:11493, CLI subcommands, file IPC outbox, WKWebView wasm bridge]
  extension_points: [workspace crates, fd-skills SkillContext trait, design-system presets, codegen backends]
source_of_truth:
  - README.md
  - PRD.md
  - Cargo.toml
---

# Fusion-Design 产品页

[English](../README.md) | [中文](../README_CN.md)

## 定位 / Positioning

Fusion-Design 是 Fusion 生态中的**设计平台层**（platform class，developer entry）：一套本地离线、AI 对话式的 UI / 原型设计工作台，面向开发者与设计工程流程，提供画布内核、AI 生成、代码导出、设计规范检测等可复用能力。

- **推理后端**：fusion-mlx（本地多模态推理，云模型禁止）；默认经 fusion-gateway `127.0.0.1:11432`，直连 `127.0.0.1:11434` 亦可。
- **宿主形态**：fusion-studio DMG（内嵌 fd-cli 与 WASM 画布）、独立 CLI tarball、fd-server web 工作台（`127.0.0.1:11493`，仅回环）。
- **离线约束**：100% 本地，HTTP 仅指向 `127.0.0.1` / 内网网段；CI 以 `deny-external-http` 门禁强制。

## 平台角色 / Platform block

| 维度 | 内容 |
|------|------|
| serves（谁消费它） | fusion-studio（WKWebView 内嵌画布 + CLI 子进程）、fusion-web-shell（`/shell/fusion-design/` 模块）、fusion-cli / fusion-cowork（工作流节点调用 fd-cli 子命令） |
| builds_on（它依赖谁） | fusion-mlx（推理）、fusion-gateway（路由与鉴权）、fusion-artifacts-engine（Artifact 入库，IPC + REST 双路）、fusion-code（`send-to-code` IPC 目录）、fusion-trainer（`train` 子命令微调） |
| hosts（它承载什么） | fd-cli（20+ 子命令）、fd-server（HTTP API + WASM 画布托管 + D1 工作台）、fd-host-web（wasm32 浏览器渲染层）、fd-skills（Skill 注册中心） |
| integration_surface | HTTP `127.0.0.1:11493`（fd-server）、CLI stdout / NDJSON 流式、文件 IPC outbox（`~/.fusion-design`）、`navigator.__fd_host_post` wasm 桥 |
| extension_points | workspace 新增 crate、`fd-skills::SkillContext` trait（新设计 Skill）、`fd-design-system` 预设注册、`fd-codegen` 导出后端 |

## 能力清单（消费者不必再写什么）/ Capabilities

每项能力回答：接入方省掉了什么自研，代价 / 约束是什么。

1. **矢量画布内核（fd-canvas-core）** — 消费者不必再实现 PenDocument 数据模型、Undo/Redo 栈（50 级快照）、节点级 Diff/Patch、Flex/Grid 布局声明。
   约束：快照式 undo 在超大文档上有全量克隆内存开销（见 docs/superpowers 计划）。
2. **AI 生成（fd-ai-adapter + fd-skills）** — 不必再写 OpenPencil ChatProvider 对接、SSE 流式解析、多模态视觉输入、重试 / 空闲超时护栏；文生 UI、图生 UI、多方案、SpecDoc、PageFlow 开箱即用。
   约束：仅限本地 MLX 后端；必须配置 `FUSION_CLIENT_KEY` 与网关一致。
3. **代码导出（fd-codegen）** — 不必再写 PenDocument → HTML / React+Tailwind 的布局感知转换。
   约束：导出格式目前仅 `html` / `react` 两种。
4. **设计规范体系（fd-design-system + fd-design-lint）** — 三套内置规范（Apple HIG / 极简仪表盘 / robot-sim）、Token 与明暗主题、13 条 lint 规则 + 自动修复。
   约束：内置预设外的规范需按 Registry API 自行注册。
5. **Web 工作台（fd-server）** — 不必再写生成 / 迭代 / 还原 / 导出 API、WASM 画布托管、Artifact 落地页；字段级 4xx 错误契约由契约测试固定。
   约束：仅回环监听，默认端口 11493，无多租户。
6. **生态出口（fd-ecosystem）** — 不必再实现向 fusion-code 发送 HTML+TSX 的 IPC、向 fusion-artifacts-engine 存 Artifact、模板标签检索、场景模板。
   约束：IPC 依赖对端目录约定；fusion-trainer 需在 PATH 或经 `FUSION_TRAINER_BIN` 指定。
7. **导出管线（fd-export）** — PNG / SVG / PDF / HTML 批量导出，CLI `export-batch` 消费 JSON 数组。
   约束：批量入参 schema 见 `crates/fd-cli`。

## 端到端链路 / End-to-end flow

```
自然语言 / 草图
  → fd-cli generate|image-to-ui （或 POST /api/generate）
  → fd-ai-adapter → fusion-gateway :11432 → fusion-mlx :11434（本地推理）
  → PenDocument JSON（fd-canvas-core）
  → fd-codegen 导出 HTML/React+Tailwind
  → send-to-code（fusion-code IPC）/ save-artifact（fusion-artifacts-engine）
```

## 集成入口

- HTTP API 契约：[api-reference.md](api-reference.md)（权威细则见 [API.md](API.md)）
- CLI 子命令：[get-started.md](get-started.md)、`fusion-design --help`
- 环境变量：[config-reference.md](config-reference.md)
- 架构与 crate 职责：[architecture.md](architecture.md)
