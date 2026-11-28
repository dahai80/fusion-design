#!/usr/bin/env bash
# fusion-design-server 服务生命周期（issue#31 / DS-0）。
# 用法: ./start.sh start|stop|status|log [-f]
# 端口: 11493（port-registry.yaml 属主 fusion-design），FUSION_DESIGN_PORT 可覆盖。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
PORT="${FUSION_DESIGN_PORT:-11493}"
PID_FILE="$ROOT/.server.pid"
LOG_DIR="${FUSION_DESIGN_DATA_DIR:-$HOME/.fusion-design}/logs"
LOG_FILE="$LOG_DIR/server.log"
BIN="$ROOT/target/release/fusion-design-server"

mkdir -p "$LOG_DIR"

pid_alive() {
    [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null
}

# Issue #43: resolve the gateway admin key for manifest registration.
# Order: FUSION_ADMIN_KEY env, then gateway config web_shell.admin_key
# (FUSION_GATEWAY_CONFIG overrides, default ~/fusion/fusion-gateway/config.yaml)
# — same resolution order as fusion-supervisor env_map.rs / fusion-cowork.
# Never log or echo the key.
resolve_admin_key() {
    if [ -n "${FUSION_ADMIN_KEY:-}" ]; then
        printf '%s' "$FUSION_ADMIN_KEY"
        return 0
    fi
    local cfg="${FUSION_GATEWAY_CONFIG:-$HOME/fusion/fusion-gateway/config.yaml}"
    [ -f "$cfg" ] || return 1
    awk '
        /^[^[:space:]#]/ { in_ws = ($0 == "web_shell:") }
        in_ws && /admin_key:/ {
            sub(/.*admin_key:[[:space:]]*/, "")
            gsub(/["'"'"']/, "")
            if ($0 != "") { print; exit }
        }
    ' "$cfg"
}

# Issue #52: resolve the gateway LLM-channel client key for the server env.
# Order: FUSION_CLIENT_KEY env, then gateway config auth.api_keys — prefer the
# name=admin entry (all-model wildcard), else the first entry; same semantics
# as fusion-supervisor env_map.rs read_gateway_client_key(). Path:
# FUSION_GATEWAY_CONFIG overrides, default ~/fusion/fusion-gateway/config.yaml.
# Never log or echo the key.
resolve_client_key() {
    if [ -n "${FUSION_CLIENT_KEY:-}" ]; then
        printf '%s' "$FUSION_CLIENT_KEY"
        return 0
    fi
    local cfg="${FUSION_GATEWAY_CONFIG:-$HOME/fusion/fusion-gateway/config.yaml}"
    [ -f "$cfg" ] || return 1
    awk '
        # Entries list name and key fields in arbitrary order (sometimes dash-
        # prefixed); buffer both per entry and flush on the next "- " boundary.
        # The name=admin entry wins, else the first entry with a key.
        /^[^[:space:]#]/ { in_auth = ($0 == "auth:"); if (!in_auth) { in_keys = 0 } }
        in_auth && /^[[:space:]]+api_keys:/ { in_keys = 1; next }
        in_keys && /^[[:space:]]+- / {
            if (cur == "admin" && have_key) { print key; done = 1; exit }
            if (first_key == "" && have_key) { first_key = key }
            have_key = 0; cur = ""
        }
        in_keys && /^[[:space:]]+- (key|name):/ { sub(/^[[:space:]]+-[[:space:]]+/, "  ") }
        in_keys && /^[[:space:]]+(key|name):/ {
            line = $0
            if (line ~ /name:/) { cur = line; sub(/.*name:[[:space:]]*/, "", cur); gsub(/["'"'"']/, "", cur) }
            else { v = line; sub(/.*key:[[:space:]]*/, "", v); gsub(/["'"'"']/, "", v); if (v != "") { key = v; have_key = 1 } }
        }
        END {
            if (!done) {
                if (cur == "admin" && have_key) { print key }
                else if (first_key != "") { print first_key }
            }
        }
    ' "$cfg"
}

# Issue #42: register the web workbench manifest with the shell registry
# (fusion-gateway /api/v1/shell/manifests, admin-authed). Best-effort:
# gateway may be down or unauthenticated — log and continue, never block
# start. Issue #43: send Authorization: Bearer and distinguish failure
# modes (auth rejected vs unreachable vs endpoint absent) in the log.
register_shell_manifest() {
    local manifest="$ROOT/web/manifest.json"
    local gateway="${FUSION_GATEWAY_URL:-http://127.0.0.1:11432}"
    if [ ! -f "$manifest" ]; then
        echo "shell manifest missing: $manifest (skip registration)" >&2
        return 0
    fi
    if ! command -v curl >/dev/null 2>&1; then
        echo "shell manifest registration skipped: curl not available" >&2
        return 0
    fi
    local key
    if ! key="$(resolve_admin_key)" || [ -z "$key" ]; then
        echo "shell manifest registration skipped: no admin key (set FUSION_ADMIN_KEY or gateway config web_shell.admin_key)" >&2
        return 0
    fi
    local body code
    body="$(mktemp)" || { echo "shell manifest registration skipped: mktemp failed" >&2; return 0; }
    code="$(curl -sS -m 3 -o "$body" -w '%{http_code}' -X POST \
        -H "Content-Type: application/json" -H "Authorization: Bearer $key" \
        --data @"$manifest" "$gateway/api/v1/shell/manifests" 2>/dev/null || true)"
    rm -f "$body"
    case "$code" in
        200|201)
            echo "shell manifest registered at $gateway"
            ;;
        401|403)
            echo "shell manifest registration failed: gateway rejected auth (HTTP $code) — FUSION_ADMIN_KEY does not match gateway web_shell.admin_key" >&2
            ;;
        404)
            echo "shell manifest registration skipped: gateway endpoint absent (HTTP 404 at $gateway)" >&2
            ;;
        000)
            echo "shell manifest registration skipped: gateway unreachable ($gateway)" >&2
            ;;
        *)
            echo "shell manifest registration failed: gateway HTTP $code" >&2
            ;;
    esac
    return 0
}

case "${1:-}" in
    start)
        if pid_alive; then
            echo "already running (pid $(cat "$PID_FILE")) on port $PORT"
            exit 0
        fi
        # Issue #50/#52 (design-d28): the nightly eval hit fusion-mlx HTTP 403 on
        # every /api/generate because the server was launched without
        # FUSION_CLIENT_KEY (the gateway LLM-channel key). Resolve the key with
        # the supervisor chain (env -> gateway config auth.api_keys[admin]) and
        # hand it to the server process; warn loudly only if both sources fail.
        if [ -z "${FUSION_CLIENT_KEY:-}" ]; then
            if k="$(resolve_client_key)" && [ -n "$k" ]; then
                export FUSION_CLIENT_KEY="$k"
                echo "FUSION_CLIENT_KEY resolved from gateway config (auth.api_keys[admin])"
                echo "FUSION_CLIENT_KEY resolved from gateway config auth.api_keys[admin]" >> "$LOG_FILE"
            else
                echo "warning: FUSION_CLIENT_KEY not set and gateway config has no auth.api_keys[admin] - model-backed endpoints will return 502 (upstream 403)" >&2
                echo "warning: FUSION_CLIENT_KEY unresolved - model-backed endpoints will return 502 (upstream 403)" >> "$LOG_FILE"
            fi
        fi
        if [ ! -x "$BIN" ]; then
            echo "binary missing: $BIN" >&2
            echo "build first: cargo build --release -p fd-server" >&2
            exit 1
        fi
        nohup "$BIN" >> "$LOG_FILE" 2>&1 &
        echo $! > "$PID_FILE"
        sleep 1
        if ! pid_alive; then
            echo "start failed, see $LOG_FILE" >&2
            tail -5 "$LOG_FILE" >&2 || true
            rm -f "$PID_FILE"
            exit 1
        fi
        echo "started (pid $(cat "$PID_FILE")) on http://127.0.0.1:$PORT"
        register_shell_manifest
        ;;
    stop)
        if pid_alive; then
            PID="$(cat "$PID_FILE")"
            kill "$PID"
            for _ in 1 2 3 4 5; do
                kill -0 "$PID" 2>/dev/null || break
                sleep 1
            done
            kill -0 "$PID" 2>/dev/null && kill -9 "$PID" 2>/dev/null || true
            echo "stopped (pid $PID)"
        else
            echo "not running"
        fi
        rm -f "$PID_FILE"
        ;;
    status)
        if pid_alive; then
            echo "running (pid $(cat "$PID_FILE")) on http://127.0.0.1:$PORT"
        else
            echo "not running"
            exit 1
        fi
        ;;
    log)
        tail "${@:2}" -n 100 "$LOG_FILE" 2>/dev/null || echo "no log yet: $LOG_FILE"
        ;;
    *)
        echo "usage: $0 start|stop|status|log [-f]" >&2
        exit 1
        ;;
esac
