#!/usr/bin/env bash
# 离线硬约束全局审计门禁（审计 H-A17）。
#
# CLAUDE.md 声明「100% 离线，唯一 HTTP 至 127.0.0.1」，但原实现无编译期/CI 级
# 全局钩子——离线全靠开发者在每条 URL 路径手动调 validate_localhost，新增 crate
# 或 fd-ai-adapter 之外引入出站 HTTP 库即静默绕过。本脚本在 CI 层强制：
#
#   1. 出站 HTTP 客户端库（reqwest/ureq/hyper/attohttpc/isahc 等）的 Cargo.toml
#      依赖声明只允许出现在 crates/fd-ai-adapter/（唯一允许发 HTTP 的 crate）。
#   2. 任何 crate src 中不得出现这些库的 use/路径调用（fd-ai-adapter 除外）。
#
# 违反 → CI 失败。新增合法出站路径须先评估是否确需，并在 PR 说明。
set -euo pipefail

cd "$(dirname "$0")/.."

# 出站 HTTP 库清单（小写匹配 Cargo.toml 依赖名）。
HTTP_LIBS="reqwest|ureq|hyper|attohttpc|isahc|surf|minreq"

status=0

# 1. Cargo.toml 依赖声明：HTTP 库只允许在 fd-ai-adapter。
violating_tomls=$(
    for toml in crates/*/Cargo.toml; do
        crate=$(basename "$(dirname "$toml")")
        if [ "$crate" = "fd-ai-adapter" ]; then
            continue
        fi
        # 匹配依赖段中的库名（行首空白 + 库名 = "版本" 或库名 = { ... }）
        if grep -qE "^[[:space:]]*(${HTTP_LIBS})([[:space:]]*=|\")" "$toml"; then
            echo "$toml"
        fi
    done
)
if [ -n "$violating_tomls" ]; then
    echo "::error::出站 HTTP 库依赖声明出现在非 fd-ai-adapter crate，违反离线硬约束（H-A17）"
    echo "$violating_tomls" | sed 's/^/  /'
    status=1
fi

# 2. src 中 HTTP 库的 use/路径调用（fd-ai-adapter 除外）。
# find 递归覆盖 src 嵌套子目录（修复 E-1 glob 盲区：crates/*/src/*.rs 只扫直层，
# 嵌套模块文件漏扫）。对齐 deny-panic.sh / deny-unwrap-expect.sh 的嵌套覆盖。
violating_srcs=$(
    while IFS= read -r src; do
        crate=$(basename "$(dirname "$(dirname "$src")")")
        if [ "$crate" = "fd-ai-adapter" ]; then
            continue
        fi
        # use reqwest / reqwest:: / reqwest! 等
        if grep -qE "(use[[:space:]]+(${HTTP_LIBS})(::|[[:space:]])|(${HTTP_LIBS})::)" "$src"; then
            echo "$src"
        fi
    done < <(find crates -type f -name '*.rs' -path '*/src/*' | sort)
)
if [ -n "$violating_srcs" ]; then
    echo "::error::非 fd-ai-adapter crate 的 src 中出现出站 HTTP 库调用，违反离线硬约束（H-A17）"
    echo "$violating_srcs" | sed 's/^/  /'
    status=1
fi

if [ "$status" -eq 0 ]; then
    echo "离线审计通过：出站 HTTP 仅限 fd-ai-adapter（H-A17）"
else
    exit 1
fi
