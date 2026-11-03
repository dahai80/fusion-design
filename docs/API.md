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
| 404 | Referenced resource missing | `session`, `version`, `ds` |
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
| `GET /api/tokens?ds=<id>&theme=light\|dark` | — | `{design_system, theme, css}` — `css` is CSS custom properties. 404 `field:"ds"` if unknown. GET-only; other methods → 405 with hint |
| `GET /api/artifacts` | query: `tab=all\|mine\|shared`, `page`, `page_size≤100`, `sort`, `q` | artifacts page payload (fusion-artifacts-engine proxy) |
| `GET /api/artifact/<id>?version=<n>` | — | artifact detail |
| `GET /api/session/<id>` | — | `{session, current_version, versions[{version,label,created_at,node_count}]}` |
| `GET /api/version/<id>/<n>` | — | `{version, document}` |

### AI endpoints (POST, JSON body)

| Endpoint | Required fields | Optional fields | Response |
|----------|-----------------|-----------------|----------|
| `POST /api/generate` | `prompt` (non-empty string) | `page`, `model`, `design_system` | `{session, current_version, document}` |
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
