---
doc_id: fusion-design/api-reference
product: fusion-design
class: platform
entry: developer
doc_type: api-reference
title: { zh: "fd-server HTTP API Contract (v0.1.25)", en: "API" }
audience: developer
status: draft
version: 0.1.26
updated: 2026-11-07
owner: "@dahai"
source_of_truth:
  - docs/API.md
---
# fd-server HTTP API Contract (v0.1.25)

Base URL: `http://127.0.0.1:11493` (loopback only, offline — no external traffic).
Content type: `application/json; charset=utf-8` for all `/api/*` endpoints.

## Conventions

### Error shape

Every 4xx validation error carries a **field-located** body (issue #35):

```json
{"error": "human-readable message", "field": "which field is missing/wrong"}
```

| Status | Meaning | Typical `field` values |
|--------|---------|------------------------|
| 400 | Request body invalid or a field failed validation | `body` (unparseable JSON), `prompt`, `instruction`, `image_base64`, `region`, `format`, `version`, `ds` |
| 404 | Referenced resource missing, or unknown path (any `/api/*` path not in this table) | `session`, `version`, `ds` |
| 405 | Method not allowed for the path | — (body adds `method`, `path`, and a `hint` for read-only endpoints, e.g. `GET /api/tokens?ds=<id>&theme=light|dark`) |
| 413 | Request body over the 8 MB cap | — |
| 502 | Upstream model failure (after retries) | — |

### Response envelope

Success responses are plain JSON objects; `document` fields are **JSON strings**
(serialized PenDocument, same shape as `.fusiondesign` files), not nested objects.

## Endpoints

### Health & meta

| Method/Path | Request | Response |
|-------------|---------|----------|
| `GET /api/health` | — | `{service, version, sessions, mlx}` |
| `GET /api/health?deep=1` | — | adds `mlx:{probed, available, model, chat_model, auth_ok, models[], status}` |
| `GET /api/design-systems` | — | `{systems[], active, theme}` |
| `GET /api/sessions` | — | `{sessions[]}` |
| `GET /api/tokens?ds=<id>&theme=light\|dark` | — | `{design_system, theme, css, tokens[{name,value}]}` — `css` is CSS custom properties; `tokens` is the name/value list for the token panel (dark theme = light base merged with dark overlay, v0.1.26). 404 `field:"ds"` if unknown. GET-only; other methods → 405 with hint |
| `GET /api/artifacts` | query: `tab=all\|mine\|shared`, `page`, `page_size≤100`, `sort`, `q` | artifacts page payload (fusion-artifacts-engine proxy) |
| `GET /api/artifact/<id>?version=<n>` | — | artifact detail |
| `GET /api/design-intel/search?q=<text>&kind=<k>&limit=<n>` | query: `q` (required, non-empty), `kind` (`styles`\|`palettes`\|`font_pairings`\|`ux_guidelines`\|`charts`\|`stacks`\|`all`, default `all`), `limit` (1-50, default 5) | `{kind, q, count, hits[{kind,id,title,detail,score}]}` — curated dataset search (issue #44), deterministic keyword scoring, zero model/network. 400 `field:"q"/"kind"` |
| `GET /api/session/<id>` | — | `{session, current_version, versions[{version,label,created_at,node_count}]}` |
| `POST /api/session/<id>/mutate` | `node_id` (non-empty) + at least one of `x`,`y`,`w`,`h`,`fill`,`stroke`,`radius`,`font_size`,`opacity` | — | `{session, current_version, document}` — direct node patch on the current version (canvas drag / token panel apply, no model round-trip); pushes a new version labeled `编辑：<node_id>`. 400 `field:"node_id"/"body"`, 404 `field:"session"/"node_id"` |
| `GET /api/version/<id>/<n>` | — | `{version, document}` |

### AI endpoints (POST, JSON body)

| Endpoint | Required fields | Optional fields | Response |
|----------|-----------------|-----------------|----------|
| `POST /api/generate` | `prompt` (non-empty string) | `page`, `model`, `design_system` | `{session, current_version, document}` — when the prompt matches the design-intelligence datasets, a `[Design intelligence]` context block (top style / palette / typography / ux guardrails / chart, issue #44) is appended server-side before inference |
| `POST /api/iterate` | `session`, `instruction` (non-empty) | `model`, `design_system`, `region: [node_id,…]` | `{session, current_version, document}` — with `region`, only selected nodes are sent to the model and the server merges results back; unlisted nodes are guaranteed unchanged; empty model output → 502 |
| `POST /api/restore` | `image_base64` (pure base64, **no** `data:` prefix) | `hint`, `page`, `model`, `session`, `design_system` | `{session, current_version, document}` — vision model, fixed default |
| `POST /api/rollback` | `session`, `version` | — | `{session, current_version, document}` — moves the version pointer; no new version is created |
| `POST /api/export` | `session`, `format` (`html`\|`react`) | `version` | `{filename, content, mime}` |
| `POST /api/send-to-code` | `session` | `version`, `component` | `{ok, component, files[]}` — HTML+TSX land in the fusion-code IPC dir |
| `POST /api/save-artifact` | `session` | `version`, `title`, `kind` | `{ok, artifact, engine_degraded, ipc_written}` |

## Contract tests

`crates/fd-server/src/handlers.rs` (`mod contract_tests`) and `src/router.rs`
(`method_not_allowed_contract`) pin the shapes above. Changing a contract
requires updating this document and those tests in the same commit.

## Workbench frontend (issue #35 minimum canvas UX)

- **Layer list** (right column): lists every node (`kind` + `name`); clicking an
  item selects the node on the canvas (`node.select` message → wasm highlight).
- **Canvas click**: the wasm bridge pushes `node.click`/`node.select` events to
  `navigator.__fd_host_post`; the workbench consumes them and highlights the
  corresponding layer item (no two-way echo loop).
- **Undo/Redo** (canvas header): steps the session version stack pointer
  (`rollback` to `current∓1`); buttons show disabled state at the stack edges.

## Workbench as shell module (issue #42)

- `web/manifest.json` (repo root, PRD §5.1 schema) registers the workbench as
  a union-web module (`id: fusion-design`, entry `/shell/fusion-design/workbench.html`,
  category `domain`); `start.sh` POSTs it to the gateway registry on start
  (best-effort — unreachable gateway logs and continues).
- `?embedded=1` hides the module's own navigation (back link) per PRD §3.3.
- Token panel (right column): token name/value pairs from the active design
  system; select a node (layers/canvas), click a token to apply (color → fill,
  Shift+click → stroke, `radius*` → radius, `font.size*` → font size).
  Constrained picking — no bare color picker.
- Canvas drag: wasm `node.drag` events are debounced (300 ms) and persisted via
  `POST /api/session/<id>/mutate` (absolute coords = node base + cumulative dx/dy).
- Save `.fusiondesign`: downloads the current PenDocument JSON (git-friendly).
- Auth: none bespoke. 401 responses are rendered inline as an error message —
  the module never redirects, so a module 401 cannot kick the shell (issue #28 contract).
