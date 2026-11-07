---
doc_id: fusion-design/guide
product: fusion-design
class: platform
entry: developer
doc_type: guide
title: { zh: "故障排查", en: "Troubleshooting" }
audience: developer
status: draft
version: 0.1.26
updated: 2026-11-07
owner: "@dahai"
source_of_truth:
  - docs/TROUBLESHOOTING.md
  - docs/DEPLOYMENT.md
  - Scripts
---

# 故障排查 / Troubleshooting

本文是 docs-system 契约入口；按症状分类的长篇排查手册见 [TROUBLESHOOTING.md](TROUBLESHOOTING.md)（EN）/ [TROUBLESHOOTING_CN.md](TROUBLESHOOTING_CN.md)。

## 症状速查

### 生成返回 502（上游模型失败）

1. `fusion-design check-mlx` 探测：确认 `model_loaded: true` 的模型 id（未加载模型会假绿）。
2. 确认鉴权：`FUSION_CLIENT_KEY` 与网关 / MLX 服务端一致（4xx 不重试，立即失败）。
3. 确认 endpoint：默认 `FUSION_MLX_BASE_URL=http://127.0.0.1:11432`（gateway）；直连 MLX 用 `127.0.0.1:11434`。
4. 502/503 会按 `FUSION_MLX_RETRY_MAX`（默认 4）指数退避重试；重试耗尽仍 502 再看网关日志。

### SSE 流中断 / 卡住不动

- 流空闲超过 `FUSION_MLX_STREAM_IDLE_SECS`（默认 60s）会主动发 error delta 并失败退出，属预期行为（FAULT-1），不是死锁。
- 单流缓冲超 `FUSION_MLX_SSE_BUFFER_CAP`（8 MB）主动中止；模型输出失控时会出现。

### fd-server 起不来 / 端口冲突

```bash
./start.sh status                 # PID 文件 .server.pid
tail -50 ~/.fusion-design/logs/server.log
FUSION_DESIGN_PORT=12493 ./start.sh start   # 换端口
```

- 二进制缺失：`cargo build --release -p fd-server`（start.sh 会提示）。
- 端口 `11493` 已被占：换 `FUSION_DESIGN_PORT` 后重启；port-registry 属主为 fusion-design。

### WASM 画布空白 / studio 拉到旧 wasm

1. `rustup target add wasm32-unknown-unknown`。
2. wasm-bindgen-cli 必须锁 `0.2.126`（与运行时版本一致）：`cargo +1.94 install wasm-bindgen-cli --version 0.2.126`。
3. `./Scripts/build.sh` 产出后确认 `target/wasm32-unknown-unknown/release/fd_host_web_bg.wasm` 存在；fusion-studio 从该目录拉取，缺 bindgen 后处理步骤会回退到陈旧内置件。
4. 自定义目录用 `FUSION_DESIGN_WASM_DIR` 指定（发现链：exe 目录 > cwd > target wasm 目录）。

### Homebrew cargo 忽略 rust-toolchain.toml

`/opt/homebrew/bin/cargo`（1.96+）会忽略 `rust-toolchain.toml`，用 `~/.cargo/bin/cargo +1.94`，否则编译报 edition/版本错误（见 DEPLOYMENT.md 前提条件）。

### CI 门禁失败

| 门禁 | 脚本 | 解法 |
|------|------|------|
| deny-panic | `Scripts/deny-panic.sh` | 移除 `panic!`/`unimplemented!`/`todo!`/`unreachable!` 宏，返回 `Result` |
| deny-unwrap-expect | `Scripts/deny-unwrap-expect.sh` | 新裸 `.unwrap()`/`.expect()` 未入 `Scripts/unwrap-expect-allowlist.txt` 即失败；修代码或审慎更新基线 |
| deny-external-http | `Scripts/deny-external-http.sh` | 新增出站 HTTP 只能指向 `127.0.0.1`/内网；fd-server 禁 reqwest |

### 存 Artifact 重复入库

v0.1.22 已修复（REST 为主、IPC 仅降级回退）。仍复现时确认 fusion-artifacts-engine 版本 ≥ 修复版，并检查 `~/.fusion-design` 下 IPC outbox 是否有积压消息。

### 迭代后未选中的节点被改动

v0.1.24 起 `/api/iterate` 带 `region:[node_id,…]` 保证未列节点不变；不带 region 为全文档迭代。确认客户端版本 ≥ 0.1.24，且节点 `kind` 大小写不敏感解析（v0.1.24 修复）。

## 日志位置

| 组件 | 位置 |
|------|------|
| fd-cli 文件日志 | `~/Library/Logs/fusion-design/`（macOS）/ `~/.local/share/fusion-design/logs`（Linux），日轮转；`FUSION_LOG_DIR` 可覆盖，`FUSION_LOG_DISABLE_FILE=1` 关闭 |
| fd-server | `$FUSION_DESIGN_DATA_DIR/logs/server.log`（默认 `~/.fusion-design/logs/`） |

## 历史手册

- [TROUBLESHOOTING.md](TROUBLESHOOTING.md) / [TROUBLESHOOTING_CN.md](TROUBLESHOOTING_CN.md)：v0.1.12 时期的症状 A–J 全量排查（含跨部署模型漂移）。
- [DEPLOYMENT.md](DEPLOYMENT.md)：三通道部署与 OPS-7 Gatekeeper 限制。
