#!/usr/bin/env bash
# 生产代码 panic!/unimplemented!/todo!/unreachable! 硬门禁（商用加固项 #3）。
#
# 门禁语义：生产 src 顶层 panic 语句零容忍。测试代码（#[cfg(test)] mod 块）
# 精确排除——按花括号匹配求每个 test 模块的行范围，计数跳过字符串/字符/
# 原始字符串/注释，避免 CJK、JSON 示例、format! 转义花括号干扰。
#
# 修复 E-34：旧 grep `grep -vE "tests|=>"` 双重假阴性——
# (1) `=>` 排除把 match 臂 `=> panic!()` 整行过滤掉（生产代码 30+ 处漏网）；
# (2) `crates/*/src/*.rs` glob 只扫 src 直层不扫嵌套子目录。
# 本脚本用 Python 精确排除 test 块后扫全部 panic 宏（含 match 臂），消除假阴性。
#
# 已删除站点（在 allowlist 但已不在代码）静默忽略。
#
# 用法：bash Scripts/deny-panic.sh   # 本地预演
set -euo pipefail

# comm 严格按字节序比较；Python sorted() 按 UTF-8 码点序（== LC_ALL=C 字节序）。
# macOS 默认 locale 对 CJK 多字节字符排序 != 字节序，致 allowlist 与 TMP
# 排序不一致 → comm -23 误报。强制 C locale 对齐。
export LC_ALL=C

cd "$(dirname "$0")/.."

ALLOWLIST="Scripts/deny-panic-allowlist.txt"
TMP_CURRENT="$(mktemp)"
TMP_ALLOW="$(mktemp)"
trap 'rm -f "$TMP_CURRENT" "$TMP_ALLOW"' EXIT

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
            raw_hashes = 0
            k = 0
            closed = False
            while k < len(text):
                c = text[k]
                nxt = text[k + 1] if k + 1 < len(text) else ''
                if raw_hashes:
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

PANIC = re.compile(r'\b(panic!|unimplemented!|todo!|unreachable!)\s*\(')
out = []
files = sorted(glob.glob('crates/*/src/lib.rs'))
if os.path.exists('crates/fd-cli/src/main.rs'):
    files.append('crates/fd-cli/src/main.rs')
# 含嵌套子目录：补扫 src/**/*.rs（修复 E-34 glob 盲区）
files += sorted(glob.glob('crates/*/src/**/*.rs'))
for f in sorted(set(files)):
    with open(f, errors='replace') as fh:
        lines = fh.read().split('\n')
    ranges = test_block_ranges(lines)
    def in_test(ln):
        return any(s <= ln <= e for s, e in ranges)
    for idx, line in enumerate(lines, start=1):
        if in_test(idx):
            continue
        if PANIC.search(line):
            stripped = line.strip()
            if stripped.startswith('//') or stripped.startswith('/*'):
                continue
            out.append(f"{f}:{stripped}")

for line in sorted(set(out)):
    print(line)
PYEOF

LC_ALL=C sort -u -o "$TMP_CURRENT" "$TMP_CURRENT"
LC_ALL=C sort -u -o "$TMP_ALLOW" "$ALLOWLIST"
NEW=$(comm -23 "$TMP_CURRENT" "$TMP_ALLOW" || true)

if [ -n "$NEW" ]; then
    echo "::error::生产代码含 panic!/unimplemented!/todo!/unreachable!（含 match 臂），违反商用零 panic 约束"
    echo "--- 新增站点 ---"
    echo "$NEW"
    echo "--- 修复指引 ---"
    echo "1. 优先改 anyhow::bail! / Result 传播 / match 穷尽兜底，禁 unreachable!()"
    echo "2. 确属编译期不可达不变量 → 追加到 Scripts/deny-panic-allowlist.txt 并在 PR 说明理由"
    exit 1
fi

COUNT=$(wc -l < "$TMP_CURRENT" | tr -d ' ')
echo "生产代码 panic 宏站点数: ${COUNT}（全部在 allowlist，无新增）"
