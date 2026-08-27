# Fusion-Design Scenario Guide

> [English](USER_GUIDE.md) | [中文](USER_GUIDE_CN.md)
>
> For users: get started by scenario, troubleshoot by symptom (see [TROUBLESHOOTING.md](TROUBLESHOOTING.md)).

## Prerequisites

| Item | Requirement | Verify command |
|------|-------------|----------------|
| Platform | macOS Apple Silicon (M-series) | `uname -m` → `arm64` |
| fusion-mlx | Local inference service running (port 11434) | `~/claude-home/fusion-mlx/start.sh status` |
| fusion-gateway | Default via gateway 11432 (optional, direct also works) | `curl -s -m 5 http://127.0.0.1:11432/v1/models -H "Authorization: Bearer fg-admin-key" \| head -c 80` |
| CLI binary | `fusion-design` built | `fusion-design --version` |
| Auth key | env var `FUSION_MLX_API_KEY` set | `echo $FUSION_MLX_API_KEY` (non-empty, e.g. `fg-admin-key`) |

**Start/stop fusion-mlx**:

```bash
~/claude-home/fusion-mlx/start.sh start    # start (port 11434)
~/claude-home/fusion-mlx/start.sh stop     # stop
~/claude-home/fusion-mlx/start.sh status   # PID/port/memory/loaded models
~/claude-home/fusion-mlx/start.sh doctor   # health check
```

**Endpoint selection**: CLI defaults to fusion-gateway (11432). To direct-connect fusion-mlx, set `FUSION_MLX_BASE_URL=http://127.0.0.1:11434`. Priority: explicit `--endpoint` > `FUSION_MLX_BASE_URL` env > default gateway 11432.

**Model download**: when missing a model, use the mirror site, do not hit HuggingFace directly:

```bash
HF_MIRROR=https://hf-mirror.com huggingface-cli download <model-id>
```

## Scenario 1: First Text-to-UI (minimal closed loop)

**Goal**: one natural-language sentence → `.fusiondesign` canvas file.

**Precondition**: fusion-mlx running + `FUSION_MLX_API_KEY` set (see Prerequisites).

```bash
# 1. Verify MLX truly available (real inference probe, not just list)
fusion-design check-mlx --model Qwen3.5-9B-4bit

# 2. Text-to-UI
fusion-design generate --prompt "login page: email + password + remember me + login button" --out login.fusiondesign
```

**Expected output**: `login.fusiondesign` (JSON with Home page + node tree + styles). Terminal prints generation time and token count.

**Common stalls**:
- `check-mlx` returns non-zero → see [Symptom B/C/D](TROUBLESHOOTING.md)
- `generate` hangs >30s → model is loading, M-5 retry waits automatically, see [Symptom B](TROUBLESHOOTING.md)
- model name "not found" → pass an explicit loaded model id via `--model`, see [Symptom J](TROUBLESHOOTING.md)

**Tuning**: default model `Qwen3.5-9B-4bit`. For cross-deploy robustness, first probe a truly-available model id with `check-mlx --endpoint http://127.0.0.1:11434`, then pass it to `--model`.

## Scenario 2: Export Deliverables (PNG/SVG/PDF/HTML/React)

**Goal**: `.fusiondesign` → deliverable images/docs/code.

**Format selection**:

| Need | Command | Output |
|------|---------|--------|
| Screenshot for docs/review | `export --format png` | single-page PNG |
| Lossless vector delivery | `export --format svg` | single-page SVG |
| Print / PDF archive | `export --format pdf` | single-page PDF |
| Static previewable | `export --format html` | self-contained HTML |
| Data backup | `export --format json` | raw JSON |

```bash
# Single-page export (--out is the output dir; artifact filename = page name, e.g. Home.png)
fusion-design export --input login.fusiondesign --format png --out ./out
fusion-design export --input login.fusiondesign --format svg --out ./out
fusion-design export --input login.fusiondesign --format html --out ./out

# Batch multi-page (input JSON array, each item has input/format/out; out is a dir)
echo '[{"input":"a.fusiondesign","format":"png","out":"./out/a"},
       {"input":"b.fusiondesign","format":"svg","out":"./out/b"}]' > batch.json
fusion-design export-batch --input batch.json --out ./out
```

**Note**: `--out` is a **directory**, not a filename; artifacts are named by page (e.g. canvas page `Home` → `Home.png`). A multi-page canvas exports multiple files. To control the filename, rename the page in the canvas (`--page` arg).

**Common stalls**:
- `var(--color-x)` unresolved in output → see [Symptom H](TROUBLESHOOTING.md)
- `--format` reports illegal value → only `html svg json png pdf` supported (see table above)

## Scenario 3: Design Lint + Auto-fix

**Goal**: check canvas against design system (13 rules), auto-fix what's fixable.

**13 rules**: `contrast-check`, `unlabeled-input`, `text-effects`, `abnormal-rotation`, `empty-effects`, `token-inconsistency`, `unnamed-node`, `text-overflow`, `overlapping-nodes`, `hardcoded-spacing`, `hardcoded-font-size`, `missing-interaction-state`, `layout-inconsistency` (meanings in [Symptom K](TROUBLESHOOTING.md)).

```bash
# Full check (default apple-hig system)
fusion-design lint --input login.fusiondesign

# Specify system + rule subset
fusion-design lint --input login.fusiondesign --design-system apple-hig \
  --rules contrast-check,unlabeled-input,token-inconsistency

# Preview auto-fix only (no write)
fusion-design lint --input login.fusiondesign --fix --dry-run

# Apply auto-fix (Token refs / null cleanup / auto-naming)
fusion-design lint --input login.fusiondesign --fix
```

**System options**: `apple-hig` (default), `minimal-dashboard`, `robot-sim`. `fusion-design list-design-systems` lists all, `fusion-design activate <id>` switches the active system.

**Common stalls**: lint report fields unclear → see [Symptom K](TROUBLESHOOTING.md).

## Scenario 4: Sketch / Reference Image to UI (image-to-ui)

**Goal**: hand-drawn sketch or reference screenshot → `.fusiondesign`.

```bash
# Basic: sketch → UI
fusion-design image-to-ui --sketch ./sketch.png --out from-sketch.fusiondesign

# With text hint to guide style
fusion-design image-to-ui --sketch ./ref.png --hint "minimal dashboard, dark theme" \
  --out dashboard.fusiondesign
```

**Params**: `--sketch` (image path, required), `--hint` (style/scene hint, optional), `--page` (page name, default Home), `--model`, `--endpoint`, `--out`.

**Precondition**: the model must support multimodal vision input. The default `Qwen3.5-9B-4bit` text model may not accept images — switch to a multimodal model (probe a truly-available multimodal id with `check-mlx`, then pass via `--model`). See [Symptom J](TROUBLESHOOTING.md).

## Scenario 5: Multi-variant Style Comparison

**Goal**: generate 3 different style variants of the same request in one run.

```bash
# Default three styles
fusion-design multi-variants --prompt "ecommerce home: search bar + carousel + product grid" \
  --out ./out/ecom

# Custom three styles (comma-separated)
fusion-design multi-variants --prompt "login page" \
  --styles "minimal white,dark tech,skeuomorphic cards" --out ./out/login-variants
```

**Params**: `--prompt` (required), `--styles` (three styles comma-separated, defaults to built-in three), `--page`, `--model`, `--endpoint`, `--out`.

**Output**: 3 `.fusiondesign` files under `./out/`, export each to images for comparison.

## Scenario 6: Batch Multi-page + Spec Doc

**Goal A**: generate multiple pages from a flow description (unified style).

```bash
fusion-design page-flow --flow "home -> product list -> detail -> cart -> checkout" \
  --style-hint "minimal ecommerce, primary blue" --out ./out/flow
```

Output: one `.fusiondesign` per page under `./out/flow`, unified style.

**Goal B**: generate a design spec doc from an existing canvas (interaction/component/page-architecture spec).

```bash
fusion-design spec-doc --input login.fusiondesign --title "Login module design spec" \
  --out ./out/login-spec.md
```

**Params (page-flow)**: `--flow` (flow description, required), `--style-hint`, `--model`, `--endpoint`, `--out`.
**Params (spec-doc)**: `--input` (`.fusiondesign`, required), `--title` (doc title, default "设计规范文档"), `--model`, `--endpoint`, `--out`.

## Scenario 7: Design → Runnable Code (Codegen + Code sync)

**Goal**: `.fusiondesign` → runnable frontend code (HTML / React+Tailwind / Tailwind-only / Swift UI).

```bash
# HTML (default)
fusion-design codegen --input login.fusiondesign --target html --out ./out/Login.html

# React + Tailwind
fusion-design codegen --input login.fusiondesign --target react-tailwind \
  --component LoginForm --out ./out/LoginForm.tsx

# Tailwind class names only
fusion-design codegen --input login.fusiondesign --target tailwind-only --out ./out/login.txt

# Swift UI (macOS native)
fusion-design codegen --input login.fusiondesign --target swift-ui \
  --component LoginView --out ./out/LoginView.swift
```

**Params**: `--input` (required), `--target` (`html` default / `react-tailwind` / `tailwind-only` / `swift-ui`), `--component` (component name, default MyComponent), `--out`.

**Fusion Code sync**: after export, push to a Fusion Code project dir via fd-ecosystem IPC with `--ipc-base` (file-based IPC, no network):

```bash
fusion-design codegen --input login.fusiondesign --target react-tailwind \
  --out ./out/LoginForm.tsx --ipc-base ~/.fusion/ipc
```

**Security**: codegen HTML-escapes content (XSS protection), sanitizes CSS injection. Generated code is local read/write only, no network request injection. See [Symptom L](TROUBLESHOOTING.md).

## Scenario 8: CLI Pipeline Streaming Inference (script integration)

**Goal**: machine-readable streaming chat for CLI pipelines / scripts / automation (NDJSON framing, not for human reading).

**Contract**: one JSON object per line, `type` field has three states — `delta` (incremental token), `chat_done` (end, with `finish_reason`), `error`. Stream terminates after the final `chat_done` line.

```bash
# Prepare messages file (JSON array, each item role+content)
echo '[{"role":"user","content":"describe login page design in one sentence"}]' > /tmp/msgs.json

# Stream + JSON framing
fusion-design chat --model Qwen3.5-9B-4bit \
  --system-prompt "you are a UI design advisor, answer concisely" \
  --messages-file /tmp/msgs.json --stream --json
```

**Expected output** (line by line):
```
{"token":"Login","type":"delta"}
{"token":" page","type":"delta"}
{"finish_reason":"stop","type":"chat_done"}
```

**Multi-turn history**: pass the full conversation array via `--messages-file` (including prior assistant messages). **RAG injection**: pass retrieved context text file via `--rag-context-file`, appended to the prompt.

**Script pipeline consumption** (jq to extract tokens line by line):

```bash
fusion-design chat --messages-file /tmp/msgs.json --stream --json \
  | while read line; do
      echo "$line" | jq -r 'select(.type=="delta") | .token' 2>/dev/null
    done
```

**Important**: this subcommand's NDJSON schema (`delta`/`chat_done`/`error`) is the CLI's self-consistent contract, **for CLI pipeline/script/test consumption**. fusion-studio actually goes through fusion-gateway TCP NDJSON (frame schema `chat_event`/`chat_done`/`error`), **not via fd-cli chat**, so this subcommand has no studio consumer (issue #17 verified).

**Common stalls**:
- `invalid type: map, expected a sequence` → `--messages-file` needs an array `[{...}]`, not a single object `{...}`
- CJK streaming garbled `U+FFFD` → see [Symptom E](TROUBLESHOOTING.md)
- streaming via gateway occasional 502 → fixed (fusion-gateway#108, PR #111, 2026-08-25); on upstream regression, direct-connect per [Symptom B](TROUBLESHOOTING.md)

## Scenario 9: Design System Switch + Token CSS

**Goal**: switch built-in design systems, export Token CSS for frontend consumption.

```bash
# List all systems
fusion-design list-design-systems

# Activate one (affects subsequent lint / token-css / theme defaults)
fusion-design activate robot-sim

# Output CSS Custom Properties of the active system (:root vars)
fusion-design token-css > tokens.css

# Output CSS vars for a specific theme (light/dark)
fusion-design theme --mode dark > tokens-dark.css
```

**Three built-in systems**:
- `apple-hig` — Apple Human Interface Guidelines, default
- `minimal-dashboard` — minimal admin dashboard
- `robot-sim` — robot simulation console

**Usage**: `<link>` or `@import` `tokens.css` in frontend; components reference `var(--color-primary)` etc. Switching systems means swapping `tokens.css` only — global Token sync across the whole site.

## Appendix: 22 Subcommand Quick Reference

| Subcommand | Purpose | Required args | See scenario |
|------------|---------|---------------|--------------|
| `list-design-systems` | List registered design systems | — | Scenario 9 |
| `activate` | Activate a design system | `<id>` | Scenario 9 |
| `generate` | Text-to-UI (NL → canvas) | `--prompt` | Scenario 1 |
| `image-to-ui` | Image-to-UI (sketch/ref → canvas) | `--sketch` | Scenario 4 |
| `multi-variants` | Multi-variant comparison (3 styles) | `--prompt` | Scenario 5 |
| `page-flow` | Flow description → batch multi-page | `--flow` | Scenario 6 |
| `spec-doc` | AI-generate design spec doc | `--input` | Scenario 6 |
| `lint` | Design lint + auto-fix | `--input` | Scenario 3 |
| `codegen` | Canvas → frontend code (4 targets) | `--input` | Scenario 7 |
| `export` | Single-page export (png/svg/pdf/html/json) | `--input --format --out` | Scenario 2 |
| `export-batch` | Batch export (JSON array input) | `--input --out` | Scenario 2 |
| `parse-html` | HTML → PenDocument JSON | `--input` | — |
| `token-css` | Output active system CSS vars | — | Scenario 9 |
| `theme` | Output a theme's CSS vars | `--mode` | Scenario 9 |
| `chat` | Machine-readable streaming chat (NDJSON) | — | Scenario 8 |
| `check-mlx` | Verify MLX endpoint truly available | — | Prereq / Scenario 1 |
| `health` | Probe MLX health | — | Prereq |
| `undo` | Undo (return previous snapshot) | `--input` | — |
| `redo` | Redo (return next snapshot) | `--input` | — |
| `diff` | Compare two canvases | `--input` | — |
| `check-frontend` | Validate frontend static asset dir | `--input` | — |
| `train` | Fine-tune model on design corpus (calls fusion-trainer) | — | — |

**Common args**: AI subcommands (`generate`/`image-to-ui`/`multi-variants`/`page-flow`/`spec-doc`/`chat`) all support `--model` (default `Qwen3.5-9B-4bit`) and `--endpoint` (default gateway 11432, env-overridable).

**On error**: any subcommand failure — first read the terminal diagnostic (fail visibly, with cause + suggestion), then check the [Troubleshooting manual](TROUBLESHOOTING.md). Set `RUST_LOG=debug` for detailed logs.
