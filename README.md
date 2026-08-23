# Fusion-Design

> Local offline AI visual design workbench for macOS — built on OpenPencil (Rust) + fusion-mlx local multimodal inference, embedded in Fusion-Desk WKWebView.

**Status**: v0.1.11 — 12 crates + vendored op-ai, 379 tests pass + perf baseline, WASM build verified, CI green (fmt/clippy/test/wasm), release packaging. 3 built-in design presets (Apple HIG / minimal dashboard / robot-sim console), self-built MCP protocol layer in fd-ecosystem, op-editor-core/op-codegen/op-mcp replaced by fd-canvas-core/fd-codegen/fd-ecosystem. 新增 `chat` 子命令（机器可读流式 NDJSON，供 fusion-studio subprocess 取代直连 MLX，issue #17）+ 修复 chat_stream_messages EOF 尾帧丢失（issue #18）。

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

1. **Infinite vector canvas** — zoom/pan/grid/multi-artboard/layer management, CSS Flex/Grid layout, Taffy layout engine
2. **Conversational AI design generation** — text-to-UI, image-to-UI, SSE streaming, multimodal visual input
3. **Local design system** — two built-in specs (Apple HIG / Admin Dashboard), Light/Dark themes, global Token sync
4. **Design lint + auto-fix** — 13 lint rules + auto-fix for Token references / null cleanup / auto-naming
5. **Fusion Code bidirectional sync** — canvas one-click export to React/HTML/Tailwind, reverse style sync
6. **Prototype interaction & handoff** — PNG/SVG/PDF/HTML export, batch export
7. **Ecosystem integration** — Simulation / Desk / KB / CLI full integration, async file watching, template tag search
8. **Undo/Redo + Diff** — snapshot stack (50 levels) + node-level Diff/Patch
9. **Full-featured CLI** — generate / export / export-batch / lint --fix / undo / redo / health / diff / theme / chat（机器可读流式 NDJSON，供 fusion-studio subprocess 取代直连 MLX，issue #17）
10. **Asset library management** — asset categorization/tagging/annotation/color extraction/design system Token binding
11. **Design spec document generation** — AI auto-generates interaction specs / component specs / page architecture docs (SpecDocSkill)
12. **Page flow batch generation** — AI generates multi-page layouts from flow descriptions with unified style (PageFlowSkill)
13. **Named version management** — version snapshots / switching / renaming / deletion / adjacent diff comparison
14. **Built-in scene templates** — 4 preset categories (mobile app / admin dashboard / marketing site / mini program), one-click install

## 🗂️ Project Structure

```
fusion-design/
├── crates/                     ← Fusion-Design custom Rust crates (workspace)
│   ├── fd-canvas-core/         ← Canvas data model (PenDocument/PenNode + UndoRedo/Diff/Taffy + VersionedDocument)
│   ├── fd-ai-adapter/          ← op-ai → fusion-mlx adapter (SSE streaming / multimodal vision / 7 Skills)
│   ├── fd-codegen/             ← HTML/React+Tailwind code export
│   ├── fd-design-system/       ← Three built-in design specs (Apple HIG / minimal dashboard / robot-sim) + Token + Light/Dark themes
│   ├── fd-design-lint/         ← 13 lint rules + auto-fix (Token references / null cleanup / auto-naming)
│   ├── fd-ecosystem/           ← Ecosystem integration (IPC + async file watching + template tag search + 4 built-in scene templates + self-built MCP JSON-RPC server)
│   ├── fd-asset/               ← Asset library management (categorization / tagging / annotation / color extraction / Token binding)
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

## 🧪 Verification

```bash
cargo check --workspace                                        # Full workspace compiles
cargo test --workspace                                         # 376 tests pass (+1 ignored perf baseline)
cargo test --release -- --ignored perf_baseline                # 1000-node perf baseline (<500ms each)
cargo build -p fd-host-web --target wasm32-unknown-unknown    # WASM build succeeds
cargo run -p fd-cli -- --help                                  # CLI available
./Scripts/build.sh                                             # Release tarball → dist/
```

**Endpoint 覆盖**：CLI `--endpoint` 默认经 `FUSION_MLX_BASE_URL` 解析（优先级：显式 `--endpoint` > env > 默认 gateway 11432），可切回直连 fusion-mlx 11434。鉴权 key 经 `FUSION_MLX_API_KEY`。

**服务可用性校验**：`fusion-design check-mlx` 做三段真探测——endpoint 解析 → `/v1/models` 鉴权 + 模型列表 → 1-token 真推理探针。gateway 的 `/v1/models` 会「假绿」（列了云端/本地模型名但 MLX 未加载，generate 实返 502），故最终判定用真 chat 调用。探针模型解析：`--model` > `FUSION_MLX_MODEL` > 列表首个（建议显式传本地 mlx 模型 id）。不可用时以非零退出码 + 诊断文案 fail visibly。

**安全护栏**：`.fusiondesign` 反序列化限制节点嵌套 ≤64、总数 ≤100000；IPC 消息文件 ≤8MB，防恶意输入栈溢出/OOM。

**CI**：`.github/workflows/ci.yml` 在 push/PR 到 main 时自动跑 fmt + clippy(`-D warnings`) + test + wasm build（ubuntu），main 通过后 macos-14 产出 release tarball artifact。

| Crate | Tests | Coverage |
|-------|-------|----------|
| fd-canvas-core | 60+ | PenDocument CRUD, UndoRedo, Diff/Patch, JSON round-trip, layout, version management, **安全护栏**, perf baseline |
| fd-ai-adapter | 81 | Mock HTTP, SSE streaming, health check, **深度可用性探针（鉴权/模型/真推理）**, multimodal vision, SpecDoc/PageFlow, **E2E 生产路径**, endpoint 解析 |
| fd-design-lint | 41 | 13 lint rules, auto-fix, apply_tokens, FixResult serialization |
| fd-design-system | 25 | Token, Theme, Registry, CSS output, 3 built-in presets (incl. robot-sim) |
| fd-ecosystem | 27 | IPC, sync_to_code, template tag search, built-in scene templates, self-built MCP server, **文件大小护栏** |
| fd-asset | 19 | Asset CRUD, categorization/tagging/annotation, color extraction, Token binding |
| fd-codegen / fd-host-web / fd-export / fd-host-desk | 4-10 each | Core functionality |
| fd-cli | 7+ | CLI argument parsing, subcommand dispatch, **商用级错误报告** |

## 📦 Release

```bash
./Scripts/build.sh    # 产出 dist/fusion-design-<version>-aarch64-apple-darwin.tar.gz
```

含 `fusion-design` CLI 二进制（release strip+thin LTO）+ `fd_host_web_bg.wasm` + `fd_host_web.js`（WKWebView 前端，wasm-bindgen `--target web` 产物）+ INSTALL.md。100% 离线，仅 HTTP 至 `127.0.0.1`。

> **集成说明**：fusion-studio 的 `Scripts/build.sh` 从本仓 `target/wasm32-unknown-unknown/{release,debug}/` 拉取 `fd_host_web_bg.wasm` + `fd_host_web.js`。`build.sh` 已补 wasm-bindgen 后处理步骤，确保 bindgen 产物落地该目录——缺此步则 studio 同步脚本找不到 `_bg.wasm`，回退陈旧内置件。

## 📄 License

Licensed under the [Apache License 2.0](LICENSE).
