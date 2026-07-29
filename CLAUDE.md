# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Fusion-Design is a **local offline AI visual design workbench** for macOS Apple Silicon. It's part of the Fusion-MLX "one core, nine endpoints" ecosystem. Built on top of OpenPencil (Rust) with fusion-mlx local multimodal inference, embedded in Fusion-Desk WKWebView.

**Hard constraint**: 100% offline — no cloud API calls, no external network requests. The only HTTP traffic goes to `127.0.0.1` (fusion-mlx local inference service).

## Build & Test Commands

```bash
cargo check --workspace          # Compile check all crates
cargo test --workspace           # Run all tests (125+ tests)
cargo test -p fd-ai-adapter      # Run tests for a specific crate
cargo test -p fd-ai-adapter --test integration   # Run specific test file
cargo build --workspace          # Full build
cargo build -p fd-host-web --target wasm32-unknown-unknown  # Build wasm for WKWebView
cargo run -p fd-cli -- --help    # Run CLI
cargo run -p fd-cli -- generate --prompt "login page"   # CLI text-to-UI
cargo run -p fd-cli -- export --input doc.json --format html --out ./out  # CLI export
cargo run -p fd-cli -- export-batch --input batch.json --format svg --out ./out  # Batch export
```

No clippy/fmt CI is configured yet. Rust toolchain: `1.94` (see `rust-toolchain.toml`), with `wasm32-unknown-unknown` target.

## Architecture

### Layered Model

```
Fusion-Desk (macOS SwiftUI)
  ↓
WKWebView (loads fd-host-web wasm)
  ↓
Frontend: fd-host-web (wasm) — canvas rendering, AI chat panel, message bridge
  ↓
Backend crates (local HTTP on 127.0.0.1):
  fd-ai-adapter → fusion-mlx inference
  fd-codegen    → HTML/React/Tailwind export
  fd-export     → PNG/SVG/PDF/HTML file export
  fd-design-system → Token management, 3 built-in design specs
  fd-ecosystem  → IPC with Fusion Code/Simulation/KB/CLI
  fd-host-desk  → WKWebView ↔ backend message bridge
  fd-canvas-core → Lightweight canvas data model (PenDocument/PenNode)
  fd-cli        → Command-line interface (clap)
  ↓
fusion-mlx (local multimodal inference, Metal/ANE accelerated)
```

### Crate Dependency Graph

- `fd-canvas-core` — **leaf**, no fd-* deps. Own `PenDocument`/`PenNode`/`Page`/`NodeStyle` data model, JSON serialization for `.fusiondesign` files.
- `fd-design-system` — **leaf**, no fd-* deps. `DesignSystem`/`Token`/`DesignSystemRegistry` with 3 presets (Apple HIG, minimalist admin, robot sim).
- `fd-ai-adapter` — depends on `op-ai` (vendored). Implements OpenPencil's `ChatProvider` trait → calls fusion-mlx via OpenAI-compatible API at `127.0.0.1:8080`. **Only crate allowed to make HTTP requests**.
- `fd-codegen` — depends on `fd-canvas-core`, `fd-design-system`. Consumes `PenDocument` → generates HTML/React+Tailwind code.
- `fd-export` — depends on `fd-canvas-core`. Exports to HTML/SVG/JSON (PNG/PDF marked `NotImplemented` in MVP).
- `fd-ecosystem` — depends on `fd-canvas-core`. Local file-based IPC (JSON messages in约定 directories) for Fusion Code/Simulation/KB/CLI.
- `fd-host-desk` — no fd-* deps. `HostMessage`/`HostBridgeConfig` for WKWebView↔backend bridge.
- `fd-host-web` — depends on `fd-canvas-core`. `wasm_bindgen` entry point (`WebShell::mount()`), renders `PenDocument` → DOM/Canvas, bridges `window.webkit.messageHandlers`.
- `fd-cli` — depends on all other fd-* crates. `clap` subcommands: `list-design-systems`, `activate`, `export`, `export-batch`, `generate`.

### Key Design Decisions

1. **Self-built canvas model (`fd-canvas-core`)** instead of using OpenPencil's `op-editor-core` — the latter depends on a private `jian-ops-schema` crate that isn't available in the vendored subset. `PenDocument` is a lightweight replacement covering MVP needs.
2. **Vendored OpenPencil** — only `op-ai` is currently included in `vendor/openpencil/`. Other op-* crates (editor-core, codegen, mcp, etc.) are listed as future integrations but not yet pulled in.
3. **OpenAI-compatible API** — fusion-mlx exposes `/v1/chat/completions`, so `fd-ai-adapter` uses the same shape as OpenAI's API for the request/response types.
4. **Local file IPC** — `fd-ecosystem` communicates with other Fusion tools via JSON files in约定 directories, not network calls.

## Code Style

- Rust edition 2021, minimum Rust 1.87
- Indentation: **4 spaces** (multiples of 4)
- No docstrings on functions — code comments (especially `//!` module docs) explain intent
- Logging: use `tracing` crate (`tracing::info!`, `tracing::error!`, etc.) for all runtime logging
- Error handling: `anyhow` for applications, `thiserror` for library crate error types
- Serialization: `serde` + `serde_json` throughout
- Async: `tokio` runtime

## File Format

`.fusiondesign` files are JSON — `PenDocument` serialized via serde. They contain pages → nodes with styles, supporting Git version control.

## Fusion-MLX Integration

- fusion-mlx runs as a local HTTP service on `127.0.0.1:8080` (configurable)
- Start/stop: `~/claude-home/fusion-mlx/start.sh start|stop`
- Model downloads: use mirror `https://hf-mirror.com`
- AI tests requiring real model inference must actually load the model (no mocks for integration tests)
