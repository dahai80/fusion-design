---
doc_id: fusion-design/api-reference
product: fusion-design
class: platform
entry: developer
doc_type: api-reference
title: { zh: "API 参考", en: "API reference" }
audience: developer
status: draft
version: 0.1.26
updated: 2026-11-07
owner: "@dahai"
source_of_truth:
  - docs/API.md
  - crates/fd-server/src/handlers.rs
  - crates/fd-server/src/router.rs
---

# API 参考 / API reference

fd-server 是 Fusion-Design 的 web 服务面（issue#31 / DS-0）。Base URL `http://127.0.0.1:11493`，仅回环监听，100% 离线。**逐字段权威契约见 [API.md](API.md)**，由 `crates/fd-server` 契约测试固定；本文是索引与调用方摘要。

## 约定

- Content type：`application/json; charset=utf-8`。
- 4xx 错误为字段定位格式：`{"error": "...", "field": "prompt"}`；405 附带 `method`/`path`/`hint`；413 为超过 8 MB 请求体；502 为上游模型失败（重试耗尽后）。
- 成功响应为普通 JSON 对象；`document` 字段是 **JSON 字符串**（序列化的 PenDocument，与 `.fusiondesign` 文件同构），不是嵌套对象。
- 未知 `/api/*` 路径返回 404；已知路径用错方法返回 405（v0.1.26 起）。

## Health & meta

| Method/Path | Response |
|-------------|----------|
| `GET /api/health` | `{service, version, sessions, mlx}`；`?deep=1` 追加 MLX 探针（`probed, available, model, chat_model, auth_ok, models[], status`） |
| `GET /api/design-systems` | `{systems[], active, theme}` |
| `GET /api/sessions` | `{sessions[]}` |
| `GET /api/tokens?ds=<id>&theme=light\|dark` | `{design_system, theme, css}`；未知 `ds` → 404；GET-only，其余方法 → 405 + hint |
| `GET /api/artifacts?tab=all\|mine\|shared&page&page_size≤100&sort&q` | Artifact 分页（fusion-artifacts-engine 代理） |
| `GET /api/artifact/<id>?version=<n>` | Artifact 详情 |
| `GET /api/session/<id>` | `{session, current_version, versions[...]}` |
| `GET /api/version/<id>/<n>` | `{version, document}` |

## AI 端点（POST，JSON body）

| Endpoint | 必填 | 可选 | 说明 |
|----------|------|------|------|
| `POST /api/generate` | `prompt` | `page, model, design_system` | 文生 UI，返回 `{session, current_version, document}` |
| `POST /api/iterate` | `session, instruction` | `model, design_system, region:[node_id,…]` | 带 `region` 时只发送选中节点（O(region)），未列节点保证不变；空输出 → 502 |
| `POST /api/restore` | `image_base64`（纯 base64，无 `data:` 前缀） | `hint, page, model, session, design_system` | 图生 UI（视觉模型，固定默认） |
| `POST /api/rollback` | `session, version` | — | 移动版本指针，不产生新版本 |
| `POST /api/export` | `session, format`（`html`\|`react`） | `version` | 返回 `{filename, content, mime}` |
| `POST /api/send-to-code` | `session` | `version, component` | HTML+TSX 写入 fusion-code IPC 目录 |
| `POST /api/save-artifact` | `session` | `version, title, kind` | 存入 fusion-artifacts-engine；返回 `{ok, artifact, engine_degraded, ipc_written}` |

## CLI 子命令（进程级 API）

`fusion-design` 二进制暴露同等能力，脚本可消费：`generate` / `image-to-ui` / `multi-variants` / `spec-doc` / `page-flow` / `codegen` / `lint` / `export` / `export-batch` / `undo` / `redo` / `diff` / `parse-html` / `token-css` / `theme` / `chat`（NDJSON 流式 delta / chat_done / error）/ `check-mlx` / `check-frontend` / `health` / `train` / `list-design-systems` / `activate`。以 `fusion-design --help` 为准。

## 契约测试

形状由 `crates/fd-server/src/handlers.rs`（`mod contract_tests`）与 `src/router.rs`（`method_not_allowed_contract`）固定。修改契约必须同一提交内更新本文与测试。
