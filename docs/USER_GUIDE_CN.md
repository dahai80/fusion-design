# Fusion-Design 场景化使用指南

> [English](USER_GUIDE.md) | [中文](USER_GUIDE_CN.md)
>
> 面向使用者：按场景上手，遇错按现象速查（排障见 [TROUBLESHOOTING_CN.md](TROUBLESHOOTING_CN.md)）。

## 前置条件

| 项 | 要求 | 验证命令 |
|----|------|---------|
| 平台 | macOS Apple Silicon（M 系列） | `uname -m` → `arm64` |
| fusion-mlx | 本地推理服务运行中（端口 11434） | `~/claude-home/fusion-mlx/start.sh status` |
| fusion-gateway | 默认经 gateway 11432 转发（可选，直连也可） | `curl -s -m 5 http://127.0.0.1:11432/v1/models -H "Authorization: Bearer fg-admin-key" \| head -c 80` |
| CLI 二进制 | `fusion-design` 已构建 | `fusion-design --version` |
| 鉴权 key | 环境变量 `FUSION_MLX_API_KEY` | `echo $FUSION_MLX_API_KEY`（应非空，如 `fg-admin-key`） |

**起停 fusion-mlx**：

```bash
~/claude-home/fusion-mlx/start.sh start    # 启动（端口 11434）
~/claude-home/fusion-mlx/start.sh stop     # 停止
~/claude-home/fusion-mlx/start.sh status   # 查 PID/端口/内存/已载模型
~/claude-home/fusion-mlx/start.sh doctor   # 健康检查
```

**endpoint 选择**：CLI 默认经 fusion-gateway（11432）。若直连 fusion-mlx，设 `FUSION_MLX_BASE_URL=http://127.0.0.1:11434`。优先级：显式 `--endpoint` > `FUSION_MLX_BASE_URL` env > 默认 gateway 11432。

**模型下载**：缺模型时用镜像站，勿直连 HuggingFace：

```bash
HF_MIRROR=https://hf-mirror.com huggingface-cli download <model-id>
```

## 场景 1：首次文生 UI（最小可用闭环）

**目标**：一句自然语言 → `.fusiondesign` 画布文件。

**前提**：fusion-mlx 已起 + `FUSION_MLX_API_KEY` 已设（见前置条件）。

```bash
# 1. 先验 MLX 真可用（真推理探针，非仅列表）
fusion-design check-mlx --model Qwen3.5-9B-4bit

# 2. 文生 UI
fusion-design generate --prompt "登录页：邮箱+密码+记住我+登录按钮" --out login.fusiondesign
```

**预期输出**：`login.fusiondesign`（JSON，含 Home 页 + 节点树 + 样式）。终端打印生成耗时与 token 数。

**常见卡点**：
- `check-mlx` 返非零 → 见 [现象 B/C/D](TROUBLESHOOTING_CN.md)
- `generate` 卡住超 30s → 模型正在加载，M-5 重试会自动等，见 [现象 B](TROUBLESHOOTING_CN.md)
- 模型名报"不存在" → 显式传 `--model` 本地已加载 id，见 [现象 J](TROUBLESHOOTING_CN.md)

**调优**：默认 model `Qwen3.5-9B-4bit`。跨部署稳健做法是先 `check-mlx --endpoint http://127.0.0.1:11434` 探出真可用模型 id，再传给 `--model`。

## 场景 2：导出交付件（PNG/SVG/PDF/HTML/React）

**目标**：`.fusiondesign` → 可交付的图片/文档/代码。

**格式选型**：

| 需求 | 命令 | 产物 |
|------|------|------|
| 截图给文档/评审 | `export --format png` | 单页 PNG |
| 矢量无损交付 | `export --format svg` | 单页 SVG |
| 打印 / PDF 归档 | `export --format pdf` | 单页 PDF |
| 静态可预览 | `export --format html` | 自包含 HTML |
| 数据备份 | `export --format json` | 原始 JSON |

```bash
# 单页导出（--out 为输出目录，产物文件名=页名，如 Home.png）
fusion-design export --input login.fusiondesign --format png --out ./out
fusion-design export --input login.fusiondesign --format svg --out ./out
fusion-design export --input login.fusiondesign --format html --out ./out

# 批量多页（输入 JSON 数组，每项含 input/format/out，out 为目录）
echo '[{"input":"a.fusiondesign","format":"png","out":"./out/a"},
       {"input":"b.fusiondesign","format":"svg","out":"./out/b"}]' > batch.json
fusion-design export-batch --input batch.json --out ./out
```

**注意**：`--out` 是**目录**而非文件名，产物文件按页名命名（如画布页名 `Home` → `Home.png`）。多页画布会导出多个文件。要控制文件名，在画布里改页名（`--page` 参数）。

**常见卡点**：
- 产物里出现 `var(--color-x)` 未解析 → 见 [现象 H](TROUBLESHOOTING_CN.md)
- `--format` 报非法值 → 仅支持 `html svg json png pdf`（见上表）

## 场景 3：设计规范检测 + 自动修复

**目标**：检查画布是否符合设计规范（13 条规则），自动修可修项。

**13 条规则**：`contrast-check`（对比度）、`unlabeled-input`（未标注输入框）、`text-effects`（异常文字特效）、`abnormal-rotation`（异常旋转）、`empty-effects`（空特效）、`token-inconsistency`（Token 不一致）、`unnamed-node`（未命名节点）、`text-overflow`（文字溢出）、`overlapping-nodes`（节点重叠）、`hardcoded-spacing`（硬编码间距）、`hardcoded-font-size`（硬编码字号）、`missing-interaction-state`（缺交互态）、`layout-inconsistency`（布局不一致）。

```bash
# 全量检测（默认 apple-hig 规范）
fusion-design lint --input login.fusiondesign

# 指定规范 + 指定规则子集
fusion-design lint --input login.fusiondesign --design-system apple-hig \
  --rules contrast-check,unlabeled-input,token-inconsistency

# 仅预览自动修复（不写文件）
fusion-design lint --input login.fusiondesign --fix --dry-run

# 落地自动修复（Token 引用 / 空值清理 / 自动命名）
fusion-design lint --input login.fusiondesign --fix
```

**规范选项**：`apple-hig`（默认）、`minimal-dashboard`（极简后台）、`robot-sim`（机器人仿真控制台）。`fusion-design list-design-systems` 查全量，`fusion-design activate <id>` 切换激活规范。

**常见卡点**：lint 报告字段含义不清 → 见 [现象 K](TROUBLESHOOTING_CN.md)。

## 场景 4：草图 / 参考图逆向生成（图生 UI）

**目标**：手绘草图或参考图截图 → `.fusiondesign`。

```bash
# 基础：草图 → UI
fusion-design image-to-ui --sketch ./sketch.png --out from-sketch.fusiondesign

# 带文字提示引导风格
fusion-design image-to-ui --sketch ./ref.png --hint "极简后台仪表盘，深色主题" \
  --out dashboard.fusiondesign
```

**参数**：`--sketch`（图片路径，必填）、`--hint`（风格/场景提示，可空）、`--page`（页名，默认 Home）、`--model`、`--endpoint`、`--out`。

**前提**：所用模型须支持多模态视觉输入。`Qwen3.5-9B-4bit` 默认文本模型可能不支持图像，需切换多模态模型（用 `check-mlx` 探出真可用多模态模型 id 后传 `--model`）。见 [现象 J](TROUBLESHOOTING_CN.md)。

## 场景 5：多方案风格对比

**目标**：同一需求一次生成 3 套不同风格设计稿。

```bash
# 默认三风格
fusion-design multi-variants --prompt "电商首页：搜索栏+轮播+商品网格" \
  --out ./out/ecom

# 自定义三风格（逗号分隔）
fusion-design multi-variants --prompt "登录页" \
  --styles "极简白,深色科技,拟物卡片" --out ./out/login-variants
```

**参数**：`--prompt`（必填）、`--styles`（三风格逗号分隔，缺省用默认三风格）、`--page`、`--model`、`--endpoint`、`--out`。

**产物**：`./out/` 下 3 个 `.fusiondesign`，可分别 `export` 成图片对比。

## 场景 6：批量多页面 + 规范文档

**目标 A**：按流程描述批量生成多页面（统一风格）。

```bash
fusion-design page-flow --flow "首页→商品列表→详情→购物车→结算" \
  --style-hint "极简电商，主色蓝" --out ./out/flow
```

产物：`./out/flow` 下每页一个 `.fusiondesign`，风格统一。

**目标 B**：从已有画布生成设计规范文档（交互规范/组件规范/页面架构）。

```bash
fusion-design spec-doc --input login.fusiondesign --title "登录模块设计规范" \
  --out ./out/login-spec.md
```

**参数（page-flow）**：`--flow`（流程描述，必填）、`--style-hint`（风格提示）、`--model`、`--endpoint`、`--out`。
**参数（spec-doc）**：`--input`（`.fusiondesign`，必填）、`--title`（文档标题，默认"设计规范文档"）、`--model`、`--endpoint`、`--out`。

## 场景 7：设计稿 → 可运行代码（Codegen + Code 联动）

**目标**：`.fusiondesign` → 前端可运行代码（HTML / React+Tailwind / Tailwind-only / Swift UI）。

```bash
# HTML（默认）
fusion-design codegen --input login.fusiondesign --target html --out ./out/Login.html

# React + Tailwind
fusion-design codegen --input login.fusiondesign --target react-tailwind \
  --component LoginForm --out ./out/LoginForm.tsx

# 仅 Tailwind 类名
fusion-design codegen --input login.fusiondesign --target tailwind-only --out ./out/login.txt

# Swift UI（macOS 原生）
fusion-design codegen --input login.fusiondesign --target swift-ui \
  --component LoginView --out ./out/LoginView.swift
```

**参数**：`--input`（必填）、`--target`（`html` 默认 / `react-tailwind` / `tailwind-only` / `swift-ui`）、`--component`（组件名，默认 MyComponent）、`--out`。

**与 Fusion Code 联动**：导出后用 `--ipc-base` 经 fd-ecosystem IPC 推送到 Fusion Code 工程目录（文件式 IPC，无网络）：

```bash
fusion-design codegen --input login.fusiondesign --target react-tailwind \
  --out ./out/LoginForm.tsx --ipc-base ~/.fusion/ipc
```

**安全**：codegen 对 HTML 内容做实体转义（XSS 防护），CSS 注入净化。生成代码仅本地读写，不注入网络请求。见 [现象 L](TROUBLESHOOTING_CN.md)。

## 场景 8：CLI 管道流式推理（脚本集成）

**目标**：机器可读流式 chat，供 CLI 管道 / 脚本 / 自动化消费（NDJSON 成帧，非给人看）。

**契约**：每行一个 JSON 对象，`type` 字段三态——`delta`（增量 token）、`chat_done`（结束，含 `finish_reason`）、`error`（错误）。尾行 `chat_done` 后流终止。

```bash
# 准备消息文件（JSON 数组，每项 role+content）
echo '[{"role":"user","content":"用一句话描述登录页设计要点"}]' > /tmp/msgs.json

# 流式 + JSON 成帧
fusion-design chat --model Qwen3.5-9B-4bit \
  --system-prompt "你是 UI 设计顾问，回答简洁" \
  --messages-file /tmp/msgs.json --stream --json
```

**预期输出**（逐行）：
```
{"token":"登录","type":"delta"}
{"token":"页","type":"delta"}
{"finish_reason":"stop","type":"chat_done"}
```

**多轮历史**：`--messages-file` 传完整对话数组（含历史 assistant 消息）。**RAG 注入**：`--rag-context-file` 传检索到的上下文文本文件，拼入 prompt。

**脚本管道消费**（jq 逐行取 token）：

```bash
fusion-design chat --messages-file /tmp/msgs.json --stream --json \
  | while read line; do
      echo "$line" | jq -r 'select(.type=="delta") | .token' 2>/dev/null
    done
```

**重要**：本子命令 NDJSON schema（`delta`/`chat_done`/`error`）为 CLI 自洽契约，**供 CLI 管道/脚本/测试消费**。fusion-studio 实际走 fusion-gateway TCP NDJSON（帧 schema 为 `chat_event`/`chat_done`/`error`），**不经 fd-cli chat**，故本子命令无 studio 消费方（issue #17 已核实）。

**常见卡点**：
- `invalid type: map, expected a sequence` → `--messages-file` 要数组 `[{...}]` 非单对象 `{...}`
- CJK 流式乱码 `U+FFFD` → 见 [现象 E](TROUBLESHOOTING_CN.md)
- 流式经 gateway 偶发 502 → 已修（fusion-gateway#108，PR #111，2026-08-25），遇回退可直连见 [现象 B](TROUBLESHOOTING_CN.md)

## 场景 9：设计规范切换与 Token CSS

**目标**：切换内置设计规范，导出 Token CSS 供前端消费。

```bash
# 列出全部规范
fusion-design list-design-systems

# 激活某套（影响后续 lint / token-css / theme 默认）
fusion-design activate robot-sim

# 输出当前激活规范的 CSS Custom Properties（:root 变量）
fusion-design token-css > tokens.css

# 输出指定主题（light/dark）的 CSS 变量
fusion-design theme --mode dark > tokens-dark.css
```

**三套内置规范**：
- `apple-hig` — Apple Human Interface Guidelines，默认
- `minimal-dashboard` — 极简后台仪表盘
- `robot-sim` — 机器人仿真控制台

**用法**：`tokens.css` 在前端 `<link>` 或 `@import`，组件用 `var(--color-primary)` 等引用。换规范只需换 `tokens.css`，全站 Token 全局同步。

## 附录：22 子命令速查表

| 子命令 | 用途 | 必填参数 | 详见场景 |
|--------|------|---------|---------|
| `list-design-systems` | 列出已注册设计规范 | — | 场景 9 |
| `activate` | 激活一套设计规范 | `<id>` | 场景 9 |
| `generate` | 文生 UI（自然语言→画布） | `--prompt` | 场景 1 |
| `image-to-ui` | 图生 UI（草图/参考图→画布） | `--sketch` | 场景 4 |
| `multi-variants` | 多方案对比（3 套风格） | `--prompt` | 场景 5 |
| `page-flow` | 流程描述→批量多页面 | `--flow` | 场景 6 |
| `spec-doc` | AI 生成设计规范文档 | `--input` | 场景 6 |
| `lint` | 设计规范检测 + 自动修复 | `--input` | 场景 3 |
| `codegen` | 画布→前端代码（4 target） | `--input` | 场景 7 |
| `export` | 单页导出（png/svg/pdf/html/json） | `--input --format --out` | 场景 2 |
| `export-batch` | 批量导出（JSON 数组输入） | `--input --out` | 场景 2 |
| `parse-html` | HTML→PenDocument JSON | `--input` | — |
| `token-css` | 输出激活规范 CSS 变量 | — | 场景 9 |
| `theme` | 输出指定主题 CSS 变量 | `--mode` | 场景 9 |
| `chat` | 机器可读流式 chat（NDJSON） | — | 场景 8 |
| `check-mlx` | 校验 MLX endpoint 真可用 | — | 前置/场景 1 |
| `health` | 探测 MLX 健康状态 | — | 前置 |
| `undo` | 撤销（返回上一步快照） | `--input` | — |
| `redo` | 重做（返回下一步快照） | `--input` | — |
| `diff` | 比较两个画布差异 | `--input` | — |
| `check-frontend` | 校验前端静态资源目录 | `--input` | — |
| `train` | 基于设计语料微调模型（调 fusion-trainer） | — | — |

**通用参数**：AI 类子命令（`generate`/`image-to-ui`/`multi-variants`/`page-flow`/`spec-doc`/`chat`）均支持 `--model`（默认 `Qwen3.5-9B-4bit`）与 `--endpoint`（默认 gateway 11432，可 env 覆盖）。

**遇错**：任何子命令失败，先看终端诊断文案（fail visibly，含原因+建议），再查 [排障手册](TROUBLESHOOTING_CN.md)。设 `RUST_LOG=debug` 获取详细日志。
