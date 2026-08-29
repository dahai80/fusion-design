# Fusion-Design

[English](README.md) | [中文](README_CN.md)

> Local offline AI visual design workbench for macOS — built on OpenPencil (Rust) + fusion-mlx local multimodal inference, embedded in Fusion-Desk WKWebView.

**Status**: v0.1.12 — 11 crates + vendored op-ai, 419 tests pass, WASM build verified, CI green (fmt/clippy/test/wasm/deny-panic/deny-unwrap-expect/deny-external-http), release packaging. 3 built-in design presets (Apple HIG / minimal dashboard / robot-sim console), op-editor-core/op-codegen/op-design-lint replaced by fd-canvas-core/fd-codegen/fd-design-lint. v0.1.12：对抗安全审计全量修复（2 致命 + 13 关键 + 12 逻辑 + 8 架构 + 3 性能）—— codegen XSS 实体转义、离线 allowlist 强制（回环+RFC1918+链路本地，拒公网）、validate_limits 进反序列化边界、递归/图像/PNG/SSE/stdin 资源上限、IPC 路径遍历防护+原子写、reqwest 超时、CSS 注入净化、web_sys unwrap 降级、schema 版本号（文件+桥协议）、max_tokens 常量化、fd-host-desk 去 reqwest 依赖、CJK SSE 跨 chunk 切分修复（字节缓冲，无 U+FFFD）；死代码清理（fd-asset 整 crate、Taffy compute_layout、apply_patch、fd-ecosystem MCP+watch_async、VersionedDocument/NamedVersion）；chat NDJSON 自洽契约（delta/chat_done/error，CLI/脚本管道消费，非对齐 studio——studio 走 gateway TCP chat_event）；CI 三道零越界门禁（deny-panic + deny-unwrap-expect allowlist 基线 + deny-external-http 仅限 fd-ai-adapter 出站 HTTP）；P2/P3 收尾：E-14 截断文件告警、P2-3/R-A2 IPC consume 非破坏（解析失败保留文件不静默吞错）、H-A11 resolve_tokens 递归子节点 token 解析 + CLI codegen 接线、H-A17 离线分工文档化 + CI 出站 HTTP 审计门禁、TC-4 gateway 假绿真推理探针回归测试；含 XSS/超深/路径遍历/javascript: URL/CJK SSE 切分/离线边界/截断文件/IPC 非破坏/假绿探针对抗回归测试。

## 📋 Overview

Fusion-Design is one of the flagship products in the Fusion-MLX "one core, nine endpoints" matrix, positioned as a **fully local offline, AI conversational UI/prototype design workbench**.

| Dimension | Value |
|-----------|-------|
| Comparable products | Claude Design (cloud, closed-source), Figma, Penpot |
| Inference engine | fusion-mlx (local multimodal LLM inference, cloud models forbidden) |
| Platform | macOS Apple Silicon (M-series, Metal/ANE hardware acceleration) |
| Foundation | OpenPencil (Rust) secondary encapsulation + custom fusion-mlx adapter |
| Core positioning | Bidirectional sync with Fusion Code, design-to-runnable-code in one click |
| License | Apache-2.0 |

### Key Differentiators (vs. Claude Design)

- ✅ **100% local offline** — all AI generation, design files, and assets stay on-device, zero cloud uploads
- ✅ **Native Apple MLX** — local multimodal vision model driven, no API fees, no network dependency
- ✅ **Deep Fusion ecosystem integration** — links with simulation, automation, knowledge base, and code tools
- ✅ **Native desktop app** — embedded in Fusion-Desk WKWebView, no standalone web page needed
- ✅ **Open-source foundation** — no vendor lock-in, private deployment supported

## 🚀 V0.2 Feature Matrix

1. **Infinite vector canvas** — zoom/pan/grid/multi-artboard/layer management, CSS Flex/Grid layout declarations (rendered by layout-aware codegen / browser)
2. **Conversational AI design generation** — text-to-UI, image-to-UI, SSE streaming, multimodal visual input
3. **Local design system** — two built-in specs (Apple HIG / Admin Dashboard), Light/Dark themes, global Token sync
4. **Design lint + auto-fix** — 13 lint rules + auto-fix for Token references / null cleanup / auto-naming
5. **Fusion Code bidirectional sync** — canvas one-click export to React/HTML/Tailwind, reverse style sync
6. **Prototype interaction & handoff** — PNG/SVG/PDF/HTML export, batch export
7. **Ecosystem integration** — Simulation / Desk / KB / CLI full integration, async file watching, template tag search
8. **Undo/Redo + Diff** — snapshot stack (50 levels) + node-level Diff
9. **Full-featured CLI** — generate / export / export-batch / lint --fix / undo / redo / health / diff / theme / chat（机器可读流式 NDJSON delta/chat_done/error，CLI/脚本管道消费；issue #17 设想 studio 经此入口取代直连 MLX，但核实 studio 走 gateway TCP chat_event，本子命令无 studio 消费方）
10. **Design spec document generation** — AI auto-generates interaction specs / component specs / page architecture docs (SpecDocSkill)
11. **Page flow batch generation** — AI generates multi-page layouts from flow descriptions with unified style (PageFlowSkill)
12. **Named version management (REMOVED)** — version snapshots / switching / renaming / deletion / adjacent diff comparison
   > **死代码清理（H-A14/P2-4）**：`VersionedDocument`/`NamedVersion` 命名版本 API（save_version/switch_to/diff_versions 等）跨 crate 零生产消费者，`.fusiondesign` 实存裸 `PenDocument`，CLI undo/redo 走独立 `UndoRedoStack`。审计定性「260 行死代码」，save_version 同时入版本表+undo 栈致进程内双份深拷贝。已整段移除（同 H-A13 apply_patch）；随同移除 uuid_v4/now_iso/civil_from_days/VERSION_SEQ/MAX_VERSIONED_FILE_BYTES。撤销/重做能力不受影响（feature 8 的 `UndoRedoStack` 独立提供）。如未来需命名版本，按 delta-COW 设计，勿复活整快照方案。
13. **Built-in scene templates** — 4 preset categories (mobile app / admin dashboard / marketing site / mini program), one-click install

## 🗂️ Project Structure

```
fusion-design/
├── crates/                     ← Fusion-Design custom Rust crates (workspace)
│   ├── fd-canvas-core/         ← Canvas data model (PenDocument/PenNode + UndoRedo/Diff, Flex/Grid layout declarations)
│   ├── fd-ai-adapter/          ← op-ai → fusion-mlx adapter (SSE streaming / multimodal vision / 7 Skills)
│   ├── fd-codegen/             ← HTML/React+Tailwind code export (layout-aware Flex/Grid CSS)
│   ├── fd-design-system/       ← Three built-in design specs (Apple HIG / minimal dashboard / robot-sim) + Token + Light/Dark themes
│   ├── fd-design-lint/         ← 13 lint rules + auto-fix (Token references / null cleanup / auto-naming)
│   ├── fd-ecosystem/           ← Ecosystem integration (file IPC + template tag search + 4 built-in scene templates)
│   ├── fd-host-desk/           ← Fusion-Desk WKWebView host bridge
│   ├── fd-host-web/            ← WASM frontend rendering (WebShell + BridgeCommand)
│   ├── fd-export/              ← PNG/SVG/PDF/HTML batch export
│   └── fd-cli/                 ← CLI (generate/export/lint/undo/redo/health/diff/theme)
├── vendor/
│   └── openpencil/             ← Trimmed OpenPencil subset (Git subtree)
└── frontend/
    └── dist/                   ← Compiled static assets (loaded by WKWebView)
```

## 🛠️ Tech Stack

| Layer | Technology |
|-------|-----------|
| Foundation | OpenPencil (Rust workspace) — `op-ai` (vendored) |
| AI inference | fusion-mlx (local multimodal, Metal/ANE accelerated) |
| Host | Fusion-Desk WKWebView (macOS native) |
| Frontend | WASM (wasm32-unknown-unknown) + DOM/Canvas rendering |
| Communication | Local private HTTP (127.0.0.1) + WKWebView Bridge |
| Async | tokio + notify (file watching) + futures (SSE streaming) |

## 📖 User Guide

Scenario-based usage + troubleshooting (bilingual):

- **[Usage Guide](docs/USER_GUIDE.md)** / **[使用指南](docs/USER_GUIDE_CN.md)** — 9 scenarios: first text-to-UI, export, lint+fix, image-to-ui, multi-variant, batch+spec-doc, codegen, streaming pipeline, design-system switch. Plus a 22-subcommand quick reference.
- **[Troubleshooting](docs/TROUBLESHOOTING.md)** / **[排障手册](docs/TROUBLESHOOTING_CN.md)** — 12 symptoms → root cause → fix: MLX unreachable, 502/503 loading, false-green models, auth 401, stream break/CJK garble, deserialize over-limit, IPC/path-traversal, Token color unresolved, wasm/studio sync, model drift, lint rules, XSS guardrail.

## 🧪 Verification

```bash
cargo check --workspace                                        # Full workspace compiles
cargo test --workspace                                         # 419 tests pass (+1 ignored perf baseline)
bash Scripts/deny-unwrap-expect.sh                             # 零 unwrap/expect 门禁（allowlist 基线）
bash Scripts/deny-external-http.sh                             # 出站 HTTP 仅限 fd-ai-adapter（离线硬约束）
cargo test --release -- --ignored perf_baseline                # 1000-node perf baseline (<500ms each)
cargo build -p fd-host-web --target wasm32-unknown-unknown    # WASM build succeeds
cargo run -p fd-cli -- --help                                  # CLI available
./Scripts/build.sh                                             # Release tarball → dist/
```

**Endpoint 覆盖**：CLI `--endpoint` 默认经 `FUSION_MLX_BASE_URL` 解析（优先级：显式 `--endpoint` > env > 默认 gateway 11432），可切回直连 fusion-mlx 11434。鉴权 key 经 `FUSION_MLX_API_KEY`。

**服务可用性校验**：`fusion-design check-mlx` 做三段真探测——endpoint 解析 → `/v1/models` 鉴权 + 模型列表 → 1-token 真推理探针。gateway 的 `/v1/models` 会「假绿」（列了云端/本地模型名但 MLX 未加载，generate 实返 502），故最终判定用真 chat 调用。探针模型解析：`--model` > `FUSION_MLX_MODEL` > 列表首个（建议显式传本地 mlx 模型 id）。不可用时以非零退出码 + 诊断文案 fail visibly。

**M-5 重试退避**：`fd-ai-adapter` 三处 HTTP 路径（`blocking_post` / `chat_stream_messages` / `check_generate`）对 502/503 瞬时错误指数退避重试（500ms→1s→2s→4s→8s 封顶，默认 4 次），等模型加载完成即成功；4xx 永久错误直接失败。`FUSION_MLX_RETRY_MAX` 调最大尝试次数（设 1 关闭）。流式仅覆盖建连阶段，中途断流不重试。

**安全护栏**：`.fusiondesign` 反序列化限制节点嵌套 ≤64、总数 ≤100000；IPC 消息文件 ≤8MB，防恶意输入栈溢出/OOM。

**CI**：`.github/workflows/ci.yml` 在 push/PR 到 main 时自动跑 fmt + clippy(`-D warnings`) + test + wasm build + 双零 panic 门禁（`deny-panic` 查 `panic!`/`unimplemented!`/`todo!`/`unreachable!` 宏，`deny-unwrap-expect` 查裸 `.unwrap()`/`.expect()`，后者用 `Scripts/unwrap-expect-allowlist.txt` 基线，新增站点未在 allowlist 则失败）（ubuntu），main 通过后 macos-14 产出 release tarball artifact。

| Crate | Tests | Coverage |
|-------|-------|----------|
| fd-canvas-core | 46 | PenDocument CRUD, UndoRedo, Diff/Patch, JSON round-trip, layout, **安全护栏**, perf baseline |
| fd-ai-adapter | 85 | Mock HTTP, SSE streaming, health check, **深度可用性探针（鉴权/模型/真推理）**, multimodal vision, SpecDoc/PageFlow, **E2E 生产路径**, endpoint 解析, **CJK SSE 跨 chunk 切分**, **H-A7 ChatProvider 增量分块** |
| fd-design-lint | 41 | 13 lint rules, auto-fix, apply_tokens, FixResult serialization |
| fd-design-system | 25 | Token, Theme, Registry, CSS output, 3 built-in presets (incl. robot-sim) |
| fd-ecosystem | 22 | IPC, sync_to_code, template tag search, built-in scene templates, **文件大小护栏** |
| fd-codegen / fd-host-web / fd-export / fd-host-desk | 4-10 each | Core functionality |
| fd-cli | 7+ | CLI argument parsing, subcommand dispatch, **商用级错误报告** |

## 📦 Release

```bash
./Scripts/build.sh    # 产出 dist/fusion-design-<version>-aarch64-apple-darwin.tar.gz
```

含 `fusion-design` CLI 二进制（release strip+thin LTO）+ `fd_host_web_bg.wasm` + `fd_host_web.js`（WKWebView 前端，wasm-bindgen `--target web` 产物）+ INSTALL.md。100% 离线，仅 HTTP 至 `127.0.0.1`。

> **集成说明**：fusion-studio 的 `Scripts/build.sh` 从本仓 `target/wasm32-unknown-unknown/{release,debug}/` 拉取 `fd_host_web_bg.wasm` + `fd_host_web.js`。`build.sh` 已补 wasm-bindgen 后处理步骤，确保 bindgen 产物落地该目录——缺此步则 studio 同步脚本找不到 `_bg.wasm`，回退陈旧内置件。

## 🔧 Environment Variables

All optional. Unset = default. Affects runtime behavior without code changes.

| Variable | Default | 作用 / Effect | Set in |
|----------|---------|---------------|--------|
| `FUSION_MLX_BASE_URL` | `http://127.0.0.1:11432` (gateway) | fusion-mlx inference endpoint. CLI `--endpoint` overrides. Comma-separated multi-node failover supported (e.g. `http://a:11432,http://b:11432`). | fd-ai-adapter |
| `FUSION_MLX_API_KEY` | (none) | Bearer auth key for gateway/MLX. Must match the key configured on the service. | fd-ai-adapter |
| `FUSION_MLX_MODEL` | (list first) | Default model id for `check-mlx` probe (resolution: `--model` > env > `/v1/models` first entry). Pass an explicit local mlx id to avoid false-green on unloaded cloud models. | fd-cli |
| `FUSION_MLX_RETRY_MAX` | `4` | Max attempts on 502/503 transient errors (exponential backoff 500ms→1s→2s→4s→8s capped). Set `1` to disable retry. 4xx permanent errors fail immediately. | fd-ai-adapter |
| `FUSION_MLX_RETRY_DEADLINE_SECS` | `300` | Total deadline across all retry attempts. Exceeding bails even if attempts remain. | fd-ai-adapter |
| `FUSION_MLX_SSE_BUFFER_CAP` | `8388608` (8 MB) | Max bytes buffered for a single SSE stream before bail. Guards against runaway model output OOM. | fd-ai-adapter |
| `FUSION_MLX_STREAM_IDLE_SECS` | `60` | Max idle seconds between SSE chunks. Mid-stream stall beyond this emits error delta and fails visibly (no infinite hang). | fd-ai-adapter |
| `FUSION_LOG_DISABLE_FILE` | (unset) | Set `1` (or `true`) to disable file logging (stdout-only). Default writes daily-rotated logs to `~/Library/Logs/fusion-design/` (macOS) or `~/.local/share/fusion-design/logs` (Linux). | fd-cli |
| `FUSION_LOG_DIR` | (platform default) | Override the file-log directory. Default: `~/Library/Logs/fusion-design/` (macOS) / `~/.local/share/fusion-design/logs` (Linux). | fd-cli |
| `FUSION_VENV_ROOT` | (auto-detect) | Root path of the shared `.venv` for ecosystem tool invocation. Falls back to workspace-relative discovery. | fd-ecosystem |
| `FUSION_TRAINER_BIN` | `fusion-trainer` | Path to the fusion-trainer binary for ecosystem training IPC. Override when not on `PATH`. | fd-ecosystem |

> Audit note: `FUSION_MLX_STREAM_IDLE_SECS` is introduced by P2 FAULT-1 (v0.1.14). `FUSION_LOG_DISABLE_FILE` / `FUSION_LOG_DIR` (file-log toggle + dir override) belong to OPS-13 (v0.1.14, fd-cli 文件日志落地). The other env vars were previously undocumented or inline-only; this table is the single source of truth (OPS-16).

## 📄 License

Licensed under the [Apache License 2.0](LICENSE).
