# P3 残留项 + ARCH-10 round 2/3 全修复设计

> 日期：2026-08-28
> 审计基线：`~/fusion/audit/fusion-design-audit-result-product-0827.md` §9.3 P3（8 项）+ §9.2 ARCH-10 round 2/3
> 范围：FAULT-2/PERF-3/PERF-4/SEC-N3/FUNC-9/FUNC-7/ARCH-3/ARCH-11/ARCH-10 round 2/3（9 项）
> 用户裁定：全 9 项一次做；按风险档递增 3 档 checkpoint；FUNC-7 真做 delta diff；ARCH-10 本轮顺带拆

## 背景

v0.1.14 闭合 §9.2 P2 全 14 项。残留：§9.3 P3 工程债 8 项 + ARCH-10 round 2/3（上版留 TODO divider）。
审计 §10/§11 明确 P3 + ARCH-10 可维护性债「非阻断商用」，但用户要求达更高工程完备。本轮全修。

## 执行档级（每档 checkpoint `cargo +1.94 test --workspace` 全绿才进下档）

### 档 1 — surgical + 中等（低中风险，单 crate 改）

### 档 2 — ARCH-3 fd-cli 16-arm 拆分（机械低风险）

### 档 3 — ARCH-10 round 2/3 + ARCH-11 + FUNC-7（高风险结构改）

## 档 1 详设

5 项 surgical/中等，各单 crate 改，回归风险低。

### FAULT-2 — stdin 字节边界损坏（fd-cli/src/common.rs:20-35）

**现状**：`read_stdin_capped` 用 `buf.push_str(&String::from_utf8_lossy(&chunk[..n]))`。
chunk 可能在多字节 UTF-8 字符中间切断 → from_utf8_lossy 用 U+FFFD 替换损坏字节，**数据损坏**。
注释（22-24）明写「改用 bytes 累加再 from_utf8 保证安全」但代码未改，与注释矛盾。

**修**：`buf: Vec<u8>` 累加原始 bytes，循环内只 `buf.extend_from_slice(&chunk[..n])` + 查 `buf.len() > CAP`。
循环结束 `String::from_utf8(buf)?` 一次性转（边界完整，无效 UTF-8 返 Err fail visibly，不静默替换）。
返回类型不变 `anyhow::Result<String>`。

**测试**：构造含多字节字符（如「中」3 字节）恰好被 chunk 切断的输入，断言输出完整无损。

### PERF-4 — async CLI handler 阻塞 worker（fd-cli/src/main.rs，7 处 std::fs::write）

**现状**：async `run()` 内 7 处 `std::fs::write(&p, &json)?`（646/677/701/724/816/836/857）同步阻塞 tokio worker。
单线程 runtime 时阻塞所有异步任务。

**修**：改 `tokio::fs::write(&p, &json).await?`。tokio::fs 是 async 文件 IO，不阻塞 worker。
fd-cli Cargo.toml 已有 tokio dep（含 fs feature？须核，若无加 `features=["fs"]`）。

**测试**：现有 undo/redo/export 测试覆盖写路径，验证仍正常。无须新测试（行为等价，仅非阻塞化）。

### SEC-N3 — sanitize_image_url 放行裸相对路径（fd-export/src/lib.rs:442-475）

**现状**：无协议前缀路径（如 `../../etc/passwd`）直接放行。离线 SVG 渲染无出网，但相对路径
可解析到文件系统敏感位置（理论风险，离线无升级路径，审计标 P3 理论）。

**修**：放行前检查含 `..` 段的路径。`if trimmed.contains("/../") || trimmed.starts_with("../")`
→ 拒绝返空串 + warn「拒含 .. 的路径穿越意图」。纯文件名/正常相对路径仍放行（本地资源）。

**测试**：`sanitize_image_url("../secret")` → 空串；`sanitize_image_url("logo.png")` → 放行。

### FUNC-9 — PDF 丢旋转（fd-export/src/lib.rs:725-760）

**现状**：非 0 旋转 shape 走 `other =>` 分支跳过 + warn。printpdf 0.12 `Op` 枚举无易用 per-shape transform。
注释 TODO「后续接 printpdf Transform」。

**修**：printpdf 0.12 有 `printpdf::ops::Op::Transform` 接 `printpdf::transforms::Matrix`。
对带旋转的 shape（rect/circle 等），在 shape ops 前插入 `Op::Transform(Matrix::rotate(angle, cx, cy))`
+ shape 后 `Op::Transform(Matrix::identity())` 重置（save/restore 语义）。
须核 printpdf 0.12 实际 API（Transform 变体 + Matrix 构造）。若 API 不可用则保留 warn + 文档标注 API 限制。

**测试**：构造 90° 旋转 rect doc → export PDF → 断言无 warn（或断言 skipped_kinds 不含旋转项）。

### PERF-3 — lint overlap O(n²) 无界兄弟组（fd-design-lint/src/lib.rs:528-553）

**现状**：`check_overlapping_nodes` 双重循环 `for i..for j..` O(n²)，兄弟组无上限时 worst-case 慢。
非热路径（lint 按需触发）但 worst-case 无界。

**修**：扫描线 O(n log n)：按 x 排序节点，sweep line + 活跃集（按 y 排序的 BTreeSet）检测相交。
或更简：兄弟组节点数 > 阈值（如 200）时降级——`if siblings.len() > 200 { warn + skip O(n²) }`，
小文档保持精确检测。权衡：扫描线实现复杂易错，阈值降级简单可靠 + warn fail visible。
**选阈值降级**（Rule 2 simplicity）：> 200 节点兄弟组 warn「节点过多跳过重叠检测」+ 跳过，
≤ 200 保持现有 O(n²) 精确检测（小文档 n² 可接受）。文档化 worst-case 退化。

**测试**：201 节点兄弟组 → warn + 0 violation；200 节点 → 正常检测。


## 档 2 详设

### ARCH-3 — fd-cli 余 16 内联 arm 拆 commands/*.rs（fd-cli/src/main.rs:526-929）

**现状**：6 arm 已拆 commands/（chat/check_mlx/export/generate/health/lint），余 ~13 内联：
ListDesignSystems/Activate/ExportBatch/ImageToUi/MultiVariants/SpecDoc/PageFlow/CheckFrontend/
ParseHtml/TokenCSS/Codegen/Undo/Redo/Diff/Theme/Train。main.rs 1104 行。

**修**：按既有模式每 arm 提 `commands/<name>.rs`，签名 `pub async fn run(...) -> anyhow::Result<()>`。
main.rs match arm 改 `Command::Foo { .. } => commands::foo::run(...).await,`。
common helper（rotate_backup/read_stdin_capped/parse_json 等）留 common.rs，commands `use crate::common::*`。
mod.rs 补 `pub mod <name>;` 声明。

**文件**：新增 ~13 个 commands/*.rs（list_design_systems.rs/activate.rs/export_batch.rs/image_to_ui.rs/
multi_variants.rs/spec_doc.rs/page_flow.rs/check_frontend.rs/parse_html.rs/token_css.rs/codegen.rs/
undo.rs/redo.rs/diff.rs/theme.rs/train.rs）。实际按 arm 体量合并小 arm（如 list_design_systems+activate
可合 design_system.rs），避免碎文件。

**风险**：纯机械迁移，零逻辑改。每移一 arm `cargo check -p fd-cli` 编译通过。
私有 item（main.rs 内 helper）迁移时须 `pub(crate)` 暴露给 commands。

**测试**：现有 integration.rs 3 用例 + 全 workspace test 覆盖。拆分后行为不变，test 应全过。


## 档 3 详设

3 项高风险结构改。每项独立 checkpoint，失败即回退该项。

### ARCH-10 round 2/3 — thread_local 重模块拆分（fd-host-web/src/lib.rs）

**现状**：lib.rs 3596 行。thread_local 全局态 4 个（ACTIVE_DRAG_MOVE/PENDING_DRAG_UP/RESIZE_HANDLES
拖拽 Closure，35-48；LOG_RING，143）。待拆模块：
- `bridge`（handle_host_message/setup_message_listener，336-789）— 消息派发
- `events`（拖拽/点击/滚轮 Closure 绑定，1433-2472）— Closure thread_local 纠缠
- `render_dom`（789-1303）— DOM 渲染
- `render_canvas`（3384-3596）— canvas 渲染
- `render_page`（3384）— 共用

**难点**：thread_local 是 crate 级 `static`，跨模块文件仍可见（同 crate，`use crate::ACTIVE_DRAG_MOVE`）。
但 Closure 绑定函数（events 内 setup_drag 等）引用 `SHELL` mutex + thread_local，跨模块迁移须
暴露足够 `pub(crate)` item。回归风险高——wasm 行为难本地测（须 harness 验）。

**修策略（保守）**：按自包含度递增拆，每移一模块 `cargo check -p fd-host-web` + `cargo test -p fd-host-web`（55 native 测试）+ wasm build + harness 13 断言验证：
1. `bridge.rs`：handle_host_message + setup_message_listener + dispatch_bridge。依赖 SHELL + render_dom/render_page 调用 → 须 `pub(crate) fn render_dom/render_page`。
2. `render_dom.rs`：render_dom + DOM 节点创建。依赖 cached_doc 写站。
3. `render_canvas.rs`：render_page + canvas 绘制。
4. `events.rs`：拖拽/点击/滚轮 Closure。依赖 thread_local + SHELL + render。**最高风险**——若跨模块可见性断裂，留该模块内联 + TODO，不强拆。

**回退**：任一模块迁移后 wasm build/test/harness 失败 → 回退该模块至 lib.rs 内联，留 TODO，进下项。

### ARCH-11 — fd-skills 新 crate（SkillRegistry/DesignSkills/DesignSkill trait）

**现状**：fd-ai-adapter/src/lib.rs 1746-2532 含 DesignSkill trait + SkillRegistry + 7 skill impl +
DesignSkills。skill `execute` 经 `ctx.chat()`（SkillContext 封装 client）。审计 §10 称「skills 与
inference client 紧耦合是设计固有」。

**修**：新 crate `fd-skills`（叶子 + 仅 fd-canvas-core 依赖），用 trait 抽象解耦：
- fd-skills 定义：`DesignSkill` trait + `SkillContext`（trait，抽象 `chat/chat_async`）+ `SkillRegistry`
  + `DesignSkills` + 7 skill impl（TextToUi/ImageToUi/PartialEdit/LocalEdit/SpecDoc/PageFlow/MultiVariants）。
  SkillContext 改 trait：`pub trait SkillContext { fn chat(&self, sys, user, max_tokens) -> Result<String>;
  fn chat_async(...) -> ...; fn token_prompt_fragment(&self) -> Option<String>; }`。
- fd-ai-adapter：impl `SkillContext for FusionMlxClient`（或包装类型），`use fd_skills::*`。
  adapter 反向依赖 fd-skills（单向：fd-skills ← adapter，fd-skills 不知 adapter）。
- workspace Cargo.toml 加 `"crates/fd-skills"`。
- fd-ai-adapter Cargo.toml 加 `fd-skills = { path = "../fd-skills" }`。

**风险**：SkillContext 从具体 struct 改 trait → 7 skill impl 签名全改（`ctx: &SkillContext` →
`ctx: &impl SkillContext` 或 `ctx: &dyn SkillContext`）。dyn vs impl 选择影响泛型蔓延。选 `&dyn SkillContext`
（对象安全，避免泛型蔓延到 SkillRegistry）。须核 SkillContext 方法对象安全（无泛型/Self）。

**测试**：fd-skills 加单测（mock SkillContext impl，验证 7 skill prompt 构造 + parse）。
adapter 现有 skill 测试迁 fd-skills + adapter 留 client 侧集成测试。

### FUNC-7 — delta undo 重写（fd-canvas-core/src/lib.rs:762-807）

**现状**：UndoRedoStack 快照式 VecDeque<PenDocument>，max 50。undo/redo 返全快照克隆。
超大文档 50×全文档内存缺口。

**修**：新 `PenDocumentDiff` 数据结构 + delta 栈：
```
pub struct PenDocumentDiff {
    pub page_id: String,
    pub added: Vec<PenNode>,      // 新增节点
    pub deleted: Vec<PenNode>,    // 删除节点（含原值，供 rollback）
    pub modified: Vec<NodeDiff>,  // 修改节点（old_style/new_style 等）
}
pub struct NodeDiff { pub node_id: String, pub old: PenNode, pub new: PenNode }
```
UndoRedoStack 改 `undo_stack: VecDeque<PenDocumentDiff>` + `current: PenDocument`（当前态）。
- `push(diff)`: 存 diff，不存全快照。
- `undo()`: pop diff → `apply_reverse(diff, &mut current)` → 返 current clone。
- `redo()`: pop redo_stack diff → `apply(diff, &mut current)` → 返 current clone。
- diff 计算：调用方传 `old_doc` + `new_doc` → `PenDocumentDiff::compute(old, new)` 递归比节点。

**下游同步**：fd-cli/src/main.rs:830/851 undo/redo arm 现从序列化 stack 调 `stack.undo()`。
delta 栈序列化 schema 变（VecDeque<PenDocumentDiff> vs VecDeque<PenDocument>）——**破坏性**。
旧 `.fusiondesign.history` 快照格式不兼容。须版本化 + 旧格式迁移（或丢弃旧 history 重建）。
审计 §10 称快照式「MVP 可接受」——delta 是演进，旧 history 不兼容可接受（warn + 重建）。

**风险**：diff 计算正确性（节点 id 匹配 + 嵌套 children diff）是核心难点。compute 须覆盖
add/delete/modify/move（reorder）。move 若按 delete+add 处理则丢失 reorder 语义（可接受 MVP）。
测试密集：add/delete/modify/undo/redo/序列化往返/旧格式迁移。

**回退**：若 diff 计算复杂度超 budget → 保留快照式 + 文档标注 TODO（用户已选真做，尽力完成）。


## 验证

每档完成跑全量门禁，全绿才进下档：

1. `~/.cargo/bin/cargo +1.94 test --workspace` — 全量回归（现 474 + 新增），0 failed。
2. `~/.cargo/bin/cargo +1.94 clippy --workspace --all-targets -- -D warnings` — clean。
3. `~/.cargo/bin/cargo +1.94 fmt --all -- --check` — 0 diff。
4. `Scripts/deny-panic.sh` + `deny-unwrap-expect.sh` + `deny-external-http.sh` — 三门禁过。
5. wasm build：`cargo +1.94 build -p fd-host-web --target wasm32-unknown-unknown` + wasm-bindgen — clean。
6. `node Scripts/wasm-harness-check.mjs` — 8 用例/13 断言全 PASS（档 3 ARCH-10 后必跑）。
7. 档 1 新增单测：FAULT-2 多字节切断、SEC-N3 路径穿越、FUNC-9 旋转、PERF-3 阈值降级。
8. 档 3 新增：fd-skills mock 单测、FUNC-7 diff compute/undo/redo/序列化往返/旧格式迁移。
9. rtk 吞退出码：用输出文案（"0 failed"/"Finished"/无 "error["）校验非 `$?`。

档 3 ARCH-10 每移一模块：`cargo check -p fd-host-web` + native test 55 + wasm build + harness。

## 风险与回退

| 风险 | 缓解 | 回退 |
|------|------|------|
| ARCH-10 thread_local 跨模块可见性断裂 | 逐模块 cargo check + harness 验 | 回退该模块内联 + TODO |
| ARCH-11 SkillContext trait 对象安全/泛型蔓延 | 选 &dyn SkillContext 避免泛型 | 保留 adapter 内联 + TODO |
| FUNC-7 diff 计算正确性（嵌套/move） | 密集测试 + move 按 delete+add MVP | 保留快照式 + TODO |
| FUNC-7 旧 history 序列化不兼容 | 版本化 + 旧格式 warn 重建 | 接受破坏（演进） |
| 全 9 项 token 超预算 | 3 档 checkpoint，每档 commit 可独立留存 | 档 3 逐项回退 |
| 纯机械迁移（ARCH-3）编译断裂 | 每 arm cargo check | 回退该 arm 内联 |

**总体回退底线**：档 1（surgical）+ 档 2（ARCH-3）低风险必完成。档 3 逐项独立，单项失败回退该项
留 TODO，不影响前两档已落地收益。最终状态 ≥ v0.1.14 + 档 1/2 + 档 3 部分项。

