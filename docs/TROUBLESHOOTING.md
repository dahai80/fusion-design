# Fusion-Design Scenario Troubleshooting

> [English](TROUBLESHOOTING.md) | [中文](TROUBLESHOOTING_CN.md)
>
> Quick lookup by symptom: symptom → root cause → fix. Companion to [USER_GUIDE.md](USER_GUIDE.md).

## Quick Index

| Symptom | Keywords | Jump |
|---------|----------|------|
| A | connection failed / health failed / not running | [Symptom A](#symptom-a-mlx-service-unreachable-connection-failed--health-check-failed) |
| B | 502 / 503 / model loading / hang | [Symptom B](#symptom-b-generate-returns-502--503-model-loading) |
| C | false-green / listed but generate fails | [Symptom C](#symptom-c-check-mlx-false-green-model-listed-but-generate-fails) |
| D | 401 / API key / auth | [Symptom D](#symptom-d-auth-failed-401--invalid-api-key) |
| E | stream break / garbled / U+FFFD | [Symptom E](#symptom-e-stream-breaks-mid-way--cjk-garbled-ufffd) |
| F | deserialize / too deep / over limit | [Symptom F](#symptom-f-fusiondesign-deserialize-error-too-deep--over-limit) |
| G | IPC / message lost / path traversal | [Symptom G](#symptom-g-ipc-message-lost--path-traversal-blocked) |
| H | var(--*) unresolved / Token color | [Symptom H](#symptom-h-export-token-color-unresolved-var--in-output) |
| I | wasm / frontend load / studio stale | [Symptom I](#symptom-i-wasm-frontend-load-failed--studio-sync-stale) |
| J | model not found / cross-deploy drift | [Symptom J](#symptom-j-model-not-found--cross-deploy-model-list-drift) |
| K | lint report / 13 rule meanings | [Symptom K](#symptom-k-lint-report-unclear-13-rule-meanings) |
| L | XSS / injection / security guardrail | [Symptom L](#symptom-l-generated-content-contains-xss--injection-security-guardrail-triggered) |
| M | no log file / field diagnostic / file log | [Symptom M](#symptom-m-no-log-file-on-disk--need-field-diagnostic-artifact) |

## Symptom A: MLX service unreachable (connection failed / health check failed)

**Symptom**: `fusion-design health` or `check-mlx` reports "connection failed / connection refused / service not running".

**Root cause**: fusion-mlx (11434) or fusion-gateway (11432) not running, or port occupied.

**Diagnose**:

```bash
# 1. Check fusion-mlx status
~/claude-home/fusion-mlx/start.sh status

# 2. Check ports
lsof -i :11434    # fusion-mlx
lsof -i :11432    # fusion-gateway

# 3. Direct-connect probe
curl -s -m 5 http://127.0.0.1:11434/v1/models -H "Authorization: Bearer $FUSION_MLX_API_KEY" | head -c 100
```

**Fix**:

```bash
# fusion-mlx not running
~/claude-home/fusion-mlx/start.sh start
~/claude-home/fusion-mlx/start.sh doctor   # health check after start

# gateway not running but MLX up -> CLI direct-connect
export FUSION_MLX_BASE_URL=http://127.0.0.1:11434
fusion-design check-mlx
```

**Prevention**: run `start.sh status` before use; add a `health` precheck in scripts, stop on non-zero.

## Symptom B: Generate returns 502 / 503 (model loading)

**Symptom**: `generate` / `chat` returns 502 or 503, or the first request hangs 30s+.

**Root cause**: fusion-mlx takes time to load a large model on first use, returning 503 ("model loading") during load; returns 502 after a model is evicted. These are transient errors, not permanent failures.

**Mechanism**: fd-ai-adapter **M-5 retry backoff** is built in — three HTTP paths (`blocking_post` / `chat_stream_messages` / `check_generate`) retry 502/503 with exponential backoff (500ms→1s→2s→4s→8s capped, default 4 attempts), succeeding once the model finishes loading. 4xx (auth/request format) fails immediately without retry.

**Fix**:
- **Default behavior**: no action needed; the CLI retries automatically, be patient (up to ~3.5s backoff + inference time).
- **Disable retry** (when debugging and want the error immediately): `export FUSION_MLX_RETRY_MAX=1`.
- **Increase retry** (large/slow-loading model): `export FUSION_MLX_RETRY_MAX=8`.

**Verify**: the log shows `blocking_post: transient error, backing off retry attempt=0 code=503` to confirm retry is active. Set `RUST_LOG=info` to see it.

**Note**: streaming (`chat --stream`) retry covers the **connection-establishment phase** only; mid-stream breaks after the stream is established are not retried (complex semantics). Connection 502 via gateway is fixed (see Symptom E upstream note).

## Symptom C: check-mlx false-green (model listed but generate fails)

**Symptom**: `curl /v1/models` returns a long list of model names, but `generate` reports 502 / model not loaded.

**Root cause**: fusion-gateway's `/v1/models` "false-greens" — it lists all cloud + local model names, but MLX has not actually loaded that model. Judging by the list alone misleads.

**Mechanism**: `fusion-design check-mlx` does a three-stage real probe to break the false-green: endpoint resolution → `/v1/models` auth + list → **1-token real inference probe**. The final verdict uses a real chat call, not the list.

**Fix**:
- **Use check-mlx, not curl list**, to judge availability:
  ```bash
  fusion-design check-mlx --model Qwen3.5-9B-4bit
  ```
- **Pass an explicit local mlx model id** (resolution priority: `--model` > `FUSION_MLX_MODEL` env > list first). The list's first entry may be an unloaded cloud model, so pass it explicitly.
- Probe returns `model_loaded: false` → the model genuinely is not loaded; load it on the MLX side (`start.sh` + download the model).

**Verify**: `check-mlx` success = all three stages pass + real inference returns 1 token. A non-zero exit code + diagnostic text means fail visibly.

## Symptom D: Auth failed (401 / invalid API key)

**Symptom**: returns 401 / "Unauthorized" / "invalid api key".

**Root cause**: `FUSION_MLX_API_KEY` unset / wrong / inconsistent with the key configured on gateway/MLX. fd-ai-adapter authenticates via `Authorization: Bearer <key>`.

**Diagnose**:

```bash
echo "env key: $FUSION_MLX_API_KEY"
# Direct MLX key check
curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:11434/v1/models \
  -H "Authorization: Bearer $FUSION_MLX_API_KEY"
# Gateway key check
curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:11432/v1/models \
  -H "Authorization: Bearer $FUSION_MLX_API_KEY"
```

**Fix**:
```bash
export FUSION_MLX_API_KEY=fg-admin-key   # replace with the actually configured key
```

**Mechanism**: 401 is a 4xx permanent error; M-5 retry **does not retry**, it fails immediately (retrying only wastes time). A key mismatch bails at once, with an auth-failure diagnostic in the log.

## Symptom E: Stream breaks mid-way / CJK garbled (U+FFFD)

**Symptom**: in `chat --stream` output, Chinese characters appear as `U+FFFD` (replacement character / garbled diamond), or the stream breaks mid-way.

**Root cause (garble)**: early SSE streaming read fixed byte chunks; CJK characters (3-byte UTF-8) split across chunks caused half-character decode failures. **Fixed** (L6 fix): byte buffering — incomplete UTF-8 across chunks is held until the next chunk completes it, no U+FFFD.

**Root cause (mid-stream break)**: network jitter / MLX restart after the stream is established. M-5 stream retry covers the **connection-establishment phase** only; mid-stream breaks are not retried (complex semantics, not covered this round).

**Fix**:
- **Garble**: upgrade to v0.1.12+ (contains the L6 byte-buffer fix); no such issue. If it recurs → file an issue (attach model + prompt + full stream log).
- **Mid-stream break**: rerun the command (M-5 retries on the next connection). If breaks are frequent → check MLX stability (`start.sh status` for memory/process).
- **gateway streaming 502** (connection phase): fusion-gateway#108 fixed (PR #111, 2026-08-25, local-first ordering). On upstream regression with 502 recurring, temporarily direct-connect:
  ```bash
  export FUSION_MLX_BASE_URL=http://127.0.0.1:11434
  fusion-design chat --stream --json ...
  ```

## Symptom F: .fusiondesign deserialize error (too deep / over limit)

**Symptom**: `export` / `lint` / `codegen` etc. reading `.fusiondesign` reports "nesting over limit / total nodes over limit / deserialize failed".

**Root cause**: security guardrail triggered. `.fusiondesign` deserialization limits node nesting depth ≤64 and total node count ≤100000, preventing stack overflow / OOM from malicious input.

**Mechanism**: `validate_limits` enters the deserialization boundary; over-limit is rejected outright (no partial load), failing visibly with a diagnostic.

**Fix**:
- **Normal file triggers it**: the file is genuinely too large (e.g. AI-generated abnormal bloat). Export the raw JSON with `export --format json` to inspect node count, trim manually.
- **Malicious / corrupt file**: the guardrail is correct to reject it. Restore a prior version from Git history (`.fusiondesign` is JSON, supports Git version control).
- **Self-constructed file over limit**: split into multiple pages (`--page`) or multiple files; keep a single file under 100k nodes.

## Symptom G: IPC message lost / path traversal blocked

**Symptom**: after pushing via `--ipc-base` through fd-ecosystem, the peer does not receive it; or it reports "path traversal blocked / illegal path".

**Root cause (path traversal)**: the IPC message file path contains `..` / an absolute path escaping bounds, blocked by path-traversal protection (prevents escaping the IPC directory).

**Root cause (message lost)**: the old version silently swallowed errors and dropped messages on parse failure. **Fixed** (P2-3/R-A2): parse failure **keeps the file, no silent swallow**, logs a warning, recoverable manually.

**Fix**:
- **Path traversal**: use only flat names for IPC file names (e.g. `login.fusiondesign`), no `../` or absolute paths. See fd-ecosystem docs for the peer directory convention.
- **Message lost**: upgrade to v0.1.12+ (contains non-destructive consume). Check the IPC directory for residual files + log warnings to locate. File-size guardrail ≤8MB, over-limit rejected.
- **Peer not running**: fd-ecosystem is file-based IPC; the peer (Fusion Code etc.) must watch the agreed ipc_base directory. Confirm the peer service is running.

## Symptom H: Export Token color unresolved (var(--*) in output)

**Symptom**: in `export --format png/svg/pdf` output, colors are still `var(--color-primary)` text rather than hex values; image colors look off.

**Root cause**: node styles use Token references (`var(--color-primary)` or `token:color-primary`), but they were not resolved to hex at export. **Fixed** (#8): before rasterization, `var(--*)` / `token:*` are resolved to hex.

**Fix**:
- **Still unresolved**: upgrade to v0.1.12+ (contains the #8 resolution). Confirm the active system defines that Token (`fusion-design token-css` to check); a missing Token definition cannot be resolved → add the Token or switch systems.
- **PNG/SVG color wrong**: Token resolution depends on the active system. Before export, `fusion-design activate <correct system>` to ensure the Token table is complete.

**Verify**: the variable table output by `fusion-design token-css` is the basis for export resolution. A Token name mismatch means resolution fails.

## Symptom I: wasm frontend load failed / studio sync stale

**Symptom**: Fusion-Desk WKWebView loads a blank canvas / reports a wasm error; fusion-studio pulls stale wasm.

**Root cause**: wasm artifacts (`fd_host_web_bg.wasm` + `fd_host_web.js`) missing, or the studio sync script pulls the wrong directory.

**Artifact chain**:
```bash
# 1. Build raw wasm (must use rustup cargo; homebrew cargo has no wasm target)
~/.cargo/bin/cargo build -p fd-host-web --target wasm32-unknown-unknown
# -> target/wasm32-unknown-unknown/debug/fd_host_web.wasm (14MB raw)

# 2. wasm-bindgen post-step (emit _bg.wasm + .js)
~/.cargo/bin/wasm-bindgen --target web \
  --out-dir target/wasm32-unknown-unknown/debug \
  target/wasm32-unknown-unknown/debug/fd_host_web.wasm
# -> fd_host_web_bg.wasm (1.5MB) + fd_host_web.js (27KB)
```

**Fix**:
- **wasm load error**: confirm step 2 (bindgen post-step) ran; without it only raw wasm exists, unusable by WKWebView. `build.sh` includes this step.
- **studio stale**: fusion-studio `Scripts/build.sh` pulls from this repo's `target/wasm32-unknown-unknown/{release,debug}/`. After changing wasm, rerun bindgen to land in that directory, or studio pulls the old artifact.
- **can't find crate for std**: homebrew cargo (`/opt/homebrew/bin/cargo`) has no wasm target. Use `~/.cargo/bin/cargo` (rustup proxy, toolchain 1.94 includes wasm32-unknown-unknown).

**Verify**: `ls -la target/wasm32-unknown-unknown/debug/fd_host_web_bg.wasm` exists and is recent.

## Symptom J: Model not found / cross-deploy model list drift

**Symptom**: `--model <id>` reports model not found; the default model errors on a different machine.

**Root cause**: the default model `Qwen3.5-9B-4bit` is a built-in common MLX text model (real-inference verified), but **the deployed MLX model list varies by environment** — different machines have different downloaded models, and gateway mixes cloud/local names.

**Fix (cross-deploy robust approach)**:

```bash
# 1. Probe a truly-available model id on this machine (real inference probe, breaks false-green)
fusion-design check-mlx --endpoint http://127.0.0.1:11434
# Pick an id with model_loaded: true from the output

# 2. Pass that id explicitly
fusion-design generate --model <probed-id> --prompt "..."
```

**Pin via env** (reuse across commands):
```bash
export FUSION_MLX_MODEL=<truly-available-id>
fusion-design generate --prompt "..."        # no --model -> uses env
```

**Note**: priority is `--model` > `FUSION_MLX_MODEL` env > list first. The list's first entry may be an unloaded cloud model (false-green); do not rely on it.

**Model download**: when missing a model, use the mirror site, do not hit HuggingFace directly:
```bash
HF_MIRROR=https://hf-mirror.com huggingface-cli download <model-id>
```

## Symptom K: lint report unclear (13 rule meanings)

**Symptom**: `lint` outputs a bunch of rule names + violation locations; unclear what each means or how to fix.

**13 rule meanings + fixability**:

| Rule | Detects | Auto-fixable |
|------|---------|--------------|
| `contrast-check` | insufficient text/background contrast (poor readability) | No (manual color tweak) |
| `unlabeled-input` | input field without label/placeholder | No |
| `text-effects` | abnormal text effects (e.g. all-caps rotation) | No |
| `abnormal-rotation` | node with abnormal rotation angle | No |
| `empty-effects` | empty effect node (exists but no visual effect) | Yes (`--fix` cleanup) |
| `token-inconsistency` | color/font not using Token (hardcoded) | Yes (`--fix` to Token ref) |
| `unnamed-node` | node not named (default name) | Yes (`--fix` auto-name) |
| `text-overflow` | text overflows container | No |
| `overlapping-nodes` | nodes overlap each other | No |
| `hardcoded-spacing` | spacing hardcoded, not Token | No |
| `hardcoded-font-size` | font-size hardcoded, not Token | No |
| `missing-interaction-state` | interactive element lacks hover/active/disabled | No |
| `layout-inconsistency` | inconsistent layout properties | No |

**Usage**:

```bash
# Run only the rules you care about
fusion-design lint --input x.fusiondesign --rules token-inconsistency,unnamed-node,empty-effects

# Auto-fix the fixable (3 rules), preview first
fusion-design lint --input x.fusiondesign --fix --dry-run

# Apply fix
fusion-design lint --input x.fusiondesign --fix
```

**Not fixable**: rules marked "No" require manual canvas adjustment (contrast/overlap/overflow etc. are semantic judgments, not mechanical fixes).

## Symptom L: Generated content contains XSS / injection (security guardrail triggered)

**Symptom**: in `codegen` HTML output, `<script>` / `onerror=` etc. are escaped to entities; or exported CSS with abnormal content is sanitized.

**Root cause**: the security guardrail actively sanitizes. codegen HTML-escapes content (XSS protection, `<`→`&lt;`), sanitizes CSS injection (blocks malicious CSS). This is **expected behavior**, not a bug.

**Mechanism**:
- codegen XSS entity-escaping: HTML special characters from AI output or user input are escaped, preventing `<script>` injection.
- CSS injection sanitization: abnormal CSS values (e.g. `url(javascript:)`, expressions) are stripped.
- Offline allowlist: HTTP egress allows only `127.0.0.1` (loopback + RFC1918 + link-local), rejects public internet. Generated code has no network-request injection.

**Fix**:
- **Generated code escaped**: if you need raw HTML tags (e.g. genuinely embedding `<script>`), this is a design tradeoff — fusion-design prioritizes safety. Restoring them manually in the output requires care; confirm the source is trusted.
- **`javascript:` URL blocked**: the guardrail rejects dangerous protocols; use normal `http(s)` or local paths.
- **To confirm the guardrail works**: deliberately pass a prompt containing `<script>alert(1)</script>`; the output should show `&lt;script&gt;`, not an executable tag.

**This is a feature**: 100% offline + XSS/injection protection is fusion-design's core security promise; do not "fix" it away as a bug.

## Symptom M: No log file on disk / need field diagnostic artifact

**Symptom**: fd-cli ran (via fusion-studio WKWebView embed or terminal direct) but no `fusion-design.log.*` file exists; or a field failure happened and there is no persistent diagnostic artifact.

**Root cause**: fd-cli writes daily-rotated logs via `tracing-appender` (OPS-13, v0.1.14). The default location is `~/Library/Logs/fusion-design/` on macOS (`~/.local/share/fusion-design/logs` on Linux). A missing file means one of: disabled via env, the dir could not be created (permissions), or the process exited before flushing.

**Fix**:
- **Default location**: `ls ~/Library/Logs/fusion-design/` — look for `fusion-design.log.YYYY-MM-DD`.
- **File disabled**: `FUSION_LOG_DISABLE_FILE=1` (or `=true`) forces stdout-only. Unset it to restore file logging:
  ```sh
  unset FUSION_LOG_DISABLE_FILE
  ```
- **Redirect to a custom dir** (useful for capturing a specific session):
  ```sh
  FUSION_LOG_DIR=/tmp/fd-session RUST_LOG=debug fusion-design --version
  ls /tmp/fd-session/   # → fusion-design.log.YYYY-MM-DD
  ```
- **Raise verbosity**: default filter is `warn`. Set `RUST_LOG=info` (or `=debug`) to capture diagnostic detail in the file:
  ```sh
  RUST_LOG=info FUSION_LOG_DIR=/tmp/fd-session fusion-design list-design-systems
  ```
- **Dir creation failed → stdout fallback**: if `init_logging` cannot `mkdir -p` the log dir (permissions/disk full), it prints `日志目录创建失败 ...，回退 stdout` to stderr and falls back to stdout-only — the CLI still runs. Check the printed path and free space.
- **Empty file**: `--version` / `--help` exit before any `tracing` event fires, so the rotated file may be 0 bytes. Run a real subcommand (`list-design-systems`, `health`, `check-mlx`) to capture events.

**Guard lifetime**: the file writer flushes when `init_logging`'s `WorkerGuard` is dropped at process exit. Normal CLI shutdown flushes correctly; a `SIGKILL` can lose the last buffered line.

## Environment Variables Reference

All optional. Unset = default. Complete table lives in `README.md` § Environment Variables; this is the troubleshooting-oriented quick reference (OPS-16).

| Variable | Default | Effect | Troubleshooting use |
|----------|---------|--------|---------------------|
| `FUSION_MLX_BASE_URL` | `http://127.0.0.1:11432` | Inference endpoint. CLI `--endpoint` overrides. Multi-node: comma-separated. | Symptom A/B/D: switch gateway→direct `11434`, or add failover node. |
| `FUSION_MLX_API_KEY` | (none) | Bearer auth key. | Symptom D: must match gateway/MLX configured key. |
| `FUSION_MLX_MODEL` | (list first) | Default model id for `check-mlx`. | Symptom C/J: pass explicit local mlx id to dodge false-green. |
| `FUSION_MLX_RETRY_MAX` | `4` | Max attempts on 502/503. `1`=disable. | Symptom B: raise for slow-loading models, lower for fast error visibility. |
| `FUSION_MLX_RETRY_DEADLINE_SECS` | `300` | Total retry deadline. | Symptom B: raise if model load exceeds 5 min. |
| `FUSION_MLX_SSE_BUFFER_CAP` | `8388608` | Max SSE buffer bytes before bail. | Symptom E: runaway output OOM guard. |
| `FUSION_MLX_STREAM_IDLE_SECS` | `60` | Max idle seconds between SSE chunks (FAULT-1, v0.1.14). | Symptom E: mid-stream stall now fails visibly instead of hanging. |
| `FUSION_LOG_DISABLE_FILE` | (unset) | `1`/`true` = stdout-only, no file log (OPS-13, v0.1.14). | Symptom M: unset to restore `~/Library/Logs/fusion-design/` file logging. |
| `FUSION_LOG_DIR` | (platform default) | Override file-log dir (OPS-13, v0.1.14). | Symptom M: redirect a session's logs to `/tmp/...` for capture. |
| `FUSION_VENV_ROOT` | (auto-detect) | Shared `.venv` root for ecosystem tool calls. | Symptom G: override when venv not co-located. |
| `FUSION_TRAINER_BIN` | `fusion-trainer` | Path to fusion-trainer binary. | Symptom G: override when not on `PATH`. |
