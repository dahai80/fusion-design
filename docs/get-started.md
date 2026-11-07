---
doc_id: fusion-design/get-started
product: fusion-design
class: platform
entry: developer
doc_type: get-started
title: { zh: "快速上手", en: "Get started" }
audience: developer
status: draft
version: 0.1.26
updated: 2026-11-07
owner: "@dahai"
source_of_truth:
  - README.md
  - docs/DEPLOYMENT.md
  - start.sh
---

# 快速上手 / Get started

前提：macOS Apple Silicon，Rust `1.94`（见 `rust-toolchain.toml`；Homebrew cargo 可能忽略该文件，用 `cargo +1.94`）。推理依赖本地 fusion-mlx 或 fusion-gateway。

## 1. 构建工作区

```bash
cd /Users/dahai/fusion/fusion-design
cargo build --release
```

## 2. 准备本地推理

```bash
~/fusion/fusion-mlx/start.sh start        # fusion-mlx on 127.0.0.1:11434
# 或已启动 fusion-gateway（127.0.0.1:11432，默认 endpoint）
```

探测模型可用性（避免未加载模型造成的假绿）：

```bash
FUSION_CLIENT_KEY=<key> ./target/release/fusion-design check-mlx
```

## 3. 首次生成（CLI）

```bash
./target/release/fusion-design generate "一个带侧边栏的管理后台登录页" \
  --output login.json
./target/release/fusion-design codegen --input login.json --format react --out-dir ./out
./target/release/fusion-design lint --input login.json
```

## 4. 启动 Web 工作台（可选）

```bash
cargo build --release -p fd-server
./start.sh start          # 127.0.0.1:11493，仅回环
./start.sh status
curl -s http://127.0.0.1:11493/api/health
```

工作台 UI 打开 `http://127.0.0.1:11493`，含 Artifact 落地页、图层列表、撤销 / 重做。

## 5. 环境变量（可选）

| 变量 | 默认 | 说明 |
|------|------|------|
| `FUSION_MLX_BASE_URL` | `http://127.0.0.1:11432` | 推理 endpoint，逗号分隔可做多节点故障转移 |
| `FUSION_CLIENT_KEY` | 无 | 网关 / MLX Bearer 鉴权 key |
| `FUSION_MLX_MODEL` | 模型列表首项 | `check-mlx` 探测的默认模型 id |
| `FUSION_DESIGN_PORT` | `11493` | fd-server 监听端口 |

完整列表见 [config-reference.md](config-reference.md)。

## 6. 测试与验证

```bash
cargo test --workspace
./Scripts/deny-panic.sh && ./Scripts/deny-unwrap-expect.sh && ./Scripts/deny-external-http.sh
```

## 下一步

- 试试完整生成→导出→入库链路：[recipes/generate-and-export.md](recipes/generate-and-export.md)
- 跑起 Web 工作台并验证 API：[recipes/run-design-workbench.md](recipes/run-design-workbench.md)
- 部署三通道（studio DMG / CLI tarball / WASM）：[DEPLOYMENT.md](DEPLOYMENT.md)

## 常见问题

- 模型未加载导致生成 502：用 `check-mlx` 选一个 `model_loaded: true` 的模型 id，详见 [troubleshooting.md](troubleshooting.md)。
- 端口被占用：`FUSION_DESIGN_PORT` 换端口后 `./start.sh restart`（stop + start）。
