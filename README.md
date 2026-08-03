# Fusion-Design

> Local offline AI visual design workbench for macOS — built on OpenPencil (Rust) + fusion-mlx local multimodal inference, embedded in Fusion-Desk WKWebView.

**Status**: V0.2 — 11 crates + vendored op-ai, 321 tests pass, WASM build verified.

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
9. **Full-featured CLI** — generate / export / export-batch / lint --fix / undo / redo / health / diff / theme
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
│   ├── fd-design-system/       ← Two built-in design specs + Token + Light/Dark themes
│   ├── fd-design-lint/         ← 13 lint rules + auto-fix (Token references / null cleanup / auto-naming)
│   ├── fd-ecosystem/           ← Ecosystem integration (IPC + async file watching + template tag search + 4 built-in scene templates)
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
cargo test --workspace                                         # 321 tests pass
cargo build -p fd-host-web --target wasm32-unknown-unknown    # WASM build succeeds
cargo run -p fd-cli -- --help                                  # CLI available
```

| Crate | Tests | Coverage |
|-------|-------|----------|
| fd-canvas-core | 53+ | PenDocument CRUD, UndoRedo, Diff/Patch, JSON round-trip, layout, version management |
| fd-ai-adapter | 60+ | Mock HTTP, SSE streaming, health check, multimodal vision, SpecDoc/PageFlow |
| fd-design-lint | 41 | 13 lint rules, auto-fix, apply_tokens, FixResult serialization |
| fd-design-system | 10+ | Token, Theme, Registry, CSS output |
| fd-ecosystem | 16 | IPC, sync_to_code, template tag search, built-in scene templates |
| fd-asset | 19 | Asset CRUD, categorization/tagging/annotation, color extraction, Token binding |
| fd-codegen / fd-host-web / fd-export / fd-host-desk | 4-10 each | Core functionality |
| fd-cli | 5+ | CLI argument parsing, subcommand dispatch |

## 📄 License

Licensed under the [Apache License 2.0](LICENSE).
