---
doc_id: fusion-design/recipe/run-design-workbench
product: fusion-design
class: platform
entry: developer
doc_type: recipe
title: { zh: "配方：跑通 Web 工作台并验证 API", en: "Recipe: run the web workbench and verify the API" }
audience: developer
status: draft
version: 0.1.26
updated: 2026-11-07
owner: "@dahai"
source_of_truth:
  - crates/fd-server
  - start.sh
  - docs/API.md
---

# 配方：跑通 fd-server Web 工作台并验证 API

目标：本地起 fd-server（127.0.0.1:11493），用 curl 验证生成→迭代→导出链路与错误契约。

## 步骤

1. 构建 fd-server 并启动：

```bash
cd /Users/dahai/fusion/fusion-design
cargo build --release -p fd-server
./start.sh start
./start.sh status
```

2. 验证健康（必做）：`curl -s http://127.0.0.1:11493/api/health` 返回 `{service, version, sessions, mlx}`；加 `?deep=1` 应看到 MLX 探针结果。

3. 走一次生成链路：

```bash
curl -s -X POST http://127.0.0.1:11493/api/generate \
  -H "Content-Type: application/json" \
  -d '{"prompt":"一个简洁的登录页"}'
```

记下返回的 `session` id。

4. 验证迭代与回滚（用上一步的 session id）：

```bash
curl -s -X POST http://127.0.0.1:11493/api/iterate \
  -H "Content-Type: application/json" \
  -d '{"session":"<session-id>","instruction":"把主色改成深蓝"}'
curl -s -X POST http://127.0.0.1:11493/api/rollback \
  -H "Content-Type: application/json" \
  -d '{"session":"<session-id>","version":1}'
```

5. 验证错误契约（必做）：

```bash
curl -s -X POST http://127.0.0.1:11493/api/generate -d '{}' \
  -H "Content-Type: application/json"    # 期望 400 {"error":...,"field":"prompt"}
curl -s http://127.0.0.1:11493/api/not-exist      # 期望 404
curl -s -X POST http://127.0.0.1:11493/api/tokens -d '{}' \
  -H "Content-Type: application/json"    # 期望 405 + hint
```

6. 停止：`./start.sh stop`。

## 常见坑

- 端口占用 / 换端口：`FUSION_DESIGN_PORT` 覆盖后必须重启 start.sh；端口注册属主是 fusion-design（`architecture/port-registry.yaml`）。
- `document` 字段是 JSON **字符串**（序列化 PenDocument），不是嵌套对象；客户端需二次解析。
- WASM 画布空白：确认 `target/wasm32-unknown-unknown/release/fd_host_web_bg.wasm` 存在，wasm-bindgen-cli 锁 0.2.126，或用 `FUSION_DESIGN_WASM_DIR` 显式指定。
- 服务仅监听回环地址，从其他机器访问不通是预期（100% 离线约束）。
- 真模型 E2E 测试是 `--ignored` 用例：`cargo test -p fd-server --test e2e_real_model -- --ignored`，需 gateway :11432 + `FUSION_CLIENT_KEY` + 已加载视觉模型。
