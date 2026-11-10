---
doc_id: fusion-design/guide
product: fusion-design
class: platform
entry: developer
doc_type: guide
title: { zh: "Fusion-Design 场景化使用指南", en: "USER_GUIDE_CN" }
audience: developer
status: draft
version: 0.1.26
updated: 2026-11-07
owner: "@dahai"
source_of_truth:
  - docs/USER_GUIDE_CN.md
---
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
| 鉴权 key | 环境变量 `FUSION_CLIENT_KEY` | `echo $FUSION_CLIENT_KEY`（应非空，如 `fg-admin-key`） |

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

**前提**：fusion-mlx 已起 + `FUSION_CLIENT_KEY` 已设（见前置条件）。

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

**目标**：检查画布是否符合设计规范（17 条规则），自动修可修项。

**13 条经典规则**：`contrast-check`（对比度）、`unlabeled-input`（未标注输入框）、`text-effects`（异常文字特效）、`abnormal-rotation`（异常旋转）、`empty-effects`（空特效）、`token-inconsistency`（Token 不一致）、`unnamed-node`（未命名节点）、`text-overflow`（文字溢出）、`overlapping-nodes`（节点重叠）、`hardcoded-spacing`（硬编码间距）、`hardcoded-font-size`（硬编码字号）、`missing-interaction-state`（缺交互态）、`layout-inconsistency`（布局不一致）。**4 条 desigma 准入规则**（issue #41）：`bare-color-literal`（裸颜色字面量）、`off-token-spacing`（离刻度间距）、`off-token-radius`（离刻度圆角）、`missing-dark-pair`（缺暗色配对），全部 Error 级。

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

# desigma 准入门禁（issue #41）：退出码 0 通过 / 1 存在 Error 违规 / 2 运行时失败。
# stdout 输出管道可消费的违规 JSON；映射文档：docs/DESIGMA_TOKEN_MAPPING.md
fusion-design lint --input customer-page.fusiondesign \
  --design-system desigma-aligned --profile desigma --gate
```

**规范选项**：`apple-hig`（默认）、`minimal-dashboard`（极简后台）、`robot-sim`（机器人仿真控制台）、`desigma-aligned`（desigma 来源客户设计的推荐预设——每个颜色 Token 均带暗色配对）。`fusion-design list-design-systems` 查全量，`fusion-design activate <id>` 切换激活规范。

**规则集（profile）**：`--profile default` = 全部 17 条；`--profile desigma` = 4 条 Token 对齐门禁规则（`--gate` 输出二值退出码，供 cowork/CI 管道集成）。显式 `--rules` 优先于 profile。

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

## 场景 10：Web 工作台（fd-server，浏览器端）

**目标**：在浏览器 `127.0.0.1:11493` 完成完整设计闭环——对话生成、截图还原、版本回退、导出、生态交接，无需离开页面。

```bash
cargo build --release -p fd-server
./start.sh start          # 打开 http://127.0.0.1:11493
./start.sh status|stop|log
```

**落地页**（`http://127.0.0.1:11493/`，即 gateway shell 里的 Design 模块入口）：

1. **全部 / 我的 / 与您共享** 三 tab 列出 `fusion-artifacts-engine` 的产物（`GET /api/artifacts`），带搜索、排序（最近打开 / 创建日期 / 名称 / 置顶优先）与分页。
2. **创建新内容** 提供 文档 / 幻灯片 / 设计 三张卡片，跳 `workbench.html?kind=...&prompt=...`，提示词已预填、光标停在末尾等用户接话：
   - 设计 → `Make a Design artifact for `
   - 文档 → `Make a Document artifact about `
   - 幻灯片 → `Make a Slides artifact about `
3. 点已有产物卡片进入 **Artifact 查看器**（`artifact.html?id=<id>`）：就地渲染正文（HTML 走沙箱 iframe，其余走纯文本），头部可下载 HTML、或「在工作台继续」带回对话；
4. 工作台左上返回箭头回到 Artifacts 列表。

**工作台流程**：
1. 输入提示词（如 `登录页，邮箱+密码表单，主色 indigo`）→ `POST /api/generate` 在 WASM 画布渲染 v1。
2. 对话迭代（`把主按钮文案改成 Sign In`）→ `POST /api/iterate` 产出 v2；右侧版本列表支持点选查看与回退。 长文档可传 `region:[节点id,...]` 只送选中节点（O(region) 上下文替代整篇文档）；服务端把结果合并回完整页面，未列出的节点保证不变。提示词保持稳定顺序（文档 TOC → 最近变更 → 基准 JSON → 指令殿后），保证本地引擎 prefix cache 命中。
3. 粘贴或上传截图 → `POST /api/restore` 走本地视觉模型，新建可编辑会话。
4. 导出：**HTML** / **React** 按钮；或交接：**发给 Code**（`POST /api/send-to-code`，HTML+TSX 落 fusion-code IPC 目录）、**存 Artifacts**（`POST /api/save-artifact`：经 `POST /api/v1/artifacts/create` 在 fusion-artifacts-engine 建产物，落地页即可见；成功时不写 IPC，避免引擎 IPC 消费者重复入库。引擎不可达才回退写 `save_artifact` IPC 消息、由引擎恢复后补入库，响应带 `engine_degraded: true`）。

工作台右上可切换**模型**（仅对话模型，缺省 Qwen3.5-4B；选择持久化并随生成/迭代下发；截图还原固定用视觉模型）。右栏有**图层列表**（每个节点显示 kind + 名称，点击即选中，画布经 wasm 桥高亮该节点，画布点击也会反向高亮图层项）。画布标题栏有**撤销/重做**按钮（按版本栈步进，边界自动禁用）。完整 HTTP 契约——必填/可选字段、字段定位 4xx（`{"error","field"}`）、全部响应形状——见 [docs/API.md](API.md)。主题与语言跟随 shell 统一切换，模块内不另设开关（独立打开时回退 shell 约定键，默认深色/中文）。

**环境变量**：`FUSION_ARTIFACTS_URL`（默认 `http://127.0.0.1:11451`）、`FUSION_ARTIFACTS_API_KEY`（缺省读 `~/.fusion/artifacts/config.yaml` 的 `security.api_key`）、`FUSION_DESIGN_PORT`（默认 11493）、`FUSION_DESIGN_DATA_DIR`（IPC 发件箱 + 日志，默认 `~/.fusion-design`）、`FUSION_DESIGN_WASM_DIR`（画布资产；默认发现链 可执行文件目录 > cwd > target wasm 目录）。

**注意**：生成需本地推理栈在线（gateway :11432 或 MLX :11434 + `FUSION_CLIENT_KEY`）。生态交接的跨仓消费方为 fusion-code#228 / fusion-artifacts-engine#76。

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
