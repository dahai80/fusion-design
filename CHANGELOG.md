---
doc_id: fusion-design/changelog
product: fusion-design
class: platform
entry: developer
doc_type: changelog
title: { zh: "变更日志", en: "Changelog" }
audience: developer
status: draft
version: 0.1.26
updated: 2026-11-07
owner: "@dahai"
source_of_truth:
  - docs/changes
  - README.md
---

# Changelog

逐版本全文见 `docs/changes/`（各文件即唯一事实源）。当前 workspace 版本 `0.1.26`（`Cargo.toml` `[workspace.package] version`）。

| 版本 | 日期 | 摘要 | 全文 |
|------|------|------|------|
| 0.1.26 | 2026-11 | 路由修复：未知 `/api/*` 返回 404，405 保留给已知路径错方法（issue #36） | [v0.1.26](docs/changes/v0.1.26.md) |
| 0.1.25 | 2026-11 | 统一 API 契约（字段定位 4xx）、GET-only tokens、pinning 契约测试、最小画布 UX（issue #35） | [v0.1.25](docs/changes/v0.1.25.md) |
| 0.1.24 | 2026-11 | region 迭代（O(region)）、`kind` 大小写不敏感 round-trip 修复 | [v0.1.24](docs/changes/v0.1.24.md) |
| 0.1.23 | 2026-11 | 工作台模型可选择（仅 chat 模型）；主题 / 语言跟随 Shell | [v0.1.23](docs/changes/v0.1.23.md) |
| 0.1.22 | 2026-11 | 修复存 Artifact 双写重复入库（REST 主、IPC 降级） | [v0.1.22](docs/changes/v0.1.22.md) |
| 0.1.21 | 2026-11 | Artifact 就地查看器 | [v0.1.21](docs/changes/v0.1.21.md) |
| 0.1.20 | 2026-11 | Design 入口改为 Artifact 落地页；存 Artifacts 真正入 fusion-artifacts-engine | [v0.1.20](docs/changes/v0.1.20.md) |
| 0.1.19 | 2026-11 | 工作台 UI 对齐 fusion-web-shell 设计语言 | [v0.1.19](docs/changes/v0.1.19.md) |
| 0.1.18 | 2026-11 | fd-server web 工作台 + 生态出口（issues #31/#32/#33） | [v0.1.18](docs/changes/v0.1.18.md) |
| 0.1.17 | 2026-10-30 | issue #30 闭合（`FUSION_CLIENT_KEY` 迁移告警） | [v0.1.17](docs/changes/v0.1.17.md) |
| 0.1.16 | 2026-10-29 | v0.1.16-rc.1 转正 | [v0.1.16](docs/changes/v0.1.16.md) / [rc.1](docs/changes/v0.1.16-rc.1.md) |
| 0.1.15 | 2026-08-30 | 闭合 post-v0.1.14 三项 P3 缺口 | [v0.1.15](docs/changes/v0.1.15.md) |
| 0.1.14 | 2026-08 | P2 维护版（运行时正确性 + FAULT-1 流空闲护栏 + OPS-13 文件日志）；issue #20 chat SSE 流式 | [v0.1.14](docs/changes/v0.1.14.md) / [issue-20](docs/changes/v0.1.14-issue-20.md) |
| 0.1.13 | 2026-08-28 | wasm-bindgen 版本固定 + WASM 诊断环形缓冲（OPS-8/OPS-13-WASM） | [v0.1.13](docs/changes/v0.1.13.md) |
| 0.1.12 | 2026-08-27 | 审计 50/53 项 P0-P3 闭合 + M-5 重试 | [v0.1.12](docs/changes/v0.1.12.md) |
| 0.1.3–0.1.9 | 2026-08 | 补丁序列（issue #5–#10、fd-host-web 泄漏修复、生产验收差距） | [v0.1.3](docs/changes/v0.1.3.md) … [v0.1.9](docs/changes/v0.1.9.md)，另见 [fix-host-web-leak-hang](docs/changes/fix-host-web-leak-hang.md) |
