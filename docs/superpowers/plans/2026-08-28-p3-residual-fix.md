# P3 残留项 + ARCH-10 round 2/3 全修复 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 闭合生产审计 §9.3 P3 工程债 8 项 + §9.2 ARCH-10 round 2/3 残留，达 v0.1.15 工程完备发布门禁。

**Architecture:** 按风险递增 3 档执行。档 1（5 项 surgical 单 crate 改）→ 档 2（ARCH-3 fd-cli 13 内联 arm 机械拆分）→ 档 3（3 项高风险结构改：ARCH-10 round 2/3 thread_local 模块拆分、ARCH-11 fd-skills 新 crate + trait 解耦、FUNC-7 delta undo）。每档全量门禁 checkpoint，档 3 逐项独立可回退。

**Tech Stack:** Rust 2021, toolchain 1.94, tokio (fs feature 已在 workspace), printpdf 0.12 (default-features=false), thiserror, serde, wasm-bindgen, thread_local Closure storage (wasm 单线程).

**Spec:** `docs/superpowers/specs/2026-08-28-p3-residual-fix-design.md`

## Global Constraints

- 工具链：`~/.cargo/bin/cargo +1.94` ONLY（homebrew cargo 1.96 忽略 rust-toolchain.toml）。
- rtk 代理吞退出码：所有 cargo/脚本校验用输出文案（"0 failed"/"Finished"/无 "error["）校验，非 `$?`。
- 4 空格缩进（4 的倍数），不生成 docstring，代码注释解释意图，写代码默认有 tracing 日志（fd-host-web 无 tracing dep 用 web_sys::console::log）。
- 非 test `src/` 零 `unwrap()`/`expect()`/`panic!`/`unimplemented!`/`todo!`/`unreachable!`（CI 三门禁：deny-panic + deny-unwrap-expect allowlist + deny-external-http）。
- 100% 离线，出站 HTTP 仅 `127.0.0.1`/私有段，无云 API。
- commit/push only when user asks；if on default branch, branch first。
- AI 测试须真实加载模型（起停 `~/claude-home/fusion-mlx/start.sh`），不 mock 集成测试。
- 所有测试验证完成后清理过程数据，只保留最终输出件和日志。
- 每执行完一个 Task 主动 compact（保留：待修改文件、接口方案、未完成任务清单、业务约束；丢弃：旧终端输出、重复文件全文、废弃调试日志）。

---

## File Structure

**档 1（surgical）：**
- `crates/fd-cli/src/common.rs` — FAULT-2 `read_stdin_capped` 字节累加
- `crates/fd-cli/src/main.rs` — PERF-4 7 处 `std::fs::write` → `tokio::fs::write`；ARCH-3 arm 拆分
- `crates/fd-cli/tests/common_tests.rs`（新）— FAULT-2 多字节切断测试
- `crates/fd-export/src/lib.rs` — SEC-N3 `sanitize_image_url` 拒 `..`；FUNC-9 旋转 Transform
- `crates/fd-export/tests/export_security.rs`（新）— SEC-N3 路径穿越测试
- `crates/fd-design-lint/src/lib.rs` — PERF-3 `check_overlapping_nodes` 阈值降级

**档 2（ARCH-3）：**
- `crates/fd-cli/src/commands/design_system.rs`（新）— ListDesignSystems + Activate + TokenCSS + Theme
- `crates/fd-cli/src/commands/ai_skill.rs`（新）— ImageToUi + MultiVariants + SpecDoc + PageFlow
- `crates/fd-cli/src/commands/io_cmd.rs`（新）— ExportBatch + ParseHtml + Codegen
- `crates/fd-cli/src/commands/history.rs`（新）— Undo + Redo + Diff
- `crates/fd-cli/src/commands/misc.rs`（新）— CheckFrontend + Train
- `crates/fd-cli/src/commands/mod.rs` — 补 5 个 `pub mod` 声明
- `crates/fd-cli/src/main.rs` — 13 内联 arm 改委托调用

**档 3（高风险）：**
- `crates/fd-host-web/src/bridge.rs`（新）— ARCH-10 round2 消息派发
- `crates/fd-host-web/src/render_dom.rs`（新）— ARCH-10 round3 DOM 渲染
- `crates/fd-host-web/src/render_canvas.rs`（新）— ARCH-10 round3 canvas 渲染
- `crates/fd-host-web/src/events.rs`（新）— ARCH-10 round3 拖拽/点击 Closure（最高风险，可回退）
- `crates/fd-host-web/src/lib.rs` — 模块声明 + 移出代码
- `crates/fd-skills/Cargo.toml`（新）— ARCH-11 叶子 crate
- `crates/fd-skills/src/lib.rs`（新）— DesignSkill trait + SkillContext trait + SkillRegistry + 7 skill impl
- `crates/fd-skills/tests/skills_mock.rs`（新）— mock SkillContext 单测
- `crates/fd-ai-adapter/src/lib.rs` — ARCH-11 impl SkillContext for FusionMlxClient，移除 skill 代码
- `crates/fd-ai-adapter/Cargo.toml` — 加 fd-skills dep
- `Cargo.toml`（workspace）— 加 `"crates/fd-skills"`
- `crates/fd-canvas-core/src/lib.rs` — FUNC-7 PenDocumentDiff + delta 栈
- `crates/fd-canvas-core/tests/undo_delta.rs`（新）— diff compute/undo/redo/序列化往返/旧格式迁移

---

## 档 1 — surgical + 中等（低中风险）

### Task 1: FAULT-2 — stdin 字节边界损坏修复

**Files:**
- Modify: `crates/fd-cli/src/common.rs:13-34`
- Test: `crates/fd-cli/tests/common_tests.rs`（新）

**Interfaces:**
- Consumes: 无（`read_stdin_capped` 是公共入口）
- Produces: `pub fn read_stdin_capped() -> anyhow::Result<String>`（签名不变，行为改字节安全）

- [ ] **Step 1: 写失败测试**

`crates/fd-cli/tests/common_tests.rs`:
```rust
use fd_cli::common::read_stdin_capped;
use std::io::Write;

// FAULT-2：多字节字符恰被 chunk 切断时，from_utf8_lossy 会用 U+FFFD 替换损坏字节。
// 改字节累加 + 末尾 from_utf8 后，切断的字符在拼接后完整，输出无损。
#[test]
fn read_stdin_capped_multibyte_split_across_chunk() {
    // 「中」= 0xE4 0xB8 0xAD（3 字节）。构造「中」重复 64KB/3 ≈ 21845 次，
    // 让 chunk 边界落在多字节字符中间。用子进程喂 stdin 隔离测试自身 stdin。
    let chars_per_chunk = 64 * 1024 / 3;
    let mut input = String::new();
    for _ in 0..(chars_per_chunk + 10) {
        input.push('中');
    }
    let expected = input.clone();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--read-stdin-test")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(input.as_bytes()).unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "子进程失败: {}", String::from_utf8_lossy(&output.stderr));
    let got = String::from_utf8(output.stdout).unwrap();
    assert_eq!(got.trim_end(), expected, "多字节字符跨 chunk 切断后数据损坏");
}
```

注：测试需子进程入口。若 `--read-stdin-test` 不存在，改用直接调 `read_stdin_capped` + pipe 重定向（见 Step 3 备选）。

- [ ] **Step 2: 跑测试验证失败**

Run: `~/.cargo/bin/cargo +1.94 test -p fd-cli --test common_tests -- read_stdin_capped_multibyte_split_across_chunk`
Expected: 当前实现 `from_utf8_lossy` 替换切断字节 → 输出含 U+FFFD → `assert_eq!` FAIL。或编译失败（测试入口不存在）。

- [ ] **Step 3: 改 `read_stdin_capped` 字节累加**

`crates/fd-cli/src/common.rs:13-34` 改为：
```rust
/// 分块读 stdin 至 STDIN_READ_CAP，超限 bail + warn。
pub fn read_stdin_capped() -> anyhow::Result<String> {
    use std::io::Read;
    let stdin = std::io::stdin();
    let mut handle = stdin.lock();
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = handle.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        // FAULT-2：字节累加原始 bytes，不在循环内转 UTF-8。
        // chunk 可能在多字节字符中间切断，循环内 from_utf8_lossy 会用 U+FFFD 替换损坏字节。
        // 末尾一次性 from_utf8 保证边界完整，无效 UTF-8 返 Err fail visibly（不静默替换）。
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > STDIN_READ_CAP {
            tracing::warn!("stdin 超过 {STDIN_READ_CAP} 字节上限，拒绝读取");
            anyhow::bail!("stdin 输入超过 {STDIN_READ_CAP} 字节上限，拒绝读取防 OOM");
        }
    }
    String::from_utf8(buf).map_err(|e| anyhow::anyhow!("stdin 输入非 UTF-8: {e}"))
}
```

若 Step 1 测试用子进程入口方案，需在 `main.rs` 末加 `#[cfg(test)]` 子进程测试入口分支（不可——bin crate 无法被自身子进程识别测试 flag）。**改用备选**：测试直接调 `read_stdin_capped` 时无法重定向 stdin。**最终方案**：测试不调 `read_stdin_capped`，改为抽纯函数 `pub fn decode_stdin_bytes(buf: Vec<u8>) -> anyhow::Result<String>` 测试，`read_stdin_capped` 内部调它。Step 1 测试改为：
```rust
#[test]
fn decode_stdin_bytes_multibyte_split() {
    // 「中」= 0xE4 0xB8 0xAD。把首字符的后 2 字节放 chunk 末、前 1 字节放下 chunk 首，
    // 模拟 chunk 边界切断。旧 from_utf8_lossy 逐 chunk 转会丢字节。
    let mut bytes = vec![0xE4, 0xB8, 0xAD]; // 完整「中」
    bytes.push(0xE4); // 切断：下字符首字节孤立
    let mut full = bytes.clone();
    full.push(0xB8);
    full.push(0xAD); // 拼「中中」
    let got = fd_cli::common::decode_stdin_bytes(full).unwrap();
    assert_eq!(got, "中中");
}
```
`common.rs` 加：
```rust
/// FAULT-2：纯函数，累加 bytes 一次性 from_utf8。供 read_stdin_capped 复用 + 单测。
pub fn decode_stdin_bytes(buf: Vec<u8>) -> anyhow::Result<String> {
    String::from_utf8(buf).map_err(|e| anyhow::anyhow!("stdin 输入非 UTF-8: {e}"))
}
```
`read_stdin_capped` 末改 `decode_stdin_bytes(buf)`。

- [ ] **Step 4: 跑测试验证通过**

Run: `~/.cargo/bin/cargo +1.94 test -p fd-cli --test common_tests`
Expected: PASS

- [ ] **Step 5: commit**

```bash
git add crates/fd-cli/src/common.rs crates/fd-cli/tests/common_tests.rs
git commit -m "fix(FAULT-2): stdin read_stdin_capped 字节累加修复多字节切断损坏

循环内 from_utf8_lossy 在多字节字符中间切断时用 U+FFFD 替换损坏字节致数据损坏。
改 Vec<u8> 累加原始 bytes + 末尾一次性 from_utf8，边界完整，无效 UTF-8 返 Err fail visibly。
抽纯函数 decode_stdin_bytes 供单测。"
```

### Task 2: PERF-4 — async CLI handler 阻塞 worker 修复

**Files:**
- Modify: `crates/fd-cli/src/main.rs:646,677,701,724,816,836,857`（7 处 `std::fs::write`）

**Interfaces:**
- Consumes: 无
- Produces: 无（行为等价，仅非阻塞化）。tokio "fs" feature 已在 workspace `Cargo.toml:45`。

- [ ] **Step 1: 定位 7 处 `std::fs::write`**

Run: `grep -n "std::fs::write" crates/fd-cli/src/main.rs`
Expected: 7 行（646/677/701/724/816/836/857 附近，行号随 Task 1 后偏移）。

- [ ] **Step 2: 逐处改 `tokio::fs::write`**

每处 `std::fs::write(&p, &json)?` → `tokio::fs::write(&p, &json).await?`。同理 `&code`、`&hist_out` 变体。
7 处全改。无新增 import（tokio 已 `use` 在 crate）。

- [ ] **Step 3: 编译验证**

Run: `~/.cargo/bin/cargo +1.94 check -p fd-cli`
Expected: Finished（无 error[）。

- [ ] **Step 4: 跑现有 undo/redo/export 测试验证写路径正常**

Run: `~/.cargo/bin/cargo +1.94 test -p fd-cli`
Expected: 全 PASS（行为等价，仅非阻塞化，无新测试）。

- [ ] **Step 5: commit**

```bash
git add crates/fd-cli/src/main.rs
git commit -m "perf(PERF-4): fd-cli async handler 7 处 std::fs::write 改 tokio::fs::write

async run() 内同步文件写阻塞 tokio worker，单线程 runtime 时阻塞所有异步任务。
tokio::fs::write 异步文件 IO 不阻塞 worker。行为等价，非阻塞化。"
```

### Task 3: SEC-N3 — sanitize_image_url 路径穿越修复

**Files:**
- Modify: `crates/fd-export/src/lib.rs:442-469`
- Test: `crates/fd-export/tests/export_security.rs`（新）

**Interfaces:**
- Consumes: 无
- Produces: `fn sanitize_image_url(raw: &str) -> String`（私有，行为改拒 `..`）

- [ ] **Step 1: 写失败测试**

`crates/fd-export/tests/export_security.rs`:
```rust
// SEC-N3 须测私有函数。用 #[cfg(test)] 暴露或改 pub(crate)+测试在同 crate。
// 本测试放同 crate tests 模块更简——见 lib.rs 内 tests。此处用集成测试测导出产物含/不含 href。
use fd_export::Exporter;
```
私有函数不可集成测试。**改方案**：把 `sanitize_image_url` 改 `pub(crate)` + 在 `lib.rs` 内 `#[cfg(test)] mod tests` 加单测：
```rust
#[test]
fn sanitize_image_url_rejects_dotdot_traversal() {
    assert_eq!(sanitize_image_url("../secret"), "");
    assert_eq!(sanitize_image_url("a/../b"), "");
    assert_eq!(sanitize_image_url("../../etc/passwd"), "");
}
#[test]
fn sanitize_image_url_allows_normal_relative() {
    assert_eq!(sanitize_image_url("logo.png"), "logo.png");
    assert_eq!(sanitize_image_url("assets/icon.svg"), "assets/icon.svg");
    assert_eq!(sanitize_image_url("data:image/png;base64,xxx"), "data:image/png;base64,xxx");
}
```

- [ ] **Step 2: 跑测试验证失败**

Run: `~/.cargo/bin/cargo +1.94 test -p fd-export -- sanitize_image_url`
Expected: `rejects_dotdot_traversal` FAIL（当前 `../secret` 返 `../secret` 非空串）。

- [ ] **Step 3: 加 `..` 拒绝逻辑**

`crates/fd-export/src/lib.rs:466-468`（"无协议前缀"放行分支前）插入：
```rust
    // SEC-N3：无协议前缀路径放行前检查 .. 段防路径穿越。
    // 离线 SVG 无出网，但相对路径可解析到文件系统敏感位置（理论风险）。
    if trimmed.contains("/../") || trimmed.starts_with("../") || trimmed == ".." {
        tracing::warn!(url = %trimmed, "SVG image href 拒绝含 .. 的路径穿越意图");
        return String::new();
    }
```

- [ ] **Step 4: 跑测试验证通过**

Run: `~/.cargo/bin/cargo +1.94 test -p fd-export -- sanitize_image_url`
Expected: PASS

- [ ] **Step 5: commit**

```bash
git add crates/fd-export/src/lib.rs
git commit -m "fix(SEC-N3): sanitize_image_url 拒绝含 .. 的路径穿越

无协议前缀路径直接放行，../secret 可解析到文件系统敏感位置。
放行前检查 /../ 与 ../ 前缀，命中返空串 + warn。纯文件名/正常相对路径仍放行。"
```

### Task 4: FUNC-9 — PDF 旋转丢失修复

**Files:**
- Modify: `crates/fd-export/src/lib.rs:730-739`

**Interfaces:**
- Consumes: 无
- Produces: PDF 导出支持旋转 shape（printpdf Op::Transform）

- [ ] **Step 1: 探针 printpdf 0.12 API 可用性**

printpdf 0.12 `default-features=false`。须核实 `printpdf::ops::Op::Transform` + `printpdf::transforms::Matrix` 是否存在。
Run: `grep -rn "pub enum Op" ~/.cargo/registry/src/*/printpdf-0.12*/src/ops/ 2>/dev/null; grep -rn "pub struct Matrix\|pub fn rotate\|pub fn identity" ~/.cargo/registry/src/*/printpdf-0.12*/src/transforms/ 2>/dev/null`
Expected: 找到 `Op::Transform { matrix }` + `Matrix::rotate`/`Matrix::identity`。
**若 API 不可用**（default-features=false 砍掉 transforms 模块）：跳到 Step 6 回退文档标注，不改代码。

- [ ] **Step 2: 若 API 可用，写失败测试**

在 `lib.rs` `#[cfg(test)] mod tests` 加：
```rust
// FUNC-9：旋转 shape 导出 PDF 不应进 other=> 跳过分支。验证 skipped_kinds 不含旋转项。
// 难以直接断言 PDF 二进制——改测内部辅助函数（若有 rotation 形状枚举）。
// 简化：构造带旋转节点 doc 调 render_pdf 到 tempfile，断言无 panic + 文件非空 + 无 warn 日志。
#[test]
fn pdf_export_rotated_shape_not_skipped() {
    // 构造 90° 旋转 rect doc，导出 PDF，断言文件生成非空。
    // printpdf Transform 路径生效则 shape 不进 other=> 分支。
    // ... 见 Step 3 实现后补全
}
```

- [ ] **Step 3: 插入 Op::Transform 旋转**

`crates/fd-export/src/lib.rs` 处理旋转 shape 的分支（当前在 `other =>` 前）。须先找到旋转字段的现有处理。若旋转 shape 当前无独立分支（全进 other），则加分支：
```rust
            // FUNC-9：带旋转的 shape 插入 Op::Transform 旋转，shape 后重置。
            shape_with_rotation if has_rotation(shape_with_rotation) => {
                let angle = shape_with_rotation.rotation;
                ops.push(printpdf::ops::Op::Transform {
                    matrix: printpdf::transforms::Matrix::rotate(
                        angle as f32,
                        printpdf::Pt(el.x as f32),
                        printpdf::Pt(el.y as f32),
                    ),
                });
                // ... 原 shape 绘制 ops ...
                ops.push(printpdf::ops::Op::Transform {
                    matrix: printpdf::transforms::Matrix::identity(),
                });
            }
```
**注**：实际 shape 处理结构须读 700-739 完整上下文确定 rotation 字段名 + 现有绘制调用。Step 3 实施时先 Read 700-739 全段再写精确代码。

- [ ] **Step 4: 跑测试 + 编译验证**

Run: `~/.cargo/bin/cargo +1.94 test -p fd-export -- pdf_export_rotated`
Expected: PASS + 无编译 error[。

- [ ] **Step 5: commit**

```bash
git add crates/fd-export/src/lib.rs
git commit -m "fix(FUNC-9): PDF 导出旋转 shape 插入 Op::Transform 不再跳过

非 0 旋转 shape 原 other=> 跳过+warn，丢旋转。printpdf 0.12 Op::Transform + Matrix::rotate
对带旋转 shape 在绘制 ops 前后插入 transform/identity 重置（save/restore 语义）。"
```

- [ ] **Step 6（回退）：若 Step 1 API 不可用**

保留 `other =>` warn 分支。改注释（731-732）：
```rust
            // L-12/R-11：不支持此元素类型不静默丢弃——聚合 skipped_kinds。
            // FUNC-9：printpdf 0.12 default-features=false 无 Op::Transform，旋转 shape 无法支持。
            // 保留 warn 跳过。如需旋转支持须启用 printpdf transforms feature（重依赖权衡后另议）。
```
commit: `docs(FUNC-9): PDF 旋转 shape 受 printpdf feature 限制，文档标注保留 warn`
**进 Task 5。**

### Task 5: PERF-3 — lint overlap O(n²) 阈值降级

**Files:**
- Modify: `crates/fd-design-lint/src/lib.rs:528-553`
- Test: `crates/fd-design-lint/src/lib.rs`（内联 `#[cfg(test)] mod tests`）

**Interfaces:**
- Consumes: `PenNode`（fd-canvas-core）
- Produces: `check_overlapping_nodes` >200 节点 warn+skip

- [ ] **Step 1: 写失败测试**

`lib.rs` `#[cfg(test)] mod tests` 加：
```rust
#[test]
fn check_overlapping_skips_when_over_threshold() {
    // PERF-3：>200 节点兄弟组应 warn + 0 violation（跳过 O(n²)）。
    use fd_canvas_core::{PenNode, NodeStyle};
    let mut siblings = Vec::new();
    for i in 0..201 {
        siblings.push(PenNode {
            id: format!("n{i}"),
            name: format!("n{i}"),
            kind: fd_canvas_core::NodeKind::Rect,
            x: 0.0, y: 0.0, w: 10.0, h: 10.0,
            style: NodeStyle::default(),
            text: None,
            children: vec![],
            rotation: 0.0,
            z_index: 0,
        });
    }
    let checker = LintChecker::new();
    let mut violations = Vec::new();
    checker.check_overlapping_nodes(&siblings, &mut violations);
    assert!(violations.is_empty(), ">200 节点应跳过重叠检测，不应产出 violation");
}
```
**注**：`PenNode` 字段顺序按 fd-canvas-core lib.rs:130 定义（id/kind/name/x/y/w/h/style/text/children/rotation/z_index），无 opacity。`check_overlapping_nodes` 实际签名须 Read lib.rs:528 核实（可能含 `&self`/page 参数），Step 3 实施时确认调用方。

- [ ] **Step 2: 跑测试验证失败**

Run: `~/.cargo/bin/cargo +1.94 test -p fd-design-lint -- check_overlapping_skips`
Expected: 当前 201 节点仍跑 O(n²) → 产出大量 violation → `is_empty` FAIL。

- [ ] **Step 3: 加阈值降级**

`crates/fd-design-lint/src/lib.rs:528-530` 改 `check_overlapping_nodes` 开头：
```rust
    fn check_overlapping_nodes(&self, siblings: &[PenNode], violations: &mut Vec<LintViolation>) {
        // PERF-3：兄弟组节点数 > 阈值时降级——O(n²) worst-case 无界。
        // 小文档（≤200）保持精确检测，大文档 warn + skip（fail visibly）。
        const OVERLAP_CHECK_THRESHOLD: usize = 200;
        if siblings.len() > OVERLAP_CHECK_THRESHOLD {
            tracing::warn!(
                count = siblings.len(),
                threshold = OVERLAP_CHECK_THRESHOLD,
                "节点过多跳过重叠检测（O(n²) 降级），worst-case 退化"
            );
            return;
        }
        // E-25：旧实现 a.z_index == b.z_index 才检测 → 不同 z_index 重叠漏检。
        // 改为检测视觉重叠（bbox 相交）即报，z_index 差值大时降级为 Info。
        for i in 0..siblings.len() {
            // ... 原 O(n²) 逻辑不动 ...
```

- [ ] **Step 4: 跑测试验证通过**

Run: `~/.cargo/bin/cargo +1.94 test -p fd-design-lint`
Expected: PASS（新测试 + 现有全过）。

- [ ] **Step 5: commit**

```bash
git add crates/fd-design-lint/src/lib.rs
git commit -m "perf(PERF-3): check_overlapping_nodes >200 节点阈值降级跳过 O(n²)

兄弟组无上限时 O(n²) worst-case 无界。>200 节点 warn + skip（fail visibly），
≤200 保持现有精确检测。Rule 2 简单性：阈值降级替代扫描线，简单可靠。"
```

### Checkpoint 档 1

- [ ] **全量门禁**：
```bash
~/.cargo/bin/cargo +1.94 test --workspace     # 0 failed（现 474 + 档 1 新增）
~/.cargo/bin/cargo +1.94 clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo +1.94 fmt --all -- --check
Scripts/deny-panic.sh && Scripts/deny-unwrap-expect.sh && Scripts/deny-external-http.sh
```
全绿才进档 2。

---

## 档 2 — ARCH-3 fd-cli 13 内联 arm 拆分（机械低风险）

### Task 6: ARCH-3 拆分设计系统 arm 组

**Files:**
- Create: `crates/fd-cli/src/commands/design_system.rs`
- Modify: `crates/fd-cli/src/commands/mod.rs`，`crates/fd-cli/src/main.rs:526-542,755-764,877-890`

**Interfaces:**
- Consumes: `fd_cli::design::DesignSystemRegistry`，`fd_design_system::DesignSystemRegistry`（Theme 用后者）
- Produces: `pub async fn run_list_design_systems()`, `run_activate(id: String)`, `run_token_css(design_system: String)`, `run_theme(design_system: String, mode: ThemeModeArg)` — 各返 `anyhow::Result<()>`

- [ ] **Step 1: 创建 design_system.rs**

`crates/fd-cli/src/commands/design_system.rs`:
```rust
//! A-3：设计系统相关子命令（ListDesignSystems/Activate/TokenCSS/Theme）从 main.rs 拆出。

use crate::design;

pub fn run_list_design_systems() -> anyhow::Result<()> {
    let mut reg = design::DesignSystemRegistry::new();
    reg.register_builtin();
    for id in reg.list() {
        println!("{id}");
    }
    Ok(())
}

pub fn run_activate(id: String) -> anyhow::Result<()> {
    let mut reg = design::DesignSystemRegistry::new();
    reg.register_builtin();
    reg.activate(&id)?;
    println!("已激活: {id}");
    Ok(())
}

pub fn run_token_css(design_system: String) -> anyhow::Result<()> {
    let mut reg = design::DesignSystemRegistry::new();
    reg.register_builtin();
    let system = reg.get(&design_system).ok_or_else(|| {
        anyhow::anyhow!("设计规范 '{design_system}' 未找到，可用: {:?}", reg.list())
    })?;
    println!("{}", system.to_css_custom_properties());
    Ok(())
}

pub fn run_theme(design_system: String, mode: crate::ThemeModeArg) -> anyhow::Result<()> {
    let mut reg = fd_design_system::DesignSystemRegistry::new();
    reg.register_builtin();
    let system = reg.get(&design_system).ok_or_else(|| {
        anyhow::anyhow!("设计规范 '{design_system}' 未找到，可用: {:?}", reg.list())
    })?;
    let css = system.to_css_custom_properties_for_theme(mode.into());
    println!("{css}");
    Ok(())
}
```
**注**：`ThemeModeArg` 类型名须读 main.rs `Command::Theme` 枚举核实（Step 3 Read 确认）。

- [ ] **Step 2: mod.rs 补声明**

`crates/fd-cli/src/commands/mod.rs` 加 `pub mod design_system;`

- [ ] **Step 3: main.rs 改 4 arm 委托**

`main.rs:526-542`（ListDesignSystems + Activate）、755-764（TokenCSS）、877-890（Theme）改：
```rust
        Command::ListDesignSystems => commands::design_system::run_list_design_systems(),
        Command::Activate { id } => commands::design_system::run_activate(id),
        ...
        Command::TokenCSS { design_system } => commands::design_system::run_token_css(design_system),
        ...
        Command::Theme { design_system, mode } => commands::design_system::run_theme(design_system, mode),
```
删各 arm 内联 `// TODO A-3` + 代码体。

- [ ] **Step 4: 编译验证**

Run: `~/.cargo/bin/cargo +1.94 check -p fd-cli`
Expected: Finished（无 error[）。若 `ThemeModeArg` 未 `pub` 或路径错，改 visibility/路径。

- [ ] **Step 5: 跑测试验证**

Run: `~/.cargo/bin/cargo +1.94 test -p fd-cli`
Expected: 全 PASS（行为不变）。

- [ ] **Step 6: commit**

```bash
git add crates/fd-cli/src/commands/design_system.rs crates/fd-cli/src/commands/mod.rs crates/fd-cli/src/main.rs
git commit -m "refactor(ARCH-3): 拆 ListDesignSystems/Activate/TokenCSS/Theme 到 commands/design_system.rs

从 main.rs run() 内联 arm 提取，遵循既有 check_mlx.rs 模式（pub async/sync fn run）。零逻辑改。"
```

### Task 7: ARCH-3 拆分 AI skill arm 组

**Files:**
- Create: `crates/fd-cli/src/commands/ai_skill.rs`
- Modify: `crates/fd-cli/src/commands/mod.rs`，`crates/fd-cli/src/main.rs:627-730`

**Interfaces:**
- Consumes: `fd_ai_adapter::FusionMlxClient`，`fd_ai_adapter::DesignSkills`，`crate::common`
- Produces: `pub async fn run_image_to_ui(...)`, `run_multi_variants(...)`, `run_spec_doc(...)`, `run_page_flow(...)` — 各返 `anyhow::Result<()>`

- [ ] **Step 1: 创建 ai_skill.rs**

`crates/fd-cli/src/commands/ai_skill.rs`:
```rust
//! A-3：AI skill 子命令（ImageToUi/MultiVariants/SpecDoc/PageFlow）从 main.rs 拆出。

use crate::common;
use std::path::PathBuf;

pub async fn run_image_to_ui(
    sketch: PathBuf,
    hint: String,
    page: String,
    model: String,
    endpoint: String,
    out: Option<PathBuf>,
) -> anyhow::Result<()> {
    let client = fd_ai_adapter::FusionMlxClient::with_endpoints(
        fd_ai_adapter::FusionMlxClient::resolve_endpoint(&endpoint)?,
    )?;
    let skills = fd_ai_adapter::DesignSkills::new(client, model);
    let doc = skills
        .image_to_ui_async(&sketch.to_string_lossy(), &hint, &page)
        .await?;
    let json = serde_json::to_string_pretty(&doc)?;
    match out {
        Some(p) => {
            tokio::fs::write(&p, &json).await?;
            println!("已生成图生 UI PenDocument JSON 到 {p:?}");
        }
        None => println!("{json}"),
    }
    Ok(())
}

pub async fn run_multi_variants(
    prompt: String,
    page: String,
    styles: Option<Vec<String>>,
    model: String,
    endpoint: String,
    out: Option<PathBuf>,
) -> anyhow::Result<()> {
    let client = fd_ai_adapter::FusionMlxClient::with_endpoints(
        fd_ai_adapter::FusionMlxClient::resolve_endpoint(&endpoint)?,
    )?;
    let skills = fd_ai_adapter::DesignSkills::new(client, model);
    let picked = common::pick_multi_variant_styles(styles);
    let docs = skills
        .multi_variants_async(
            &prompt,
            &page,
            [picked[0].as_str(), picked[1].as_str(), picked[2].as_str()],
        )
        .await?;
    let json = serde_json::to_string_pretty(&docs)?;
    match out {
        Some(p) => {
            tokio::fs::write(&p, &json).await?;
            println!("已生成 3 套多方案 PenDocument JSON 到 {p:?}");
        }
        None => println!("{json}"),
    }
    Ok(())
}

pub async fn run_spec_doc(
    input: PathBuf,
    title: String,
    model: String,
    endpoint: String,
    out: Option<PathBuf>,
) -> anyhow::Result<()> {
    let doc_json = common::read_file_capped(&input)?;
    let client = fd_ai_adapter::FusionMlxClient::with_endpoints(
        fd_ai_adapter::FusionMlxClient::resolve_endpoint(&endpoint)?,
    )?;
    let skills = fd_ai_adapter::DesignSkills::new(client, model);
    let spec = skills.spec_doc_async(&doc_json, &title).await?;
    let json = serde_json::to_string_pretty(&spec)?;
    match out {
        Some(p) => {
            tokio::fs::write(&p, &json).await?;
            println!("已生成设计规范文档到 {p:?}");
        }
        None => println!("{json}"),
    }
    Ok(())
}

pub async fn run_page_flow(
    flow: String,
    style_hint: String,
    model: String,
    endpoint: String,
    out: Option<PathBuf>,
) -> anyhow::Result<()> {
    let client = fd_ai_adapter::FusionMlxClient::with_endpoints(
        fd_ai_adapter::FusionMlxClient::resolve_endpoint(&endpoint)?,
    )?;
    let skills = fd_ai_adapter::DesignSkills::new(client, model);
    let docs = skills.page_flow_async(&flow, &style_hint).await?;
    let json = serde_json::to_string_pretty(&docs)?;
    match out {
        Some(p) => {
            tokio::fs::write(&p, &json).await?;
            println!("已生成 PageFlow 多页面文档到 {p:?}");
        }
        None => println!("{json}"),
    }
    Ok(())
}
```

- [ ] **Step 2: mod.rs 补声明 + main.rs 改 4 arm**

mod.rs 加 `pub mod ai_skill;`。main.rs:627-730 四 arm 改委托：
```rust
        Command::ImageToUi { sketch, hint, page, model, endpoint, out } =>
            commands::ai_skill::run_image_to_ui(sketch, hint, page, model, endpoint, out).await,
        Command::MultiVariants { prompt, page, styles, model, endpoint, out } =>
            commands::ai_skill::run_multi_variants(prompt, page, styles, model, endpoint, out).await,
        Command::SpecDoc { input, title, model, endpoint, out } =>
            commands::ai_skill::run_spec_doc(input, title, model, endpoint, out).await,
        Command::PageFlow { flow, style_hint, model, endpoint, out } =>
            commands::ai_skill::run_page_flow(flow, style_hint, model, endpoint, out).await,
```
删内联 arm 体。

- [ ] **Step 3: 编译 + 测试验证**

Run: `~/.cargo/bin/cargo +1.94 check -p fd-cli && ~/.cargo/bin/cargo +1.94 test -p fd-cli`
Expected: Finished + 全 PASS。

- [ ] **Step 4: commit**

```bash
git add crates/fd-cli/src/commands/ai_skill.rs crates/fd-cli/src/commands/mod.rs crates/fd-cli/src/main.rs
git commit -m "refactor(ARCH-3): 拆 ImageToUi/MultiVariants/SpecDoc/PageFlow 到 commands/ai_skill.rs

从 main.rs run() 内联 arm 提取，零逻辑改。PERF-4 tokio::fs::write 一并带入。"
```

### Task 8: ARCH-3 拆分 IO arm 组

**Files:**
- Create: `crates/fd-cli/src/commands/io_cmd.rs`
- Modify: `crates/fd-cli/src/commands/mod.rs`，`crates/fd-cli/src/main.rs:549-593,743-754,772-822`

**Interfaces:**
- Consumes: `crate::common`，`fd_export`，`fd_ai_adapter::html_to_pen_document`，`fd_codegen`，`crate::CodegenTargetArg`
- Produces: `pub async fn run_export_batch(...)`, `run_parse_html(...)`, `run_codegen(...)` — 各返 `anyhow::Result<()>`

- [ ] **Step 1: 创建 io_cmd.rs**

`crates/fd-cli/src/commands/io_cmd.rs`:
```rust
//! A-3：IO 子命令（ExportBatch/ParseHtml/Codegen）从 main.rs 拆出。

use crate::common;
use std::path::PathBuf;
use crate::CodegenTargetArg;

pub async fn run_export_batch(
    input: PathBuf,
    format: crate::export::ExportFormatArg,
    formats: Option<Vec<crate::export::ExportFormatArg>>,
    out: PathBuf,
) -> anyhow::Result<()> {
    let json = common::read_file_capped(&input)?;
    let doc: fd_canvas_core::PenDocument = serde_json::from_str(&json)?;
    let reg = common::build_registry(&doc);
    let fmt_list: Vec<fd_export::ExportFormat> = if let Some(ref fmts) = formats {
        fmts.iter().map(|f| f.clone().into()).collect()
    } else {
        vec![format.into()]
    };
    let mut total = 0;
    let mut failures: Vec<String> = Vec::new();
    for fmt in fmt_list {
        match fd_cli::export::Exporter::from_pen_document_with_tokens(&doc, fmt, &out, &reg) {
            Ok(files) => {
                tracing::info!(format = ?fmt, count = files.len(), "批量导出完成");
                total += files.len();
            }
            Err(e) => {
                let fmt_str = fmt.extension();
                tracing::warn!(format = fmt_str, error = %e, "批量导出该格式失败");
                failures.push(format!("{fmt_str}: {e}"));
            }
        }
    }
    println!("已批量导出 {} 个页面到 {out:?}", total);
    if !failures.is_empty() {
        eprintln!("批量导出部分失败 ({} 项):", failures.len());
        for f in &failures {
            eprintln!("  - {f}");
        }
        return Err(anyhow::anyhow!(
            "批量导出部分失败: 成功导出 {total} 页，{} 项格式失败",
            failures.len()
        ));
    }
    Ok(())
}

pub fn run_parse_html(input: Option<PathBuf>, page: Option<String>) -> anyhow::Result<()> {
    let html = match input {
        Some(p) => common::read_file_capped(&p)?,
        None => common::read_stdin_capped()?,
    };
    let page_name = page.as_deref().unwrap_or("Page");
    let doc = fd_ai_adapter::html_to_pen_document(&html, page_name)?;
    let json = serde_json::to_string_pretty(&doc)?;
    println!("{json}");
    Ok(())
}

pub fn run_codegen(
    input: Option<PathBuf>,
    target: CodegenTargetArg,
    component: String,
    out: Option<PathBuf>,
) -> anyhow::Result<()> {
    let json = match input {
        Some(p) => common::read_file_capped(&p)?,
        None => common::read_stdin_capped()?,
    };
    let doc: fd_canvas_core::PenDocument = serde_json::from_str(&json)?;
    let reg = common::build_registry(&doc);
    let doc = fd_codegen::resolve_tokens(&doc, &reg);
    let code = match target {
        CodegenTargetArg::Html => {
            use fd_codegen::Codegen;
            fd_codegen::HtmlCodegen.generate(&doc)
        }
        CodegenTargetArg::ReactTailwind => {
            use fd_codegen::Codegen;
            fd_codegen::ReactTailwindCodegen { component_name: component.clone() }.generate(&doc)
        }
        CodegenTargetArg::TailwindOnly => {
            use fd_codegen::Codegen;
            fd_codegen::TailwindOnlyCodegen.generate(&doc)
        }
        CodegenTargetArg::SwiftUi => {
            use fd_codegen::Codegen;
            fd_codegen::SwiftUiCodegen { view_name: component.clone() }.generate(&doc)
        }
    };
    match out {
        Some(p) => {
            tokio::fs::write(&p, &code).await?;
            tracing::info!("已导出代码到 {p:?}");
        }
        None => println!("{code}"),
    }
    Ok(())
}
```
**注**：`run_codegen` 含 `tokio::fs::write(...).await?` 但函数非 async —— **改 `pub async fn run_codegen`**。`run_parse_html` 同步（无文件写）。`ExportFormatArg` 类型名须读 main.rs `Command::ExportBatch` 核实。`fd_cli::export::Exporter` 路径须核实（main.rs:523 `use fd_cli::{design, export, host}`）。

- [ ] **Step 2: mod.rs 补声明 + main.rs 改 3 arm**

mod.rs 加 `pub mod io_cmd;`。main.rs 改：
```rust
        Command::ExportBatch { input, format, formats, out } =>
            commands::io_cmd::run_export_batch(input, format, formats, out).await,
        Command::ParseHtml { input, page } =>
            commands::io_cmd::run_parse_html(input, page),
        Command::Codegen { input, target, component, out } =>
            commands::io_cmd::run_codegen(input, target, component, out).await,
```

- [ ] **Step 3: 编译 + 测试验证**

Run: `~/.cargo/bin/cargo +1.94 check -p fd-cli && ~/.cargo/bin/cargo +1.94 test -p fd-cli`
Expected: Finished + 全 PASS。修类型/路径不匹配。

- [ ] **Step 4: commit**

```bash
git add crates/fd-cli/src/commands/io_cmd.rs crates/fd-cli/src/commands/mod.rs crates/fd-cli/src/main.rs
git commit -m "refactor(ARCH-3): 拆 ExportBatch/ParseHtml/Codegen 到 commands/io_cmd.rs"
```

### Task 9: ARCH-3 拆分 history arm 组

**Files:**
- Create: `crates/fd-cli/src/commands/history.rs`
- Modify: `crates/fd-cli/src/commands/mod.rs`，`crates/fd-cli/src/main.rs:823-876`

**Interfaces:**
- Consumes: `crate::common`，`fd_canvas_core::UndoRedoStack`
- Produces: `pub async fn run_undo(input: PathBuf)`, `run_redo(input: PathBuf)`, `run_diff(old: PathBuf, new: PathBuf)` — 各返 `anyhow::Result<()>`

- [ ] **Step 1: 创建 history.rs**

`crates/fd-cli/src/commands/history.rs`:
```rust
//! A-3：历史子命令（Undo/Redo/Diff）从 main.rs 拆出。

use crate::common;
use std::path::PathBuf;

pub async fn run_undo(input: PathBuf) -> anyhow::Result<()> {
    let history_path = input.with_extension("history.json");
    if !history_path.exists() {
        anyhow::bail!("历史文件不存在: {history_path:?}");
    }
    let hist_json = common::read_file_capped(&history_path)?;
    let mut stack: fd_canvas_core::UndoRedoStack = serde_json::from_str(&hist_json)
        .map_err(|e| anyhow::anyhow!("历史文件解析失败: {e}"))?;
    match stack.undo() {
        Some(doc) => {
            let out_json = serde_json::to_string_pretty(&doc)?;
            let hist_out = serde_json::to_string_pretty(&stack)?;
            tokio::fs::write(&history_path, &hist_out).await?;
            println!("{out_json}");
            tracing::info!("undo: 成功回退");
            Ok(())
        }
        None => anyhow::bail!("无法撤销：已到最早状态"),
    }
}

pub async fn run_redo(input: PathBuf) -> anyhow::Result<()> {
    let history_path = input.with_extension("history.json");
    if !history_path.exists() {
        anyhow::bail!("历史文件不存在: {history_path:?}");
    }
    let hist_json = common::read_file_capped(&history_path)?;
    let mut stack: fd_canvas_core::UndoRedoStack = serde_json::from_str(&hist_json)
        .map_err(|e| anyhow::anyhow!("历史文件解析失败: {e}"))?;
    match stack.redo() {
        Some(doc) => {
            let out_json = serde_json::to_string_pretty(&doc)?;
            let hist_out = serde_json::to_string_pretty(&stack)?;
            tokio::fs::write(&history_path, &hist_out).await?;
            println!("{out_json}");
            tracing::info!("redo: 成功重做");
            Ok(())
        }
        None => anyhow::bail!("无法重做：已到最新状态"),
    }
}

pub fn run_diff(old: PathBuf, new: PathBuf) -> anyhow::Result<()> {
    let old_json = common::read_file_capped(&old)?;
    let new_json = common::read_file_capped(&new)?;
    let old_doc: fd_canvas_core::PenDocument = serde_json::from_str(&old_json)?;
    let new_doc: fd_canvas_core::PenDocument = serde_json::from_str(&new_json)?;
    let diff = old_doc.diff(&new_doc);
    let output = serde_json::to_string_pretty(&diff)?;
    println!("{output}");
    Ok(())
}
```

- [ ] **Step 2: mod.rs 补声明 + main.rs 改 3 arm**

mod.rs 加 `pub mod history;`。main.rs:823-876 改委托。

- [ ] **Step 3: 编译 + 测试 + commit**

```bash
~/.cargo/bin/cargo +1.94 check -p fd-cli && ~/.cargo/bin/cargo +1.94 test -p fd-cli
git add crates/fd-cli/src/commands/history.rs crates/fd-cli/src/commands/mod.rs crates/fd-cli/src/main.rs
git commit -m "refactor(ARCH-3): 拆 Undo/Redo/Diff 到 commands/history.rs"
```

### Task 10: ARCH-3 拆分 misc arm 组

**Files:**
- Create: `crates/fd-cli/src/commands/misc.rs`
- Modify: `crates/fd-cli/src/commands/mod.rs`，`crates/fd-cli/src/main.rs:731-741,891-908`

**Interfaces:**
- Consumes: `crate::host::HostBridgeConfig`，`fd_ecosystem::TrainerClient`
- Produces: `pub fn run_check_frontend(...)`, `run_train(...)` — 各返 `anyhow::Result<()>`

- [ ] **Step 1: 创建 misc.rs**

`crates/fd-cli/src/commands/misc.rs`:
```rust
//! A-3：杂项子命令（CheckFrontend/Train）从 main.rs 拆出。

use crate::host;

pub fn run_check_frontend(dir: std::path::PathBuf, backend: String) -> anyhow::Result<()> {
    host::HostBridgeConfig {
        frontend_dir: dir,
        backend_endpoint: backend,
        block_external: true,
    }
    .validate()?;
    println!("前端目录校验通过");
    Ok(())
}

pub fn run_train(
    dataset: String,
    model: String,
    method: String,
    config: Option<String>,
) -> anyhow::Result<()> {
    let trainer = fd_ecosystem::TrainerClient::new();
    let status = match method.as_str() {
        "grpo" => trainer.run_rlsl("grpo", &dataset, &model, config.as_deref())?,
        "sft" => trainer.run_sft(&dataset, &model, config.as_deref())?,
        other => anyhow::bail!("不支持的 --method: {other} (仅 sft|grpo)"),
    };
    if !status.success() {
        anyhow::bail!("fusion-trainer 退出码 {}", status.code().unwrap_or(-1));
    }
    Ok(())
}
```
**注**：`status.code().unwrap_or(-1)` 在非 test `src/`——`unwrap_or` 非 `unwrap()`，allowlist 门禁不拦。保留。

- [ ] **Step 2: mod.rs 补声明 + main.rs 改 2 arm + 编译测试 + commit**

mod.rs 加 `pub mod misc;`。main.rs:731-741（CheckFrontend）、891-908（Train）改委托。
```bash
~/.cargo/bin/cargo +1.94 check -p fd-cli && ~/.cargo/bin/cargo +1.94 test -p fd-cli
git add crates/fd-cli/src/commands/misc.rs crates/fd-cli/src/commands/mod.rs crates/fd-cli/src/main.rs
git commit -m "refactor(ARCH-3): 拆 CheckFrontend/Train 到 commands/misc.rs — 13 内联 arm 全拆完"
```

### Checkpoint 档 2

- [ ] **全量门禁**（同档 1 命令集）。`main.rs` 应从 1104 行降到 ~600 行。全绿才进档 3。

---

## 档 3 — ARCH-10 round 2/3 + ARCH-11 + FUNC-7（高风险结构改）

> **回退底线**：档 1+2 必完成。档 3 逐项独立，单项失败→回退该项留 TODO，不影响前两档。

### Task 11: ARCH-10 round2 — host-web bridge.rs 拆分

**Files:**
- Create: `crates/fd-host-web/src/bridge.rs`
- Modify: `crates/fd-host-web/src/lib.rs:336-789`

**Interfaces:**
- Consumes: `SHELL` mutex（crate 级 static），`render_dom`/`render_page`（须 `pub(crate)`），`thread_local` LOG_RING
- Produces: `pub(crate) fn handle_host_message(...)`, `setup_message_listener(...)` — 供 lib.rs 调用

- [ ] **Step 1: 读 bridge 完整代码段**

Read `crates/fd-host-web/src/lib.rs:336-789` 全段。识别 `handle_host_message` + `setup_message_listener` + `dispatch_bridge` 及其依赖的 crate 级 item（SHELL、render_dom、render_page、thread_local、parse 辅助）。

- [ ] **Step 2: 暴露被依赖 item `pub(crate)`**

在 lib.rs 把 bridge 依赖的 `fn render_dom`、`fn render_page`、需要的 parse 辅助加 `pub(crate)`。逐项加，最小化暴露面。

- [ ] **Step 3: 创建 bridge.rs + 迁移代码**

`crates/fd-host-web/src/bridge.rs` 顶部 `use crate::*;`（或精确 `use crate::{SHELL, render_dom, render_page, ...};`）。迁移 `handle_host_message`/`setup_message_listener`/`dispatch_bridge` 代码体（零逻辑改，仅迁位置）。

- [ ] **Step 4: lib.rs 加模块声明 + 删内联代码**

lib.rs 顶部加 `mod bridge;`（或 `pub(crate) mod bridge;`）。删 336-789 内联代码，替换为 `pub(crate) use bridge::{handle_host_message, setup_message_listener};` 或在调用站改 `bridge::handle_host_message(...)`。

- [ ] **Step 5: 编译验证**

Run: `~/.cargo/bin/cargo +1.94 check -p fd-host-web`
Expected: Finished。失败即修可见性/import；3 次修不通→回退 Step 6。

- [ ] **Step 6（回退）：编译失败超 3 次**

删 bridge.rs，恢复 lib.rs 内联代码，留：
```rust
// ── ARCH-10 round2 TODO: bridge 模块跨模块可见性断裂，留内联独立 PR 拆 ──
```
进 Task 12。

- [ ] **Step 7: native test + wasm build + harness 验证**

```bash
~/.cargo/bin/cargo +1.94 test -p fd-host-web   # 55 native 测试全过
~/.cargo/bin/cargo +1.94 build -p fd-host-web --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir target/wasm32-unknown-unknown/debug target/wasm32-unknown-unknown/debug/fd_host_web.wasm
node Scripts/wasm-harness-check.mjs   # 8 用例/13 断言全 PASS
```
Expected: 全 PASS。失败→回退 Step 6。

- [ ] **Step 8: commit**

```bash
git add crates/fd-host-web/src/bridge.rs crates/fd-host-web/src/lib.rs
git commit -m "refactor(ARCH-10): 拆 host-web bridge 消息派发到 src/bridge.rs"
```

### Task 12: ARCH-10 round3 — render_dom.rs 拆分

**Files:**
- Create: `crates/fd-host-web/src/render_dom.rs`
- Modify: `crates/fd-host-web/src/lib.rs:789-1303`

**Interfaces:**
- Consumes: `cached_doc` 字段，`SHELL` mutex，DOM 创建辅助
- Produces: `pub(crate) fn render_dom(...)` — 供 bridge/调用站

- [ ] **Step 1: 读 render_dom 完整代码段**

Read `crates/fd-host-web/src/lib.rs:789-1303` 全段。识别 `render_dom` 主体 + 它依赖的 crate 级 item（SHELL mutex、`cached_doc`/`cached_doc_json` 写站、DOM 节点创建辅助函数如 `create_node_element`/`apply_style` 等具名看代码）。

- [ ] **Step 2: 暴露被依赖 item `pub(crate)`**

在 lib.rs 把 render_dom 调用/引用的 `fn render_dom` 及其依赖的 crate 级 DOM 辅助加 `pub(crate)`。逐项加，最小化暴露面（bridge.rs Task 11 已 `pub(crate)` 的不重复）。

- [ ] **Step 3: 创建 render_dom.rs + 迁移代码**

`crates/fd-host-web/src/render_dom.rs` 顶部 `use crate::{SHELL, /* DOM 辅助名 */};`。迁移 `render_dom` 主体 + DOM 节点创建辅助代码体（零逻辑改，仅迁位置）。保留所有 `web_sys::console::log` 日志。

- [ ] **Step 4: lib.rs 加模块声明 + 删内联代码**

lib.rs 顶部 mod 区加 `pub(crate) mod render_dom;`。删 789-1303 内联代码。调用站改 `render_dom::render_dom(...)` 或 `pub(crate) use render_dom::render_dom;` 后原调用站不变。

- [ ] **Step 5: 编译验证**

Run: `~/.cargo/bin/cargo +1.94 check -p fd-host-web`
Expected: Finished。失败优先补 `pub(crate)` 可见性；3 次修不通→Step 6 回退。

- [ ] **Step 6（回退）：编译失败超 3 次**

删 render_dom.rs，恢复 lib.rs 内联 789-1303，留：
```rust
// ── ARCH-10 round3 TODO: render_dom 模块跨模块可见性断裂，留内联独立 PR 拆 ──
```
进 Task 13。

- [ ] **Step 7: native test + wasm build + harness 验证**

```bash
~/.cargo/bin/cargo +1.94 test -p fd-host-web   # 55 native 测试全过
~/.cargo/bin/cargo +1.94 build -p fd-host-web --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir target/wasm32-unknown-unknown/debug target/wasm32-unknown-unknown/debug/fd_host_web.wasm
node Scripts/wasm-harness-check.mjs   # 8 用例/13 断言全 PASS
```
Expected: 全 PASS。失败→回退 Step 6。

- [ ] **Step 8: commit**

```bash
git add crates/fd-host-web/src/render_dom.rs crates/fd-host-web/src/lib.rs
git commit -m "refactor(ARCH-10): 拆 host-web render_dom 到 src/render_dom.rs"
```

### Task 13: ARCH-10 round3 — render_canvas.rs 拆分

**Files:**
- Create: `crates/fd-host-web/src/render_canvas.rs`
- Modify: `crates/fd-host-web/src/lib.rs:3384-3596`

**Interfaces:**
- Consumes: `SHELL` mutex，canvas 2D context，render_page 共用
- Produces: `pub(crate) fn render_page(...)`, `pub(crate) fn render_canvas(...)` — 供调用站

- [ ] **Step 1: 读 render_canvas 完整代码段**

Read `crates/fd-host-web/src/lib.rs:3384-3596` 全段。识别 `render_page` + canvas 2D 绘制（`fn draw_node_canvas`/`fn render_canvas` 等具名看代码）及其依赖的 crate 级 item（SHELL mutex、`cached_doc`/`cached_doc_json`、page/viewport 状态）。

- [ ] **Step 2: 暴露被依赖 item `pub(crate)`**

在 lib.rs 把 render_page/canvas 绘制依赖的 crate 级 fn 加 `pub(crate)`。注意 bridge.rs（Task 11）已声明 `pub(crate) fn render_page` 引用 → 若 render_page 迁到此模块，须更新 bridge.rs import 路径（`use crate::render_canvas::render_page;` 或保持 lib.rs re-export）。

- [ ] **Step 3: 创建 render_canvas.rs + 迁移代码**

`crates/fd-host-web/src/render_canvas.rs` 顶部 `use crate::{SHELL, /* canvas 依赖名 */};`。迁移 `render_page` + canvas 绘制代码体（零逻辑改）。保留 console::log 日志。

- [ ] **Step 4: lib.rs 加模块声明 + 删内联 + 修 bridge.rs import**

lib.rs mod 区加 `pub(crate) mod render_canvas;`。删 3384-3596 内联。若 bridge.rs（Task 11）原 `use crate::render_page` → 改 `use crate::render_canvas::render_page;`（或在 lib.rs `pub(crate) use render_canvas::render_page;` re-export 保持调用站不变）。

- [ ] **Step 5: 编译验证**

Run: `~/.cargo/bin/cargo +1.94 check -p fd-host-web`
Expected: Finished。失败优先补 `pub(crate)` + 修 bridge.rs import；3 次修不通→Step 6 回退。

- [ ] **Step 6（回退）：编译失败超 3 次**

删 render_canvas.rs，恢复 lib.rs 内联 3384-3596，留：
```rust
// ── ARCH-10 round3 TODO: render_canvas 模块跨模块可见性断裂，留内联独立 PR 拆 ──
```
进 Task 14。

- [ ] **Step 7: native test + wasm build + harness 验证**

```bash
~/.cargo/bin/cargo +1.94 test -p fd-host-web   # 55 native 测试全过
~/.cargo/bin/cargo +1.94 build -p fd-host-web --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir target/wasm32-unknown-unknown/debug target/wasm32-unknown-unknown/debug/fd_host_web.wasm
node Scripts/wasm-harness-check.mjs   # 8 用例/13 断言全 PASS
```
Expected: 全 PASS。失败→回退 Step 6。

- [ ] **Step 8: commit**

```bash
git add crates/fd-host-web/src/render_canvas.rs crates/fd-host-web/src/lib.rs crates/fd-host-web/src/bridge.rs
git commit -m "refactor(ARCH-10): 拆 host-web render_canvas/render_page 到 src/render_canvas.rs"
```

### Task 14: ARCH-10 round3 — events.rs 拆分（最高风险）

**Files:**
- Create: `crates/fd-host-web/src/events.rs`
- Modify: `crates/fd-host-web/src/lib.rs:1433-2472`

**Interfaces:**
- Consumes: `thread_local` ACTIVE_DRAG_MOVE/PENDING_DRAG_UP/RESIZE_HANDLES（DragMoveClosure），`SHELL` mutex，render 调用
- Produces: `pub(crate) fn setup_events(...)` 或各 `setup_drag`/`setup_click`/`setup_wheel` — 供 mount 调用

- [ ] **Step 1: 读 events 完整代码段 + thread_local 定义**

Read `crates/fd-host-web/src/lib.rs:1433-2472` 全段 + thread_local 定义（35-48, 143）。识别 Closure 绑定函数对 SHELL + thread_local 的引用面。

- [ ] **Step 2: 暴露 thread_local + 辅助 `pub(crate)`**

thread_local `static` 是 crate 级，跨模块文件同 crate 可见（`use crate::ACTIVE_DRAG_MOVE`）。须确认 thread_local 声明无 `pub` 限制。Closure 类型 `DragMoveClosure` 须 `pub(crate)`。

- [ ] **Step 3: 创建 events.rs + 迁移**

`crates/fd-host-web/src/events.rs` `use crate::{SHELL, ACTIVE_DRAG_MOVE, PENDING_DRAG_UP, RESIZE_HANDLES, DragMoveClosure, ...};`。迁移拖拽/点击/滚轮 Closure 绑定代码（零逻辑改）。

- [ ] **Step 4: lib.rs `mod events;` + 删内联 1433-2472**

- [ ] **Step 5: 编译验证**

Run: `~/.cargo/bin/cargo +1.94 check -p fd-host-web`
Expected: Finished。thread_local 跨模块可见性最易断裂——失败优先补 `pub(crate)`。

- [ ] **Step 6（回退）：编译失败超 3 次 或 wasm/harness 失败**

删 events.rs，恢复 lib.rs 内联，留：
```rust
// ── ARCH-10 round3 TODO: events 模块 Closure thread_local 跨模块可见性断裂，留内联独立 PR 拆 ──
```
**这是 spec 允许的回退**——events 最高风险，不强拆。

- [ ] **Step 7: native + wasm + harness 验证 + commit（若成功）**

```bash
~/.cargo/bin/cargo +1.94 test -p fd-host-web
~/.cargo/bin/cargo +1.94 build -p fd-host-web --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir target/wasm32-unknown-unknown/debug target/wasm32-unknown-unknown/debug/fd_host_web.wasm
node Scripts/wasm-harness-check.mjs
git add crates/fd-host-web/src/events.rs crates/fd-host-web/src/lib.rs
git commit -m "refactor(ARCH-10): 拆 host-web events Closure 到 src/events.rs"
```

### Task 15: ARCH-11 — fd-skills 新 crate + SkillContext trait 解耦

**Files:**
- Create: `crates/fd-skills/Cargo.toml`，`crates/fd-skills/src/lib.rs`，`crates/fd-skills/tests/skills_mock.rs`
- Modify: `crates/fd-ai-adapter/src/lib.rs`（移 skill 代码 1745-2532，impl SkillContext），`crates/fd-ai-adapter/Cargo.toml`（加 fd-skills dep），`crates/fd-ai-adapter/src/skills_tests.rs`（迁移），`Cargo.toml`（workspace 加 fd-skills）

**Interfaces:**
- Consumes: `fd-canvas-core::PenDocument`，`fd-design-system::DesignSystem`
- Produces: `fd_skills::DesignSkill` trait + `SkillContext` trait + `SkillRegistry` + `DesignSkills` + 7 skill impl + `SkillOutput`/`SpecDocument`/`InteractionSpec`/`ComponentSpec`/`ComponentProp`（从 adapter 迁）

- [ ] **Step 1: workspace Cargo.toml 加 fd-skills**

`Cargo.toml` `[workspace] members` 加 `"crates/fd-skills"`。

- [ ] **Step 2: 创建 fd-skills/Cargo.toml**

```toml
[package]
name = "fd-skills"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
authors.workspace = true
description = "Fusion-Design AI 设计 Skill — trait 化能力 + 注册中心，解耦 inference client"

[lib]
name = "fd_skills"
path = "src/lib.rs"

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }
fd-canvas-core = { path = "../fd-canvas-core" }
fd-design-system = { path = "../fd-design-system" }

[dev-dependencies]
tokio = { workspace = true }
```

- [ ] **Step 3: 创建 fd-skills/src/lib.rs — SkillContext trait + 迁移 skill 代码**

`crates/fd-skills/src/lib.rs`（迁移 fd-ai-adapter:1616-2532 的 skill 系统，SkillContext 改 trait）：
```rust
//! fd-skills：AI 设计 Skill 系统。trait 化能力 + SkillContext trait 解耦 inference client。
//! fd-ai-adapter impl SkillContext for FusionMlxClient 反向单向依赖（fd-skills ← adapter）。

use fd_canvas_core::PenDocument;
use fd_design_system::DesignSystem;

/// SkillContext trait：抽象 chat 能力，解耦 skill 与具体 inference client。
/// 对象安全（无泛型/Self），SkillRegistry 用 &dyn SkillContext 避免泛型蔓延。
pub trait SkillContext: Send + Sync {
    fn chat(&self, model: &str, sys: &str, user: &str, max_tokens: u32) -> anyhow::Result<String>;
    fn chat_async<'a>(
        &'a self,
        model: &'a str,
        sys: &'a str,
        user: &'a str,
        max_tokens: u32,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'a>,
    >;
    fn chat_with_image_async<'a>(
        &'a self,
        model: &'a str,
        sys: &'a str,
        user: &'a str,
        image_base64: &'a str,
        max_tokens: u32,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'a>,
    >;
    fn chat_with_image(
        &self,
        model: &str,
        sys: &str,
        user: &str,
        image_base64: &str,
        max_tokens: u32,
    ) -> anyhow::Result<String>;
    fn token_prompt_fragment(&self, design_system: Option<&DesignSystem>) -> Option<String>;
}
```
**注**：`token_prompt_fragment` 原是 SkillContext struct 方法（取 self.design_system 字段）。改 trait 后须把 `design_system` 作参数传入。各 skill `execute` 调用站同步改 `ctx.token_prompt_fragment(design_system)`。
迁移 `DesignSkill` trait（`execute`/`execute_async` 签名改 `ctx: &dyn SkillContext`）、`SkillRegistry`、`SkillOutput`/`SpecDocument`/`InteractionSpec`/`ComponentSpec`/`ComponentProp`、7 skill impl（TextToUi/ImageToUi/PartialEdit/LocalEdit/MultiVariants/SpecDoc/PageFlow）。`execute` 内 `ctx.chat(self.model, ...)` → `ctx.chat(model, ...)`（model 作参数传入，或 skill 持有 model 字段）。
**关键决策**：skill 持有 `model: String` 字段，`execute(&self, ctx: &dyn SkillContext)` 内用 `self.model`。`DesignSkills` 包装 `ctx: Arc<dyn SkillContext>` + `default_model`，调 `skill.execute(ctx.as_ref())`。
完整迁移代码量大——实施时逐 skill 迁，每迁一个 `cargo check -p fd-skills`。

- [ ] **Step 4: fd-ai-adapter impl SkillContext for FusionMlxClient**

`crates/fd-ai-adapter/src/lib.rs` 移除 1616-2532 skill 代码（保留 `FusionMlxClient` + `chat_sync`/`chat_async`/`chat_with_image`/`chat_with_image_sync`/`html_to_pen_document`/`parse_ui_json`）。加：
```rust
use fd_skills::SkillContext;

impl SkillContext for FusionMlxClient {
    fn chat(&self, model: &str, sys: &str, user: &str, max_tokens: u32) -> anyhow::Result<String> {
        self.chat_sync(model, sys, user, max_tokens)
    }
    fn chat_async<'a>(
        &'a self,
        model: &'a str,
        sys: &'a str,
        user: &'a str,
        max_tokens: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'a>> {
        Box::pin(async move { self.chat_async(model, sys, user, max_tokens).await })
    }
    // ... chat_with_image_async / chat_with_image / token_prompt_fragment 同理 ...
}
```
**注**：`FusionMlxClient` 须 `Send + Sync`（核实）。`DesignSkills` re-export from fd-skills 或 adapter 包装。

- [ ] **Step 5: fd-ai-adapter/Cargo.toml 加 fd-skills dep**

加 `fd-skills = { path = "../fd-skills" }`。

- [ ] **Step 6: fd-cli 调用站同步改**

main.rs/commands 用 `fd_ai_adapter::DesignSkills` → 若 DesignSkills re-export 自 fd-skills 则路径不变；若 adapter 包装则改 `fd_ai_adapter::DesignSkills` 内部用 `fd_skills::DesignSkills` + `Arc<FusionMlxClient> as Arc<dyn SkillContext>`。

- [ ] **Step 7: 迁移 skill 测试 + 加 mock 单测**

`crates/fd-ai-adapter/src/skills_tests.rs` skill trait/registry 测试迁 `crates/fd-skills/tests/skills_mock.rs`。加 mock：
```rust
// mock SkillContext，验证 7 skill prompt 构造 + parse 无真模型调用。
struct MockSkillContext { last_sys: std::sync::Mutex<String>, response: String }
impl SkillContext for MockSkillContext {
    fn chat(&self, _model: &str, sys: &str, _user: &str, _max: u32) -> anyhow::Result<String> {
        *self.last_sys.lock().unwrap() = sys.to_string();
        Ok(self.response.clone())
    }
    fn chat_async<'a>(
        &'a self,
        model: &'a str,
        sys: &'a str,
        user: &'a str,
        max_tokens: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'a>> {
        let res = self.chat(model, sys, user, max_tokens);
        Box::pin(async move { res })
    }
    fn chat_with_image_async<'a>(
        &'a self,
        model: &'a str,
        sys: &'a str,
        user: &'a str,
        _image: &'a str,
        max_tokens: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'a>> {
        let res = self.chat(model, sys, user, max_tokens);
        Box::pin(async move { res })
    }
    fn chat_with_image(
        &self,
        model: &str,
        sys: &str,
        user: &str,
        _image: &str,
        max_tokens: u32,
    ) -> anyhow::Result<String> {
        self.chat(model, sys, user, max_tokens)
    }
    fn token_prompt_fragment(&self, _ds: Option<&fd_design_system::DesignSystem>) -> Option<String> {
        None
    }
}
#[test]
fn text_to_ui_skill_prompt_constructed() {
    let mock = MockSkillContext { last_sys: Default::default(), response: r#"{"page":{...}}"#.into() };
    let skill = TextToUiSkill::new("qwen".into());
    let out = skill.execute(&mock, "登录页")?;
    assert!(matches!(out, SkillOutput::Document(_)));
    let sys = mock.last_sys.lock().unwrap();
    assert!(sys.contains("UI 生成器"));
}
```
**注**：`unwrap()` 在 `tests/` 目录——deny-unwrap-expect allowlist 对 tests 放行（核实门禁是否扫 tests/）。

- [ ] **Step 8: 编译 + 全量测试**

```bash
~/.cargo/bin/cargo +1.94 check --workspace
~/.cargo/bin/cargo +1.94 test --workspace
```
Expected: Finished + 0 failed。`&dyn SkillContext` 对象安全失败→回退 Step 9。

- [ ] **Step 9（回退）：trait 对象安全/泛型蔓延失败**

保留 adapter 内联 skill 代码，fd-skills crate 留空壳 + TODO：
```rust
// ── ARCH-11 TODO: SkillContext trait 对象安全受阻，skill 代码留 adapter 内联 ──
```
进 Task 16。

- [ ] **Step 10: commit**

```bash
git add crates/fd-skills/ crates/fd-ai-adapter/ Cargo.toml
git commit -m "refactor(ARCH-11): 新建 fd-skills crate，SkillContext trait 解耦 inference client

skill 系统从 fd-ai-adapter 迁出，SkillContext 改 trait（&dyn 对象安全避免泛型蔓延）。
adapter impl SkillContext for FusionMlxClient 反向单向依赖。fd-skills mock 单测验证 prompt 构造。"
```

### Task 16: FUNC-7 — delta undo 重写

> **命名避撞**：fd-canvas-core 已有 `PenDocumentDiff { entries: Vec<DiffEntry> }`（lib.rs:837，fd-cli `diff` 子命令展示用，字段级 JSON diff）+ `PenDocument::diff()`。本 Task 的 delta-undo 是节点级整节点回滚，概念不同 → 新结构命名 `UndoDelta` + `NodeChange`，**不复用 `PenDocumentDiff`/`NodeDiff` 名**（Rule 7：不平均两个模式）。

**Files:**
- Create: `crates/fd-canvas-core/src/undo_delta.rs`（UndoDelta 数据结构 + compute/apply/apply_reverse）
- Modify: `crates/fd-canvas-core/src/lib.rs:756-811`（UndoRedoStack 改 delta 栈 + mod 声明 + re-export），`crates/fd-cli/src/commands/history.rs`（Task 9 已建，改下游调用）
- Test: `crates/fd-canvas-core/tests/undo_delta.rs`（新）

**Interfaces:**
- Consumes: `PenDocument`（`pages: Vec<Page>`，`Page { id, name, width, height, nodes: Vec<PenNode> }`），`PenNode`（`id, kind, name, x, y, w, h, style, text, children, rotation, z_index`）
- Produces:
  - `pub struct UndoDelta { page_id: String, added: Vec<PenNode>, deleted: Vec<PenNode>, modified: Vec<NodeChange> }`
  - `pub struct NodeChange { node_id: String, old: PenNode, new: PenNode }`
  - `UndoDelta::compute(old: &PenDocument, new: &PenDocument) -> UndoDelta`
  - `UndoDelta::apply(&self, doc: &mut PenDocument)` / `UndoDelta::apply_reverse(&self, doc: &mut PenDocument)`
  - `UndoRedoStack::new(initial: PenDocument) -> Self`（破坏性：旧无参 `new()` 删）
  - `UndoRedoStack::push(&mut self, delta: UndoDelta)`（破坏性：旧 `push(PenDocument)` 删）
  - `UndoRedoStack::current(&self) -> &PenDocument`
  - `UndoRedoStack::undo(&mut self) -> Option<PenDocument>` / `redo(&mut self) -> Option<PenDocument>`（签名不变，语义改：返 current clone）

- [ ] **Step 1: 写失败测试**

`crates/fd-canvas-core/tests/undo_delta.rs`:
```rust
use fd_canvas_core::*;

fn doc_with_nodes(ids: &[&str]) -> PenDocument {
    let mut page = Page {
        id: "p0".into(), name: "P0".into(), width: 100.0, height: 100.0, nodes: vec![],
    };
    for id in ids {
        page.nodes.push(PenNode {
            id: (*id).into(), kind: NodeKind::Rect, name: (*id).into(),
            x: 0.0, y: 0.0, w: 10.0, h: 10.0, style: Default::default(),
            text: None, children: vec![], rotation: 0.0, z_index: 0,
        });
    }
    PenDocument { schema_version: 1, pages: vec![page], variables: None, active_design_system: None }
}

fn node_ids(doc: &PenDocument) -> Vec<String> {
    doc.pages.first().map(|p| p.nodes.iter().map(|n| n.id.clone()).collect()).unwrap_or_default()
}

#[test]
fn undo_reverses_add() {
    let mut stack = UndoRedoStack::new(doc_with_nodes(&["a"]));
    let new_doc = doc_with_nodes(&["a", "b"]);
    let delta = UndoDelta::compute(stack.current(), &new_doc);
    stack.push(delta);
    assert_eq!(node_ids(stack.current()), vec!["a".to_string(), "b".into()]);
    let undone = stack.undo().unwrap();
    assert_eq!(node_ids(&undone), vec!["a".to_string()]);
}

#[test]
fn undo_reverses_delete() {
    let mut stack = UndoRedoStack::new(doc_with_nodes(&["a", "b"]));
    let new_doc = doc_with_nodes(&["a"]);
    let delta = UndoDelta::compute(stack.current(), &new_doc);
    stack.push(delta);
    assert_eq!(node_ids(stack.current()), vec!["a".to_string()]);
    let undone = stack.undo().unwrap();
    assert_eq!(node_ids(&undone), vec!["a".to_string(), "b".into()]);
}

#[test]
fn undo_reverses_modify() {
    let mut doc0 = doc_with_nodes(&["a"]);
    doc0.pages[0].nodes[0].x = 5.0;
    let mut stack = UndoRedoStack::new(doc0.clone());
    let mut new_doc = doc0.clone();
    new_doc.pages[0].nodes[0].x = 99.0;
    let delta = UndoDelta::compute(stack.current(), &new_doc);
    stack.push(delta);
    assert_eq!(stack.current().pages[0].nodes[0].x, 99.0);
    let undone = stack.undo().unwrap();
    assert_eq!(undone.pages[0].nodes[0].x, 5.0);
}

#[test]
fn redo_reapplies_delta() {
    let mut stack = UndoRedoStack::new(doc_with_nodes(&["a"]));
    let new_doc = doc_with_nodes(&["a", "b", "c"]);
    stack.push(UndoDelta::compute(stack.current(), &new_doc));
    let _ = stack.undo().unwrap();
    assert_eq!(node_ids(stack.current()), vec!["a".to_string()]);
    let redone = stack.redo().unwrap();
    assert_eq!(node_ids(&redone), vec!["a".to_string(), "b".into(), "c".into()]);
}

#[test]
fn delta_stack_serde_roundtrip() {
    let mut stack = UndoRedoStack::new(doc_with_nodes(&["a"]));
    stack.push(UndoDelta::compute(stack.current(), &doc_with_nodes(&["a", "b"])));
    let json = serde_json::to_string(&stack).unwrap();
    let back: UndoRedoStack = serde_json::from_str(&json).unwrap();
    assert_eq!(node_ids(back.current()), vec!["a".to_string(), "b".into()]);
}

#[test]
fn old_snapshot_history_migrates_with_warn() {
    // 旧快照式序列化（无 current 字段，纯 VecDeque<PenDocument>）→ 反序列化失败。
    // 旧格式: {"undo_stack":[{...PenDocument...}],"redo_stack":[]}
    let old = r#"{"undo_stack":[{"schema_version":1,"pages":[{"id":"p0","name":"P0","width":100,"height":100,"nodes":[{"id":"a","kind":"rect","name":"a","x":0,"y":0,"w":10,"h":10,"rotation":0,"z_index":0}]}],"variables":null,"active_design_system":null}],"redo_stack":[]}"#;
    let result: Result<UndoRedoStack, _> = serde_json::from_str(old);
    assert!(result.is_err(), "旧快照式格式应反序列化失败（缺 current 字段）触发迁移");
}
```

- [ ] **Step 2: 跑测试验证失败**

Run: `~/.cargo/bin/cargo +1.94 test -p fd-canvas-core --test undo_delta`
Expected: 编译失败（`UndoDelta`/`NodeChange` 不存在，`UndoRedoStack::new(PenDocument)` 签名不匹配）。

- [ ] **Step 3: 创建 undo_delta.rs — UndoDelta + NodeChange + compute + apply/apply_reverse**

`crates/fd-canvas-core/src/undo_delta.rs`:
```rust
//! FUNC-7：delta undo 数据结构。替代快照式 UndoRedoStack 存整 PenDocument，
//! 改存节点级 UndoDelta（增/删/改），节省内存。move 按 delete+add 处理（MVP，丢 reorder 语义）。
//!
//! 与 lib.rs 既有的 PenDocumentDiff（字段级 JSON 展示 diff，fd-cli diff 子命令用）是不同概念，
//! 故独立命名 UndoDelta/NodeChange，不复用其名。

use serde::{Deserialize, Serialize};
use crate::{PenDocument, PenNode};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoDelta {
    pub page_id: String,
    pub added: Vec<PenNode>,
    pub deleted: Vec<PenNode>,
    pub modified: Vec<NodeChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeChange {
    pub node_id: String,
    pub old: PenNode,
    pub new: PenNode,
}

impl UndoDelta {
    /// 计算 old → new 的节点级 delta。单页 MVP（取首页 nodes 对比）。
    pub fn compute(old: &PenDocument, new: &PenDocument) -> Self {
        let old_nodes = active_nodes(old);
        let new_nodes = active_nodes(new);
        let mut added = Vec::new();
        let mut deleted = Vec::new();
        let mut modified = Vec::new();
        let old_map: std::collections::HashMap<&str, &PenNode> =
            old_nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        let new_map: std::collections::HashMap<&str, &PenNode> =
            new_nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        for n in new_nodes {
            match old_map.get(n.id.as_str()) {
                None => added.push(n.clone()),
                Some(old_n) if **old_n != *n => modified.push(NodeChange {
                    node_id: n.id.clone(),
                    old: (*old_n).clone(),
                    new: n.clone(),
                }),
                _ => {}
            }
        }
        for n in old_nodes {
            if !new_map.contains_key(n.id.as_str()) {
                deleted.push(n.clone());
            }
        }
        let page_id = new.pages.first().map(|p| p.id.clone()).unwrap_or_default();
        UndoDelta { page_id, added, deleted, modified }
    }

    /// 正向应用 delta 到 doc（push/redo 用）。增→加节点，删→移节点，改→替换节点。
    pub fn apply(&self, doc: &mut PenDocument) {
        let page = match doc.pages.iter_mut().find(|p| p.id == self.page_id) {
            Some(p) => p,
            None => {
                tracing::warn!(page_id = %self.page_id, "UndoDelta apply 目标页不存在，跳过");
                return;
            }
        };
        for n in &self.deleted {
            remove_node_by_id(&mut page.nodes, &n.id);
        }
        for ch in &self.modified {
            replace_node_by_id(&mut page.nodes, &ch.node_id, ch.new.clone());
        }
        for n in &self.added {
            page.nodes.push(n.clone());
        }
    }

    /// 反向应用 delta（undo 用）。增→移，删→加，改→换回 old。
    pub fn apply_reverse(&self, doc: &mut PenDocument) {
        let page = match doc.pages.iter_mut().find(|p| p.id == self.page_id) {
            Some(p) => p,
            None => {
                tracing::warn!(page_id = %self.page_id, "UndoDelta apply_reverse 目标页不存在，跳过");
                return;
            }
        };
        // 逆序撤销：先移 added、再换回 modified.old、最后补回 deleted。
        for n in &self.added {
            remove_node_by_id(&mut page.nodes, &n.id);
        }
        for ch in &self.modified {
            replace_node_by_id(&mut page.nodes, &ch.node_id, ch.old.clone());
        }
        for n in &self.deleted {
            page.nodes.push(n.clone());
        }
    }
}

fn active_nodes(doc: &PenDocument) -> &[PenNode] {
    doc.pages.first().map(|p| p.nodes.as_slice()).unwrap_or(&[])
}

fn remove_node_by_id(nodes: &mut Vec<PenNode>, id: &str) {
    nodes.retain(|n| n.id != id);
    for n in nodes.iter_mut() {
        if !n.children.is_empty() {
            remove_node_by_id(&mut n.children, id);
        }
    }
}

fn replace_node_by_id(nodes: &mut Vec<PenNode>, id: &str, replacement: PenNode) {
    for n in nodes.iter_mut() {
        if n.id == id {
            *n = replacement;
            return;
        }
        if !n.children.is_empty() {
            replace_node_by_id(&mut n.children, id, replacement.clone());
        }
    }
}
```
**注**：`PenNode` 须 `PartialEq` —— lib.rs:129 现 `#[derive(Debug, Clone, Serialize, Deserialize)]` 无 `PartialEq`。compute 内 `**old_n != *n` 比较需要 `PenNode: PartialEq`。NodeStyle 等子结构也须 `PartialEq`。**Step 3 前先在 lib.rs 给 `PenNode`/`NodeStyle`/`NodeKind`(已有 Eq) 加 `PartialEq`**（NodeKind 已 Copy+Eq，PenNode/NodeStyle 加 derive）。核实 NodeStyle derive 行（读 lib.rs NodeStyle 定义确认无 `PartialEq` 则加）。

- [ ] **Step 4: UndoRedoStack 改 delta 栈**

`crates/fd-canvas-core/src/lib.rs:756-811` 改为：
```rust
// ── 撤销/重做栈 ──

mod undo_delta;
pub use undo_delta::{UndoDelta, NodeChange};

const UNDO_REDO_MAX_DEPTH: usize = 50;

// FUNC-7：delta 栈替代快照栈。存 UndoDelta + 当前态 current，省内存。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoRedoStack {
    undo_stack: VecDeque<UndoDelta>,
    redo_stack: VecDeque<UndoDelta>,
    current: PenDocument,
}

impl UndoRedoStack {
    pub fn new(initial: PenDocument) -> Self {
        Self { undo_stack: VecDeque::new(), redo_stack: VecDeque::new(), current: initial }
    }

    pub fn current(&self) -> &PenDocument {
        &self.current
    }

    pub fn push(&mut self, delta: UndoDelta) {
        if self.undo_stack.len() >= UNDO_REDO_MAX_DEPTH {
            self.undo_stack.pop_front();
        }
        delta.apply(&mut self.current);
        self.undo_stack.push_back(delta);
        self.redo_stack.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo(&mut self) -> Option<PenDocument> {
        let delta = self.undo_stack.pop_back()?;
        delta.apply_reverse(&mut self.current);
        self.redo_stack.push_back(delta);
        Some(self.current.clone())
    }

    pub fn redo(&mut self) -> Option<PenDocument> {
        let delta = self.redo_stack.pop_back()?;
        delta.apply(&mut self.current);
        self.undo_stack.push_back(delta);
        Some(self.current.clone())
    }
}

impl Default for UndoRedoStack {
    fn default() -> Self {
        Self::new(PenDocument::default())
    }
}
```
**破坏性 API 变更**（内部消费无外部调用方）：
- `new()` 无参 → `new(PenDocument)` 有参（Default impl 保留无参路径用 `PenDocument::default()`）。
- `push(PenDocument)` → `push(UndoDelta)`。
- `undo()` 语义改：旧需 len≥2（快照栈无当前态概念），新 pop delta + apply_reverse 即可（current 始终在）。`can_undo` 仍 `!undo_stack.is_empty()` 一致。
- 下游调用站（fd-cli history.rs Task 9 + lib.rs 内 `DiffEntry` 等若有引用）须同步改。grep 确认全调用站：`grep -rn "UndoRedoStack" crates/`。

- [ ] **Step 5: 下游 fd-cli history.rs 同步 + 旧格式迁移**

`crates/fd-cli/src/commands/history.rs`（Task 9 已建）改。原 run_undo/run_redo 反序列化 `UndoRedoStack` 后调 `stack.undo()`。delta 栈需 `current` 初始态——history.json 现已含 `current` 字段（新格式）。旧快照式 history.json（无 current）反序列化失败 → catch + warn 重建（用当前 `.fusiondesign` doc 作 initial）。
改 run_undo：
```rust
pub async fn run_undo(input: PathBuf) -> anyhow::Result<()> {
    let history_path = input.with_extension("history.json");
    if !history_path.exists() {
        anyhow::bail!("历史文件不存在: {history_path:?}");
    }
    let hist_json = common::read_file_capped(&history_path)?;
    let mut stack: fd_canvas_core::UndoRedoStack = serde_json::from_str(&hist_json)
        .or_else(|e| {
            // FUNC-7：旧快照式 history（VecDeque<PenDocument>，无 current 字段）不兼容 delta 栈。
            // 破坏性接受：丢弃旧 history，用当前 .fusiondesign doc 重建空栈，用户须重新操作。
            tracing::warn!(error = %e, "旧快照式 history 不兼容 delta 栈，丢弃历史重建");
            let doc_json = common::read_file_capped(&input).unwrap_or_default();
            let doc: fd_canvas_core::PenDocument =
                serde_json::from_str(&doc_json).unwrap_or_default();
            Ok(fd_canvas_core::UndoRedoStack::new(doc))
        })?;
    match stack.undo() {
        Some(doc) => {
            let out_json = serde_json::to_string_pretty(&doc)?;
            let hist_out = serde_json::to_string_pretty(&stack)?;
            tokio::fs::write(&history_path, &hist_out).await?;
            println!("{out_json}");
            tracing::info!("undo: 成功回退");
            Ok(())
        }
        None => anyhow::bail!("无法撤销：已到最早状态"),
    }
}
```
run_redo 同理（`stack.redo()` + 写回 + 打印）。`.unwrap_or_default()` 在非 test `src/`——**deny-unwrap-expect allowlist 须加这两处**（见 Step 7），或改 `match` 避 unwrap（**优先改 match 避 allowlist 膨胀**）：
```rust
            let doc_json = match common::read_file_capped(&input) {
                Ok(s) => s,
                Err(_) => String::new(),
            };
            let doc: fd_canvas_core::PenDocument = serde_json::from_str(&doc_json)
                .unwrap_or_else(|_| fd_canvas_core::PenDocument::default());
```
`unwrap_or_else` 非 `unwrap()`，allowlist 不拦。保留。

- [ ] **Step 6: 跑 canvas-core 测试 + 编译**

Run: `~/.cargo/bin/cargo +1.94 test -p fd-canvas-core`
Expected: 新 6 测试 PASS + 现有全过。修 apply/apply_reverse 正确性（删/改/增顺序、嵌套 children）。

- [ ] **Step 7: deny-unwrap-expect allowlist 核查**

Run: `Scripts/deny-unwrap-expect.sh`
Expected: clean（本 Task 用 `unwrap_or_else`/`match` 避开 `unwrap()`）。若 history.rs 残留 `unwrap()` → 改 match 或加 allowlist 条目（记入 `docs/changes/v0.1.15.md` 余 TODO）。

- [ ] **Step 8: 全 workspace 回归（UndoRedoStack 破坏性 API 下游全同步）**

Run: `~/.cargo/bin/cargo +1.94 test --workspace`
Expected: 0 failed。grep 确认无遗漏调用站：`grep -rn "UndoRedoStack::new\|\.push(.*PenDocument\|UndoRedoStack::default" crates/`。

- [ ] **Step 9（回退）：diff 计算正确性反复失败 / 复杂度超 budget**

保留快照式 UndoRedoStack（恢复 lib.rs:756-811 原状），undo_delta.rs 留空壳 + TODO：
```rust
// ── FUNC-7 TODO: UndoDelta delta undo 复杂度超 budget，保留快照式 ──
```
history.rs 恢复 Task 9 快照式版本。**回退后档 3 仍算 T11-15 落地，FUNC-7 留 TODO 不阻断最终门禁**（spec 允许）。

- [ ] **Step 10: commit**

```bash
git add crates/fd-canvas-core/src/undo_delta.rs crates/fd-canvas-core/src/lib.rs crates/fd-cli/src/commands/history.rs crates/fd-canvas-core/tests/undo_delta.rs
git commit -m "feat(FUNC-7): delta undo UndoDelta 替代快照式，节点级回滚省内存

UndoRedoStack 改 VecDeque<UndoDelta> + current 态。UndoDelta compute 节点级增删改，
apply/apply_reverse 正反向回滚。move 按 delete+add MVP。PenNode/NodeStyle 加 PartialEq。
旧快照式 history 不兼容 delta 栈，warn 重建（破坏性接受，审计 §10 允许）。
与既有 PenDocumentDiff（字段级展示 diff）概念不同，独立命名 UndoDelta/NodeChange。"
```

- [ ] **Step 9: commit**

```bash
git add crates/fd-canvas-core/src/diff.rs crates/fd-canvas-core/src/lib.rs crates/fd-cli/src/commands/history.rs crates/fd-canvas-core/tests/undo_delta.rs
git commit -m "feat(FUNC-7): delta undo PenDocumentDiff 替代快照式，节省内存

UndoRedoStack 改 VecDeque<PenDocumentDiff> + current 态。move 按 delete+add MVP。
旧快照式 history 格式不兼容，warn 重建（破坏性接受，审计 §10 允许）。
diff compute/undo/redo/序列化往返/旧格式迁移单测覆盖。"
```

### Checkpoint 档 3 + 最终全量门禁

- [ ] **全量门禁**：
```bash
~/.cargo/bin/cargo +1.94 test --workspace     # 0 failed（474 + 档 1-3 新增）
~/.cargo/bin/cargo +1.94 clippy --workspace --all-targets -- -D warnings
~/.cargo/bin/cargo +1.94 fmt --all -- --check
Scripts/deny-panic.sh && Scripts/deny-unwrap-expect.sh && Scripts/deny-external-http.sh
~/.cargo/bin/cargo +1.94 build -p fd-host-web --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir target/wasm32-unknown-unknown/debug target/wasm32-unknown-unknown/debug/fd_host_web.wasm
node Scripts/wasm-harness-check.mjs   # 8 用例/13 断言全 PASS
```
rtk 吞退出码：用输出文案（"0 failed"/"Finished"/无 "error["）校验非 `$?`。

---

## 验证（全档统一）

1. `~/.cargo/bin/cargo +1.94 test --workspace` — 全量回归（现 474 + 档 1-3 新增），0 failed。
2. `~/.cargo/bin/cargo +1.94 clippy --workspace --all-targets -- -D warnings` — clean（新增代码零 unwrap/expect/panic）。
3. `~/.cargo/bin/cargo +1.94 fmt --all -- --check` — 0 diff。
4. `Scripts/deny-panic.sh` + `Scripts/deny-unwrap-expect.sh` + `Scripts/deny-external-http.sh` — 三门禁过。
5. wasm build：`cargo +1.94 build -p fd-host-web --target wasm32-unknown-unknown` + wasm-bindgen — clean。
6. `node Scripts/wasm-harness-check.mjs` — 8 用例/13 断言全 PASS（档 3 ARCH-10 后必跑）。
7. 档 1 新增单测：FAULT-2 多字节切断、SEC-N3 路径穿越、FUNC-9 旋转、PERF-3 阈值降级。
8. 档 3 新增：fd-skills mock 单测、FUNC-7 diff compute/undo/redo/序列化往返/旧格式迁移。
9. rtk 吞退出码：用输出文案（"0 failed"/"Finished"/无 "error["）校验非 `$?`。
10. 档 3 ARCH-10 每移一模块：`cargo check -p fd-host-web` + native test 55 + wasm build + harness。

## 风险与回退

| 风险 | 缓解 | 回退 |
|------|------|------|
| ARCH-10 thread_local 跨模块可见性断裂 | 逐模块 cargo check + harness 验 | 回退该模块内联 + TODO divider |
| ARCH-11 SkillContext trait 对象安全/泛型蔓延 | 选 &dyn SkillContext 避免泛型 | 保留 adapter 内联 + TODO |
| FUNC-7 diff 计算正确性（嵌套/move） | 密集测试 + move 按 delete+add MVP | 保留快照式 + TODO |
| FUNC-7 旧 history 序列化不兼容 | warn 重建（破坏性接受） | 接受破坏（演进） |
| FUNC-9 printpdf default-features=false 无 Transform | Step 1 探针核实 | 保留 warn + 文档标注 |
| 全 9 项 token 超预算 | 3 档 checkpoint，每档 commit 可独立留存 | 档 3 逐项回退 |
| 纯机械迁移（ARCH-3）编译断裂 | 每 arm 组 cargo check | 回退该 arm 内联 |

**总体回退底线**：档 1（surgical）+ 档 2（ARCH-3）低风险必完成。档 3 逐项独立，单项失败回退该项留 TODO。最终状态 ≥ v0.1.14 + 档 1/2 + 档 3 部分项。

## 释放检查点

全部 9 项修完 + 验证全绿后，**仅用户明确指示**：
- `git tag -a v0.1.15 -m "<notes>"` + push 触发 CI。
- `docs/changes/v0.1.15.md` 补 changelog。
- tag/release + commit/push 仅在用户明确指示后执行（规则：commit/push only when user asks；if on default branch, branch first）。
