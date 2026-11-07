---
doc_id: fusion-design/recipe/generate-and-export
product: fusion-design
class: platform
entry: developer
doc_type: recipe
title: { zh: "配方：文生 UI 到代码导出全链路", en: "Recipe: text-to-UI through code export" }
audience: developer
status: draft
version: 0.1.26
updated: 2026-11-07
owner: "@dahai"
source_of_truth:
  - crates/fd-cli
  - crates/fd-ai-adapter
  - crates/fd-codegen
---

# 配方：文生 UI → 代码导出全链路

目标：从一句自然语言描述，产出 PenDocument JSON、React+Tailwind 代码，并通过设计规范检测。全部本地离线。

## 步骤

1. 构建并确认推理可用：

```bash
cd /Users/dahai/fusion/fusion-design
cargo build --release
~/fusion/fusion-mlx/start.sh status    # 或确认 gateway :11432 在跑
```

2. 探测真实可用模型（避免未加载模型假绿）：

```bash
FUSION_CLIENT_KEY=<your-key> ./target/release/fusion-design check-mlx
```

3. 文生 UI：

```bash
FUSION_CLIENT_KEY=<your-key> ./target/release/fusion-design generate \
  "一个带左侧导航和统计卡片的管理后台首页" --output dashboard.json
```

4. 导出 React+Tailwind 并跑 lint：

```bash
./target/release/fusion-design codegen --input dashboard.json --format react --out-dir ./out
./target/release/fusion-design lint --input dashboard.json
```

5. 验证（必做）：`./out` 下存在生成的 `.tsx`/HTML 文件；`lint` 退出码 0 或输出的问题已在设计系统 Token 内处理；重新生成时内容随 prompt 变化。

## 常见坑

- 502：模型未加载或 key 不一致。`check-mlx` 选 `model_loaded: true` 的模型；`FUSION_CLIENT_KEY` 必须与网关一致。
- 直连 MLX 时 endpoint 是 `11434`，默认值是 gateway `11432`；用 `--endpoint` 覆盖。
- `codegen` 只支持 `html` / `react` 两种 format，传别的会报参数错误。
- 多节点故障转移：`FUSION_MLX_BASE_URL` 逗号分隔多个 endpoint，第一个失败自动切换。
- 批量导出用 `export-batch`，入参是 JSON 数组（schema 见 `crates/fd-cli`），不要循环单页调用。
