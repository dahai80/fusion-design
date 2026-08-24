#!/usr/bin/env bash
# 生产代码 unwrap/expect 硬门禁（商用加固项 #3 扩展）。
#
# 门禁语义：deny-panic 原 panic!/unimplemented!/todo!/unreachable! 硬门禁已存在；
# 本脚本扩展到 .unwrap()/.expect()。生产 src 现有安全站点录入 allowlist，
# 新增裸 .unwrap()/.expect() 未在 allowlist 内 → CI 失败。
#
# 测试代码（#[cfg(test)] 之后）天然排除：每个 lib.rs 的 cfg(test) 块在文件末尾，
# awk 在 cfg(test) 行处截断，仅扫描其上方的生产代码。
#
# 已删除站点（在 allowlist 但已不在代码）静默忽略 —— 清理只会让代码更安全。
#
# 用法：bash Scripts/deny-unwrap-expect.sh   # 本地预演
set -euo pipefail

cd "$(dirname "$0")/.."

ALLOWLIST="Scripts/unwrap-expect-allowlist.txt"
TMP_CURRENT="$(mktemp)"
trap 'rm -f "$TMP_CURRENT"' EXIT

# 生成当前生产代码 unwrap/expect 的 path:content 键（去前导空白，排除注释行）。
for f in crates/*/src/lib.rs crates/fd-cli/src/main.rs; do
    [ -f "$f" ] || continue
    cutln=$(grep -n "#\[cfg(test)\]" "$f" | head -1 | cut -d: -f1 || true)
    [ -z "$cutln" ] && cutln=999999
    awk -v cut="$cutln" -v file="$f" '
        NR<cut && (/\.unwrap\(\)/ || /\.expect\(/) {
            line=$0; sub(/^[ \t]+/,"",line)
            if (line !~ /^\/\// && line !~ /^\/\*/) print file ":" line
        }
    ' "$f"
done | sort -u > "$TMP_CURRENT"

# 新增项 = 当前存在但不在 allowlist（comm -23 = 仅在第一文件/当前的行）。
NEW=$(comm -23 "$TMP_CURRENT" "$ALLOWLIST" || true)

if [ -n "$NEW" ]; then
    echo "::error::生产代码新增裸 .unwrap()/.expect() 未在 allowlist，违反商用零 panic 扩展约束"
    echo "--- 新增站点 ---"
    echo "$NEW"
    echo "--- 修复指引 ---"
    echo "1. 优先改 .unwrap_or_default() / .ok()? / match 或 anyhow 传播"
    echo "2. 确属安全站点（编译期常量、WASM window() 惯用法、启动失败 fail-fast）→"
    echo "   追加到 Scripts/unwrap-expect-allowlist.txt 并在 PR 说明理由"
    exit 1
fi

COUNT=$(wc -l < "$TMP_CURRENT" | tr -d ' ')
echo "生产代码 unwrap/expect 站点数: ${COUNT}（全部在 allowlist，无新增）"
