---
doc_id: fusion-design/config-reference
product: fusion-design
class: platform
entry: developer
doc_type: config-reference
title: { zh: "配置参考", en: "Config reference" }
audience: developer
status: draft
version: 0.1.26
updated: 2026-11-07
owner: "@dahai"
source_of_truth:
  - README.md
  - start.sh
  - docs/DEPLOYMENT.md
---

# 配置参考 / Config reference

全部配置经环境变量，均**可选**，不设即用默认值。权威表格在 `README.md` 环境变量一节（OPS-16 定为唯一事实源）。

## 推理（fd-ai-adapter）

| 变量 | 默认 | 说明 |
|------|------|------|
| `FUSION_MLX_BASE_URL` | `http://127.0.0.1:11432` | 推理 endpoint（默认走 fusion-gateway）。CLI `--endpoint` 可覆盖；逗号分隔支持多节点故障转移 |
| `FUSION_CLIENT_KEY` | 无 | 网关 / MLX Bearer 鉴权 key，须与服务端一致。旧别名 `FUSION_MLX_API_KEY` 迁移期可用（会告警，2027-01-01 移除，issue #30） |
| `FUSION_MLX_RETRY_MAX` | `4` | 502/503 瞬态错误最大尝试次数（指数退避 500ms→…→8s 封顶）；`1` 关闭重试；4xx 立即失败 |
| `FUSION_MLX_RETRY_DEADLINE_SECS` | `300` | 全部重试的总时限 |
| `FUSION_MLX_SSE_BUFFER_CAP` | `8388608`（8 MB） | 单条 SSE 流最大缓冲，防模型失控输出 OOM |
| `FUSION_MLX_STREAM_IDLE_SECS` | `60` | SSE 块间最大空闲秒数，超时发 error delta 并可见失败（v0.1.14 P2 FAULT-1） |

## CLI（fd-cli）

| 变量 | 默认 | 说明 |
|------|------|------|
| `FUSION_MLX_MODEL` | 模型列表首项 | `check-mlx` 探测默认模型（解析顺序 `--model` > env > `/v1/models` 首项） |
| `FUSION_LOG_DISABLE_FILE` | 未设 | 设 `1`/`true` 关闭文件日志（仅 stdout） |
| `FUSION_LOG_DIR` | 平台默认 | 日志目录覆盖；默认 `~/Library/Logs/fusion-design/`（macOS）/ `~/.local/share/fusion-design/logs`（Linux），日轮转（OPS-13） |

## Web 工作台（fd-server）

| 变量 | 默认 | 说明 |
|------|------|------|
| `FUSION_DESIGN_PORT` | `11493` | 监听端口（workspace port-registry 属主 `fusion-design`） |
| `FUSION_DESIGN_DATA_DIR` | `~/.fusion-design` | 数据目录：IPC outbox（fusion-code / artifacts-engine 消息）与 `logs/server.log` |
| `FUSION_DESIGN_WASM_DIR` | 发现链 | `fd_host_web_bg.wasm` + `fd_host_web.js` 目录；发现链：exe 目录 > cwd > `target/wasm32-unknown-unknown/{release,debug}` |

## 生态联动（fd-ecosystem）

| 变量 | 默认 | 说明 |
|------|------|------|
| `FUSION_VENV_ROOT` | 自动发现 | 共享 `.venv` 根路径，用于生态工具调用；回退到 workspace 相对发现 |
| `FUSION_TRAINER_BIN` | `fusion-trainer` | fusion-trainer 二进制路径（`train` 子命令 IPC），不在 PATH 时覆盖 |

## 端口与生命周期

- fd-server 固定注册端口 `11493`（`architecture/port-registry.yaml`，属主 `fusion-design`）。
- 生命周期用 `./start.sh start|stop|status|log [-f]`；PID 文件 `.server.pid`，日志 `$FUSION_DESIGN_DATA_DIR/logs/server.log`。

## 工具链约束（构建期）

- Rust `1.94`（`rust-toolchain.toml`）；Homebrew cargo 会忽略该文件，用 `cargo +1.94`。
- wasm-bindgen-cli 锁 `0.2.126`，须与运行时 `wasm-bindgen` 版本一致（`Scripts/build.sh` 校验）。
