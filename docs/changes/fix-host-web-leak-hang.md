# fd-host-web 内存泄漏与执行卡死修复

评审发现 `fd-host-web`（WASM 渲染层）存在多处内存泄漏与卡死病根，长期运行（尤其高频拖拽/MutateNode）必然导致 GUI 卡死。本修复针对核心问题。

## 修复项

### P0-1 容器级监听器每次渲染重复注册 + forget 累积（致卡死）

**位置**：`render_dom` 末尾的 6 个 `setup_*` 调用。

**病根**：每次 `render_dom`（由 `page.render-dom` / `MutateNode` 触发）重新注册 delegated click/mousedown + canvas click/zoom/pan/marquee 共 6 个监听器，全部 `Closure::forget()` 永不释放。监听器数随渲染次数线性增长 → 同一事件被 N 个累积 handler 处理 → 帧率塌陷 → 卡死。

**修复**：新增全局 `LISTENERS_INSTALLED: AtomicBool` + `mark_listeners_installed()`。`render_dom` 仅在首次（标志为 false）注册 6 个监听器，置位后跳过。渲染只更新 DOM 内容，不再重绑事件。

### P0-2 拖拽 mouseup/mousemove 泄漏 + 重复触发（致卡死）

**位置**：delegated mousedown / canvas pan / marquee / resize handle 共 4 处的 `on_up`。

**病根**：每次拖拽会话在 window 上 `add_event_listener("mouseup")` + `forget()`，旧 mouseup 永不移除。第 K 次拖拽时 K 组历史 mouseup 同时响应。虽然 mouseup 内会 remove mousemove，但 mouseup 自身累积 → 拖拽位置错乱 + 性能崩塌。

**修复**：4 处 `on_up` 由 `Closure::wrap(Box<dyn FnMut>)` 改为 `Closure::once(Box<dyn FnOnce>)`。once 闭包触发一次后由 wasm-bindgen 自清理，不再累积活跃 listener。mousemove 仍由 mouseup 内 `remove_event_listener` 清除。

### P0-3 schedule_raf 标志位竞态致渲染永久停摆（致卡死）

**位置**：`schedule_raf`。

**病根**：`RAF_SCHEDULED` 置 true 后，若 `request_animation_frame` 注册失败（`.ok()` 吞错）或回调 panic，标志永久卡 true → 后续所有 `schedule_raf` 在入口直接 return → 渲染彻底停摆（画面冻结、操作无响应）。

**修复**：
- window 不可用：立即复位标志。
- rAF 注册失败（`is_err()`）：复位标志 + console 警告。
- 回调内**先复位标志再执行**，即使回调 panic 也不阻塞后续调度。
- 内存序由 `Relaxed` 提升为 `SeqCst`。

### P1-1 SHELL 锁中毒即全死

**位置**：所有 `*SHELL.lock().unwrap()` / `SHELL.lock().unwrap()`（3 处）。

**病根**：任何一处持锁 panic → Mutex 中毒 → 后续所有 `.lock().unwrap()` panic 传播 → 整个渲染层永久不可用。

**修复**：新增 `shell_lock()` 辅助，用 `unwrap_or_else(|e| e.into_inner())` 容错取数据，替换全部 3 处。

### P1-2 render_page 持锁期间全量重绘阻塞消息

**位置**：`render_page`。

**病根**：持有 SHELL 锁期间同步遍历全文档重绘 canvas，大文档时锁长期占用，阻塞并发的消息处理 → UI 无响应。

**修复**：仅持锁取 `canvas_id` 后立即释放（`let canvas_id = { let guard = shell_lock(); ... }`），再执行无锁重绘。

### P2-1 递归渲染无深度上限（致栈溢出卡死）

**位置**：`render_node` / `render_node_to_dom`。

**病根**：对 `node.children` 无界递归，恶意或异常深层嵌套文档 → wasm 栈溢出 → trap → 崩溃/卡死。

**修复**：新增常量 `MAX_RENDER_DEPTH = 64`。两函数加 `depth: u32` 参数，超过上限跳过子树 + console 警告。

## 测试

新增 2 测试：
- `listeners_installed_flag_is_idempotent`：验证 `mark_listeners_installed` 幂等性（P0-1）。
- `render_depth_limit_constant_bounded`：验证深度上限常量存在且合理（P2-1）。

`cargo test -p fd-host-web`：41 passed（原 39 + 2）。
`cargo test --workspace`：334 passed, 0 failed。
`cargo fmt --all -- --check`：clean。
`cargo clippy -p fd-host-web`：本次改动 0 新 warning。

## 未覆盖（后续）

- `handler.forget()`（message listener，`mount` 内）：单次 mount 不累积，但重复 mount 仍泄漏。当前 mount 设计为一次性，暂不处理。
- `on_click.forget()` / `on_wheel.forget()` 等容器级监听器：经 P0-1 幂等后只 forget 一次，生命周期与 wasm 同长，可接受。
