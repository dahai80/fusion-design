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

# Issue #42: register the web workbench manifest with the shell registry
# (fusion-gateway /api/v1/shell/manifests). Best-effort: gateway may be down
# or not yet implementing the endpoint — log and continue, never block start.
register_shell_manifest() {
    local manifest="$ROOT/web/manifest.json"
    local gateway="${FUSION_GATEWAY_URL:-http://127.0.0.1:11432}"
    if [ ! -f "$manifest" ]; then
        echo "shell manifest missing: $manifest (skip registration)" >&2
        return 0
    fi
    if command -v curl >/dev/null 2>&1; then
        if curl -fsS -m 3 -X POST -H "Content-Type: application/json" \
            --data @"$manifest" "$gateway/api/v1/shell/manifests" >/dev/null 2>&1; then
            echo "shell manifest registered at $gateway"
        else
            echo "shell manifest registration skipped (gateway unreachable or endpoint absent)" >&2
        fi
    fi
    return 0
}

case "${1:-}" in
    start)
        if pid_alive; then
            echo "already running (pid $(cat "$PID_FILE")) on port $PORT"
            exit 0
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
