---
doc_id: fusion-design/readme
product: fusion-design
class: platform
entry: developer
doc_type: readme
title: { zh: "Fusion-Design", en: "README_CN" }
audience: developer
status: draft
version: 0.1.27
updated: 2026-11-07
owner: "@dahai"
source_of_truth:
  - README_CN.md
---
# Fusion-Design

[English](README.md) | [中文](README_CN.md)

> macOS 本地离线 AI 可视化设计工作台 — 基于 OpenPencil (Rust) 底座 + fusion-mlx 本地多模态推理，原生嵌入 Fusion-Desk WKWebView。

**状态**：v0.1.26 — 14 个 crate + vendored op-ai，556 个测试通过，WASM 构建验证通过，CI 全绿（fmt/clippy/test/wasm/deny-panic/deny-unwrap-expect/deny-external-http/cargo-audit），发布打包就绪。v0.1.27 闭合 issue #41——fd-design-lint 新增 desigma 准入门禁（4 条 Error 级 Token 对齐规则：裸颜色字面量 / 离刻度间距 / 离刻度圆角 / 缺暗色配对；`--profile desigma --gate` 为 cowork 提供二值退出码；新增内置 `desigma-aligned` 预设（每个颜色 Token 均带暗色配对）；映射文档 `docs/DESIGMA_TOKEN_MAPPING.md`）——以及 issue #42——Web 工作台成为 union-web 标准 shell 模块（`web/manifest.json`（PRD §5.1）、`?embedded=1`、受限取色 Token 面板、画布拖拽经新增 `POST /api/session/<id>/mutate` 落库、保存 .fusiondesign、401 内联提示绝不跳转；gateway 托管已上游提 issue dahai80/fusion-gateway#601）。 v0.1.26：修复路由——未知 `/api/*` 路径返回 404 而非误导性 405（验收 issue #36），405 仅保留给"路径存在但方法不对"；验收报告所称 `api/v1/share-base` 前端引用在本仓 bundle 中不存在（share 状态在 fusion-artifacts-engine）。v0.1.25：闭合验收 issue #35——统一 API 契约（`docs/API.md`，generate/iterate/rollback/restore/export 全部字段定位 4xx：`{"error","field"}`），tokens 端点仅 GET 且 405 带 hint，契约测试套件钉住全部形状，最小画布 UX——图层列表 + 节点选中经 wasm 桥联动（node.select/node.click）+ 可见撤销/重做（版本栈指针步进）。v0.1.24：长文档区域迭代——`/api/iterate` 接受 `region:[节点id,...]`，只送选中节点 JSON（O(region) 替代 O(doc)），提示词固定稳定前缀序（sys 恒定，user = TOC → 变更日志 → 基准 JSON → 指令殿后），结果由服务端确定性合并，未列出节点保证不变；同时修复 kind 大小写解析回环 bug（模型回显 PascalCase 不再失败）。v0.1.23：工作台模型可切换（只列对话模型，不再误显 embedding 模型 bge-m3），主题/语言跟随 shell 统一切换（移除模块自带开关）。v0.1.22：修复存 Artifacts 重复入库——原先既写 IPC 又走 REST，引擎自带的 IPC 消费者会再建一条 `type=data` 副本；现改为 REST 优先、仅引擎不可达时回退 IPC。v0.1.21：新增 Artifact 查看器——点卡片就地打开 `artifact.html?id=...` 渲染正文（HTML 走沙箱 iframe），可下载、可回工作台继续，不再重复开对话。v0.1.20：Design 模块入口改为 Artifacts 落地页（全部 / 我的 / 与您共享三 tab、搜索 + 排序、文档 / 幻灯片 / 设计三张创建卡片跳对话框并预填提示词、分页产物列表），「存 Artifacts」真正在 fusion-artifacts-engine 建产物（不再只写无人消费的 IPC 消息）。v0.1.19：工作台 UI 对齐 fusion-web-shell 设计语言。v0.1.18 新增 fd-server Web 工作台。详见 `docs/changes/v0.1.27.md` 与 `docs/API.md`。

v0.1.18：Web 工作台落地——`fd-server` 于 `127.0.0.1:11493` 提供对话式生成/迭代、截图还原（本地视觉模型）、版本回退、HTML/React 导出、WASM 画布托管，及生态出口 `/api/send-to-code` + `/api/save-artifact`（issue #31/#33）；gateway/supervisor 注册跨仓跟踪（fusion-gateway#448、fusion-supervisor#89）。详见 `docs/changes/v0.1.18.md`。

## 📋 概览

Fusion-Design 是 Fusion-MLX「一核九端」产品矩阵的旗舰主力之一，定位为**纯本地离线、AI 对话式 UI/原型设计工作台**。

| 维度 | 值 |
|------|-----|
| 对标产品 | Claude Design（云端闭源）、Figma、Penpot |
| 推理引擎 | fusion-mlx（本地多模态大模型推理，禁止云端模型） |
| 运行平台 | macOS Apple Silicon（M 系列，Metal/ANE 硬件加速） |
| 开发底座 | OpenPencil (Rust) 二次封装 + 自研 fusion-mlx 适配层 |
| 核心定位 | 与 Fusion Code 双向同步，设计一键生成本地可运行代码 |
| 许可证 | Apache-2.0 |

### 核心差异化（对比 Claude Design）

- ✅ **100% 本地离线** — 所有 AI 生成、设计文件、素材存储本机，零云端上传
- ✅ **原生 Apple MLX** — 本地多模态视觉模型驱动，无 API 付费、无网络依赖
- ✅ **深度联动 Fusion 生态** — 联动仿真、自动化、知识库、代码工具
- ✅ **桌面原生应用** — 内嵌 Fusion-Desk WKWebView，无需独立网页
- ✅ **开源底座可私有化** — 无闭源厂商锁定

## 🚀 V0.2 功能矩阵

1. **无限矢量画布** — 缩放/平移/网格/多画板/图层管理，CSS Flex/Grid 布局声明（由 layout-aware codegen / 浏览器渲染）
2. **对话式 AI 设计生成** — 文生 UI、图生 UI、SSE 流式输出、多模态视觉输入
3. **本地设计系统** — 三套内置规范（Apple HIG / 极简后台 / robot-sim 控制台），Light/Dark 主题，Token 全局同步
4. **设计规范检测 + 自动修复** — 13 条 Lint 规则 + auto_fix Token 引用 / 空值清理 / 自动命名
5. **Fusion Code 双向同步** — 画布一键导出 React/HTML/Tailwind，反向同步样式
6. **原型交互与交付** — PNG/SVG/PDF/HTML 导出，批量导出
7. **生态联动** — Simulation / Desk / KB / CLI 全系互联，异步文件监听，模板标签检索
8. **撤销/重做 + Diff** — 快照栈（50 层）+ 节点级 Diff
9. **CLI 全功能** — generate / export / export-batch / lint --fix / undo / redo / health / diff / theme / chat（机器可读流式 NDJSON delta/chat_done/error，CLI/脚本管道消费；issue #17 设想 studio 经此入口取代直连 MLX，但核实 studio 走 gateway TCP chat_event，本子命令无 studio 消费方）
10. **设计规范文档生成** — AI 自动生成交互规范/组件规范/页面架构文档（SpecDocSkill）
11. **页面流程批量生成** — AI 按流程描述批量生成多页面，统一风格（PageFlowSkill）
12. **命名版本管理（已移除）** — 版本快照/切换/重命名/删除/相邻 diff 对比
   > **死代码清理（H-A14/P2-4）**：`VersionedDocument`/`NamedVersion` 命名版本 API（save_version/switch_to/diff_versions 等）跨 crate 零生产消费者，`.fusiondesign` 实存裸 `PenDocument`，CLI undo/redo 走独立 `UndoRedoStack`。审计定性「260 行死代码」，save_version 同时入版本表+undo 栈致进程内双份深拷贝。已整段移除（同 H-A13 apply_patch）；随同移除 uuid_v4/now_iso/civil_from_days/VERSION_SEQ/MAX_VERSIONED_FILE_BYTES。撤销/重做能力不受影响（feature 8 的 `UndoRedoStack` 独立提供）。如未来需命名版本，按 delta-COW 设计，勿复活整快照方案。
13. **内置场景模板** — 4 类预设（移动端/B 端后台/营销网站/小程序），一键安装

## 🗂️ 项目结构

```
fusion-design/
├── crates/                     ← Fusion-Design 自研 Rust crate（workspace）
│   ├── fd-canvas-core/         ← 画布数据模型（PenDocument/PenNode + UndoRedo/Diff，Flex/Grid 布局声明）
│   ├── fd-ai-adapter/          ← op-ai → fusion-mlx 适配层（SSE 流式/多模态视觉/7 个 Skill）
│   ├── fd-codegen/             ← HTML/React+Tailwind 代码导出（layout-aware Flex/Grid CSS）
│   ├── fd-design-system/       ← 三套内置设计规范（Apple HIG / 极简后台 / robot-sim）+ Token + Light/Dark 主题
│   ├── fd-design-lint/         ← 13 条 Lint 规则 + auto_fix（Token 引用/空值清理/自动命名）
│   ├── fd-ecosystem/           ← 生态联动（文件 IPC + 模板标签检索 + 4 类内置场景模板）
│   ├── fd-host-desk/           ← Fusion-Desk WKWebView 宿主桥
│   ├── fd-host-web/            ← WASM 前端渲染（WebShell + BridgeCommand）
│   ├── fd-export/              ← PNG/SVG/PDF/HTML 批量导出
│   ├── fd-server/              ← Web 工作台 HTTP 服务（:11493，issue #31）— 对话生成/迭代/还原/导出/回退 + WASM 画布托管
│   └── fd-cli/                 ← 命令行（generate/export/lint/undo/redo/health/diff/theme/chat）
├── vendor/
│   └── openpencil/             ← 裁剪后的 OpenPencil 子集（Git subtree）
└── frontend/
    └── dist/                   ← 编译后静态文件（供 WKWebView 加载）
```

## 🛠️ 技术栈

| 层级 | 技术 |
|------|------|
| 底座 | OpenPencil (Rust workspace) — `op-ai` (vendored) |
| AI 推理 | fusion-mlx（本地多模态，Metal/ANE 加速） |
| 宿主 | Fusion-Desk WKWebView（macOS 原生） |
| 前端 | WASM (wasm32-unknown-unknown) + DOM/Canvas 渲染 |
| 通信 | 本地私有 HTTP (127.0.0.1) + WKWebView Bridge |
| 异步 | tokio + notify（文件监听）+ futures（SSE 流） |

## 📖 使用指南

场景化上手 + 排障（双语）：

- **[使用指南](docs/USER_GUIDE_CN.md)** / **[Usage Guide](docs/USER_GUIDE.md)** — 9 个场景：首次文生 UI、导出交付件、Lint+修复、图生 UI、多方案对比、批量+规范文档、Codegen、流式管道、设计规范切换。附 22 子命令速查表。
- **[排障手册](docs/TROUBLESHOOTING_CN.md)** / **[Troubleshooting](docs/TROUBLESHOOTING.md)** — 12 类现象 → 根因 → 解决：MLX 不通、502/503 加载中、假绿模型、鉴权 401、流式断/CJK 乱码、反序列化超限、IPC/路径遍历、Token 颜色未解析、wasm/studio 同步、模型漂移、lint 规则、XSS 护栏。

## 🧪 验证

```bash
cargo check --workspace                                        # 全套编译通过
cargo test --workspace                                         # 419 个测试全通过（+1 ignored perf 基线）
bash Scripts/deny-unwrap-expect.sh                             # 零 unwrap/expect 门禁（allowlist 基线）
bash Scripts/deny-external-http.sh                             # 出站 HTTP 仅限 fd-ai-adapter（离线硬约束）
cargo test --release -- --ignored perf_baseline                # 1000 节点 perf 基线（每节点 <500ms）
cargo build -p fd-host-web --target wasm32-unknown-unknown    # WASM 构建通过
cargo run -p fd-cli -- --help                                  # CLI 可用
./Scripts/build.sh                                             # Release tarball → dist/
```

**Endpoint 覆盖**：CLI `--endpoint` 默认经 `FUSION_MLX_BASE_URL` 解析（优先级：显式 `--endpoint` > env > 默认 gateway 11432），可切回直连 fusion-mlx 11434。鉴权 key 经 `FUSION_CLIENT_KEY`（legacy 别名 `FUSION_MLX_API_KEY` 迁移期兜底，2027-01-01 起删除）。

**服务可用性校验**：`fusion-design check-mlx` 做三段真探测——endpoint 解析 → `/v1/models` 鉴权 + 模型列表 → 1-token 真推理探针。gateway 的 `/v1/models` 会「假绿」（列了云端/本地模型名但 MLX 未加载，generate 实返 502），故最终判定用真 chat 调用。探针模型解析：`--model` > `FUSION_MLX_MODEL` > 列表首个（建议显式传本地 mlx 模型 id）。不可用时以非零退出码 + 诊断文案 fail visibly。

**M-5 重试退避**：`fd-ai-adapter` 三处 HTTP 路径（`blocking_post` / `chat_stream_messages` / `check_generate`）对 502/503 瞬时错误指数退避重试（500ms→1s→2s→4s→8s 封顶，默认 4 次），等模型加载完成即成功；4xx 永久错误直接失败。`FUSION_MLX_RETRY_MAX` 调最大尝试次数（设 1 关闭）。流式仅覆盖建连阶段，中途断流不重试。

**流式与 gateway**：默认 endpoint 经 fusion-gateway(11432)。gateway 流式 502 bug（fusion-gateway#108：`stream=true` 连接拒绝）已于 2026-08-25 修复（PR #111 local-first ordering），真流式探针通过（SSE delta 正常）。若遇上游回退，可 `FUSION_MLX_BASE_URL=http://127.0.0.1:11434` 直连 MLX 绕过 gateway。

**fd-cli chat 消费方声明（issue #17 诚实回溯）**：issue #17 设想 `chat` 子命令为 fusion-studio subprocess 入口取代直连 MLX，但经核实 fusion-studio 实际走 fusion-gateway TCP NDJSON（`StreamingBridge.swift`，帧 schema 为 `chat_event`/`chat_done`/`error` + `session_id`/`event`），**不经 fd-cli chat**。故本子命令无 studio 消费方，NDJSON 帧 schema（`delta`/`chat_done`/`error`）为本子命令自洽契约，供 CLI 管道/脚本/测试消费，非对齐 studio。

**安全护栏**：`.fusiondesign` 反序列化限制节点嵌套 ≤64、总数 ≤100000；IPC 消息文件 ≤8MB，防恶意输入栈溢出/OOM。

**Web 工作台（fd-server，issue #31/#33）**：浏览器工作台由 `fusion-design-server` 于 `127.0.0.1:11493` 提供——与 CLI 同一 Rust 内核，零云端调用。`cargo build --release -p fd-server && ./start.sh start` 后打开 http://127.0.0.1:11493 。能力：`/api/generate` `/api/iterate`（重试 + 模型 JSON 修复链）、`/api/restore`（截图 → 本地视觉模型 → 可编辑文档）、`/api/export`（HTML/React）、`/api/rollback` + 版本查询、生态出口 `/api/send-to-code`（HTML+TSX 落 fusion-code IPC 目录）与 `/api/save-artifact`（`save_artifact` IPC 消息供 artifacts-engine 消费）、WASM 画布托管。真模型 E2E：`cargo test -p fd-server --test e2e_real_model -- --ignored`（需 gateway :11432 + `FUSION_CLIENT_KEY` + 视觉模型已加载）。环境变量（`FUSION_DESIGN_PORT` / `FUSION_DESIGN_DATA_DIR` / `FUSION_DESIGN_WASM_DIR`）见 [README.md](README.md#-environment-variables)。

**CI**：`.github/workflows/ci.yml` 在 push/PR 到 main 时自动跑 fmt + clippy(`-D warnings`) + test + wasm build + 双零 panic 门禁（`deny-panic` 查 `panic!`/`unimplemented!`/`todo!`/`unreachable!` 宏，`deny-unwrap-expect` 查裸 `.unwrap()`/`.expect()`，后者用 `Scripts/unwrap-expect-allowlist.txt` 基线，新增站点未在 allowlist 则失败）+ `deny-external-http` 出站 HTTP 仅限 fd-ai-adapter（ubuntu）；macOS-15 跑 check+test 消除平台盲区（2026-11-02 起 macos-14 runner image 退役，已迁移 macos-15）；tag 触发 release tarball。

| Crate | 测试数 | 覆盖范围 |
|-------|--------|----------|
| fd-canvas-core | 46 | PenDocument CRUD、UndoRedo、Diff/Patch、JSON 往返、布局、**安全护栏**、perf 基线 |
| fd-ai-adapter | 85 | Mock HTTP、SSE 流、健康检查、**深度可用性探针（鉴权/模型/真推理）**、多模态视觉、SpecDoc/PageFlow、**E2E 生产路径**、endpoint 解析、**CJK SSE 跨 chunk 切分**、**H-A7 ChatProvider 增量分块**、**M-5 重试退避** |
| fd-design-lint | 41 | 13 条 Lint 规则、auto_fix、apply_tokens、FixResult 序列化 |
| fd-design-system | 25 | Token、Theme、Registry、CSS 输出、3 套内置预设（含 robot-sim） |
| fd-ecosystem | 22 | IPC、sync_to_code、模板标签检索、内置场景模板、**文件大小护栏** |
| fd-codegen / fd-host-web / fd-export / fd-host-desk | 各 4-10 | 核心功能 |
| fd-cli | 7+ | CLI 参数解析、子命令分发、**商用级错误报告**、**NDJSON 契约回归** |

## 📦 发布

```bash
./Scripts/build.sh    # 产出 dist/fusion-design-<version>-aarch64-apple-darwin.tar.gz
```

含 `fusion-design` CLI 二进制（release strip+thin LTO）+ `fd_host_web_bg.wasm` + `fd_host_web.js`（WKWebView 前端，wasm-bindgen `--target web` 产物）+ INSTALL.md。100% 离线，仅 HTTP 至 `127.0.0.1`。

> **集成说明**：fusion-studio 的 `Scripts/build.sh` 从本仓 `target/wasm32-unknown-unknown/{release,debug}/` 拉取 `fd_host_web_bg.wasm` + `fd_host_web.js`。`build.sh` 已补 wasm-bindgen 后处理步骤，确保 bindgen 产物落地该目录——缺此步则 studio 同步脚本找不到 `_bg.wasm`，回退陈旧内置件。

## 📄 许可证

基于 [Apache License 2.0](LICENSE) 许可。
