---
doc_id: fusion-design/desigma-token-mapping
product: fusion-design
class: platform
entry: developer
doc_type: guide
title: { zh: "desigma 变量到 Fusion 语义 Token 映射", en: "Desigma Variables to Fusion Semantic Tokens Mapping" }
audience: developer
status: draft
version: 0.1.26
updated: 2026-11-10
owner: "@dahai"
source_of_truth:
  - docs/DESIGMA_TOKEN_MAPPING.md
---

# Desigma Variables to Fusion Semantic Tokens Mapping (issue #41)

Customer designs authored in fusion-desigma (variable collection `Theme` with
Light/Dark modes) enter the fusion-design pipeline as React/Tailwind exports
from Dev Mode. This document maps desigma variable groups to fusion semantic
tokens and defines the acceptance gate those exports must pass before entering
the shell design language.

## Gate pipeline

```
desigma Dev Mode export (React/Tailwind → HTML)
  → fd-cli parse-html --input page.html --out page.fusiondesign
  → fd-cli lint --input page.fusiondesign --design-system desigma-aligned --profile desigma --gate
      exit 0 = aligned, may enter pipeline
      exit 1 = Error violations (bare color / off-token spacing / off-token radius / missing dark pair)
      exit 2 = runtime failure (unreadable file, bad JSON)
```

The gate is consumed by the fusion-cowork desigma pipeline as a DAG step
(non-zero exit fails the node). The violations JSON on stdout carries, per
violation: `rule`, `node_id`, `message`, `suggestion`, `severity`.

## Recommended preset: `desigma-aligned`

Shipped as a built-in design system (registered by
`DesignSystemRegistry::register_builtin`, id `desigma-aligned`). Every color
token ships a dark pair, so Light/Dark variable modes map 1:1 to the shell
theme and the `missing_dark_pair` gate rule passes out of the box.

| Desigma `Theme` group | Fusion token(s) | Light | Dark |
|-----------------------|-----------------|-------|------|
| Surface               | `color.bg`      | `#FFFFFF` | `#0F1115` |
| Surface               | `color.surface` | `#F5F6F8` | `#161A22` |
| Text                  | `color.fg`      | `#1C1E21` | `#E8EAF0` |
| Text (secondary)      | `color.fg.muted`| `#65676D` | `#9AA0AB` |
| Border                | `color.border`  | `#E3E5E9` | `#2A2F3A` |
| Brand                 | `color.accent`  | `#4F7CFF` | `#6D93FF` |
| Status (positive)     | `color.success` | `#34C759` | `#30D158` |
| Status (error)        | `color.error`   | `#FF453A` | `#FF6961` |
| Number (radius)       | `radius.sm` / `radius.md` / `radius.lg` | 4 / 8 / 16 px | same |
| Number (spacing)      | `spacing.sm` / `spacing.md` / `spacing.lg` | 8 / 16 / 24 px | same |
| Number (font)         | `font.size.body` | 16 px | same |

`color.accent` light value `#4F7CFF` is the shell brand color (same value the
web shell workbench styles mirror via `--fs-brand`).

## Gate rules (profile `desigma`)

All four rules are `Error` severity — the gate is binary. Legacy default
profile rules (contrast, overflow, …) stay available via `--profile default`
or explicit `--rules`.

| Rule | Rejects | Passes |
|------|---------|--------|
| `bare_color_literal` | fill/stroke that is a raw hex/rgb/named color matching no token, without a `design_token_refs` binding | token-bound fills/strokes; literals equal to a token value |
| `off_token_spacing`  | flex gap / padding / grid gap values not on the spacing token scale (and not token-bound) | values on the scale (8/16/24 by preset); token-bound values |
| `off_token_radius`   | `style.radius` values not on the radius token scale and not token-bound | values on the scale; token-bound radii |
| `missing_dark_pair`  | a fill/stroke matching a light token with no same-name token in `dark_tokens` | tokens present in both overlays |

Both scale rules require `--design-system` (the scale comes from the design
system's Number tokens). Without a design system the desigma profile only runs
`bare_color_literal`'s token-match path — effectively no-op — so always pass
`--design-system desigma-aligned` for gate runs.

## Extending the mapping

Custom customer palettes: export the design system as JSON
(`fd-cli` / `GET /api/tokens?ds=<id>` for CSS vars), keep the semantic names
above, and either `extends: desigma-aligned` with overrides or a full token
set loaded via `DesignSystemRegistry::import_json`. The gate rules read
whatever system is active — no code change needed for a new palette, as long
as every color token has a dark counterpart.
