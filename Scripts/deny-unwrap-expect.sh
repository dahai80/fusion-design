#!/usr/bin/env bash
# 生产代码 unwrap/expect 硬门禁（商用加固项 #3 扩展）。
#
# 门禁语义：deny-panic 原 panic!/unimplemented!/todo!/unreachable! 硬门禁已存在；
# 本脚本扩展到 .unwrap()/.expect()。生产 src 现有安全站点录入 allowlist，
# 新增裸 .unwrap()/.expect() 未在 allowlist 内 → CI 失败。
#
# 测试代码排除：精确按 #[cfg(test)] mod 块的花括号匹配排除每个测试模块的行范围，
# 花括号计数跳过字符串/字符字面量与行/块注释（旧 head -1 截断法对多 test 模块文件
# 失效：fd-canvas-core 有 mod tests@1323 + mod version_tests@2223，中间的
# active_version().expect()@2038 是生产代码却被整段豁免）。修复 H-A20。
#
# 已删除站点（在 allowlist 但已不在代码）静默忽略 —— 清理只会让代码更安全。
#
# 用法：bash Scripts/deny-unwrap-expect.sh   # 本地预演
set -euo pipefail

# comm 严格按字节序比较两输入；Python sorted() 按 UTF-8 字节序输出（== 码点序）。
# macOS 默认 locale 对 CJK 多字节字符排序 != 字节序，致 allowlist 与 TMP_CURRENT
# 排序不一致 → comm -23 误报已 allowlist 的 CJK 行为「新增」。强制 C locale 对齐。
export LC_ALL=C

cd "$(dirname "$0")/.."

ALLOWLIST="Scripts/unwrap-expect-allowlist.txt"
TMP_CURRENT="$(mktemp)"
trap 'rm -f "$TMP_CURRENT"' EXIT

python3 - <<'PYEOF' > "$TMP_CURRENT"
import re, glob, os

def test_block_ranges(lines):
    # 返回 #[cfg(test)] mod 块的 (start_line, end_line) 1-indexed 列表。
    # 花括号计数跳过字符串/字符/原始字符串字面量与注释，避免 CJK、JSON 示例、
    # r#"..."# 里的 { } 干扰。原始字符串按 # 个数匹配闭合 "#...#"。
    ranges = []
    i = 0
    while i < len(lines):
        if (re.match(r'\s*#\[cfg\(test\)\]', lines[i])
                and i + 1 < len(lines)
                and re.match(r'\s*(pub\s+)?mod\s+\w+', lines[i + 1])):
            text = '\n'.join(lines[i + 1:])
            depth = 0
            in_str = in_char = in_block = in_line = False
            raw_hashes = 0  # >0 表示在原始字符串中，值为 # 个数
            k = 0
            closed = False
            while k < len(text):
                c = text[k]
                nxt = text[k + 1] if k + 1 < len(text) else ''
                if raw_hashes:
                    # 原始字符串：遇 " 后跟 raw_hashes 个 # 即闭合
                    if c == '"':
                        h = 0
                        while k + 1 + h < len(text) and text[k + 1 + h] == '#' and h < raw_hashes:
                            h += 1
                        if h == raw_hashes:
                            raw_hashes = 0
                            k += 1 + raw_hashes
                            continue
                    k += 1; continue
                if in_line:
                    if c == '\n': in_line = False
                    k += 1; continue
                if in_block:
                    if c == '*' and nxt == '/': in_block = False; k += 2; continue
                    k += 1; continue
                if in_str:
                    if c == '\\': k += 2; continue
                    if c == '"': in_str = False
                    k += 1; continue
                if in_char:
                    if c == '\\': k += 2; continue
                    if c == "'": in_char = False
                    k += 1; continue
                # 原始字符串：r/b 后跟 N 个 # 再跟 "（r#".."# / r".." / b#".."#）
                if c in ('r', 'b') and (nxt == '"' or nxt == '#'):
                    h = 0
                    j = k + 1
                    while j < len(text) and text[j] == '#':
                        h += 1; j += 1
                    if j < len(text) and text[j] == '"':
                        raw_hashes = h
                        k = j + 1; continue
                if c == '/' and nxt == '/': in_line = True; k += 2; continue
                if c == '/' and nxt == '*': in_block = True; k += 2; continue
                if c == '"': in_str = True
                elif c == "'": in_char = True
                elif c == '{': depth += 1
                elif c == '}':
                    depth -= 1
                    if depth == 0: closed = True; break
                k += 1
            if closed:
                end_line = i + 2 + text[:k].count('\n')
                ranges.append((i + 2, end_line))
                i = end_line
                continue
            break
        i += 1
    return ranges

out = []
files = sorted(glob.glob('crates/*/src/lib.rs'))
if os.path.exists('crates/fd-cli/src/main.rs'):
    files.append('crates/fd-cli/src/main.rs')
for f in files:
    with open(f, errors='replace') as fh:
        lines = fh.read().split('\n')
    ranges = test_block_ranges(lines)
    def in_test(ln):
        return any(s <= ln <= e for s, e in ranges)
    for idx, line in enumerate(lines, start=1):
        if in_test(idx):
            continue
        if '.unwrap()' in line or '.expect(' in line:
            stripped = line.strip()
            if stripped.startswith('//') or stripped.startswith('/*'):
                continue
            out.append(f"{f}:{stripped}")

for line in sorted(set(out)):
    print(line)
PYEOF

# comm 要求两输入同序。Python sorted() 按 UTF-8 码点序（== LC_ALL=C 字节序）输出；
# allowlist 文件可能经别的 locale 排序过，重排两者对齐字节序，消除 CJK 误报。
TMP_ALLOW="$(mktemp)"
trap 'rm -f "$TMP_CURRENT" "$TMP_ALLOW"' EXIT
LC_ALL=C sort -u -o "$TMP_CURRENT" "$TMP_CURRENT"
LC_ALL=C sort -u -o "$TMP_ALLOW" "$ALLOWLIST"
NEW=$(comm -23 "$TMP_CURRENT" "$TMP_ALLOW" || true)

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
