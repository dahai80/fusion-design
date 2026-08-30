//! Fusion-Design Web 宿主 — wasm32-unknown-unknown 浏览器入口。
//!
//! 零 jian 依赖，专为 Fusion-Desk WKWebView 设计。
//! 职责：
//! - `mount()`：初始化 WebShell，验证 canvas 存在
//! - 消息桥接：WKWebView ↔ 原生宿主（通过 `window.webkit.messageHandlers`）
//! - 渲染：PenDocument → HTML Canvas / DOM
//! - 交互事件：click/drag/select → BridgeEvent → 后端
//! - 离线硬约束：所有通信走 `127.0.0.1` 本地后端

use std::sync::{LazyLock, Mutex};

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use fd_canvas_core::{sanitize_css_value, PenDocument};

// ARCH-10 round2：消息桥接集群拆到独立模块（从原 320-766 行迁出）。
// 本 commit 同时落地 OPS-13 WASM 日志环形缓冲（LogRingBuffer/fd_log_*）+
// F-19 dispatch_to_host 累积队列 + log.capture.dump 派发 arm（见 commit message）。
// re-export 协议 enum + 事件发送 fn + HOST_HANDLER_NAME 供 lib.rs 事件处理器
// 与 tests.rs（use super::*）续用。
pub(crate) mod bridge;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
pub(crate) use bridge::HOST_HANDLER_NAME;
#[allow(unused_imports)]
pub(crate) use bridge::{
    parse_bridge_command, send_bridge_event, send_to_host, BridgeCommand, BridgeEvent,
};

// ARCH-10 round2：DOM 渲染管线集群拆到独立模块（从原 335-902 行迁出）。
// 零逻辑改，仅迁位置 + 必要可见性 pub(crate)。模块名 render_dom 与迁入的 fn
// render_dom 同名，经下方 pub(crate) use re-export 消解，bridge.rs/tests.rs
// 调用站不动。re-export 集：4 fn（bridge/tests/lib.rs 调）+ 3 const（lib.rs
// render_node + tests.rs 调）。
pub(crate) mod render_dom;
#[allow(unused_imports)]
pub(crate) use render_dom::{
    is_node_geom_valid, render_dom, track_to_css, viewport_cull_update, MAX_CHILDREN_PER_NODE,
    MAX_NODE_DIM_PX, MAX_ZOOM, VIEWPORT_MARGIN,
};

// ARCH-10 round2：Canvas 渲染管线集群拆到独立模块（从原 2246-2599 行迁出）。
// 零逻辑改，仅迁位置 + 必要可见性 pub(crate)（canvas_id 字段）。模块名 render_canvas
// 与迁入 fn 无重名。re-export 集：5 fn（bridge.rs 调）+ 1 const（render_dom.rs +
// tests.rs 调）经 pub(crate) use 消解跨模块可见性，bridge.rs/render_dom.rs/tests.rs
// 调用站不动。
pub(crate) mod render_canvas;
#[allow(unused_imports)]
pub(crate) use render_canvas::{
    clear_canvas, remove_plan_preview, render_page, render_plan_preview, reset_canvas_view,
    MAX_RENDER_DEPTH,
};

// ARCH-10 round2：交互事件管线集群拆到独立模块（从原 499-1528 行迁出）。
// 零逻辑改，仅迁位置 + 必要可见性 pub(crate)。模块名 events 与迁入 fn 无重名。
// re-export 集：6 setup_*_listener（render_dom.rs 调）+ 6 CSS helper（lib.rs node ops 调）
// 共 12 项经 pub(crate) use 消解跨模块可见性，render_dom.rs/tests.rs 调用站不动。
// ARCH-10 round-3：拖拽 thread_local（ACTIVE_DRAG_MOVE/PENDING_DRAG_UP/RESIZE_HANDLES）+
// DragMoveClosure + cleanup_pending_drag 迁 events.rs（主消费方归位）。lib.rs select_node
// 路径经反向 use 消费。R-1 注释随迁。
pub(crate) mod events;
#[allow(unused_imports)]
pub(crate) use events::{
    read_css_prop_value, read_node_position, read_node_size, replace_css_prop,
    setup_canvas_click_listener, setup_canvas_pan_listener, setup_canvas_zoom_listener,
    setup_delegated_click_listener, setup_delegated_mousedown_listener, setup_marquee_listener,
    strip_css_prop, update_node_position, RAF_SCHEDULED,
};
// ARCH-10 round-3：拖拽全局态反向导入（定义已迁 events.rs，本 crate select_node 路径消费）。
#[allow(unused_imports)]
pub(crate) use events::{cleanup_pending_drag, ACTIVE_DRAG_MOVE, PENDING_DRAG_UP, RESIZE_HANDLES};

fn css_escape_attr_value(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(crate) fn node_selector(node_id: &str) -> String {
    format!("[data-node-id=\"{}\"]", css_escape_attr_value(node_id))
}

// ── 全局状态 ──

pub(crate) static SHELL: LazyLock<Mutex<Option<WebShellInner>>> =
    LazyLock::new(|| Mutex::new(None));

// 容器级监听器幂等保护。
// L-15：旧实现用进程级全局 AtomicBool，容器重建（新 mount）后标志仍 true →
// 新容器无监听器，事件失效。改为 per-container 属性标记，每次渲染查当前容器。
#[cfg(target_arch = "wasm32")]
const LISTENERS_ATTR: &str = "data-fd-listeners";

// ── OPS-13：WASM 日志环形缓冲 ──
// 审计裁定：WKWebView 内现场故障零诊断件。WASM 沙箱无文件系统，console.* 仅开发者
// 工具可见，企业运维无门。加环形缓冲捕获最近 N 条 error/warn，host 经消息桥
// `log.capture.dump` 拉取 → Swift 侧落盘（host 侧 handler 留 TODO+issue，非本工程范围）。
// 容量 200 条平衡诊断覆盖与线性内存占用（每条 ~数百字节，封顶 ~100KB）。

const LOG_RING_CAPACITY: usize = 200;

#[derive(serde::Serialize, Clone)]
pub(crate) struct LogEntry {
    level: &'static str,
    ts_ms: f64,
    msg: String,
}

struct LogRingBuffer {
    entries: std::collections::VecDeque<LogEntry>,
}

impl LogRingBuffer {
    fn new() -> Self {
        Self {
            entries: std::collections::VecDeque::with_capacity(LOG_RING_CAPACITY),
        }
    }

    fn push(&mut self, entry: LogEntry) {
        if self.entries.len() >= LOG_RING_CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    fn dump(&self) -> Vec<LogEntry> {
        self.entries.iter().cloned().collect()
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

thread_local! {
    pub(crate) static LOG_RING: std::cell::RefCell<LogRingBuffer> =
        std::cell::RefCell::new(LogRingBuffer::new());
}

// 性能时间戳：wasm32 取 performance.now()（单调时钟，毫秒），非 wasm 测试填 0.0。
// Date::now() 受宿主时钟漂移影响且部分环境受限，performance.now() 更稳。
fn log_timestamp() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.performance())
            .map(|p| p.now())
            .unwrap_or(0.0)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        0.0
    }
}

// OPS-13：error/warn 日志双写——console.*（开发者工具可见）+ 环形缓冲（host 可拉取）。
// 现有 32 处裸 console::error_1/warn_1 站点暂不逐一迁移（回归面大），先接入 panic hook
// + 桥接失败 + 反序列化失败等关键现场故障路径，余下逐步迁移。
pub(crate) fn fd_log_error(msg: &str) {
    web_sys::console::error_1(&msg.into());
    LOG_RING.with(|r| {
        r.borrow_mut().push(LogEntry {
            level: "error",
            ts_ms: log_timestamp(),
            msg: msg.to_string(),
        })
    });
}

pub(crate) fn fd_log_warn(msg: &str) {
    web_sys::console::warn_1(&msg.into());
    LOG_RING.with(|r| {
        r.borrow_mut().push(LogEntry {
            level: "warn",
            ts_ms: log_timestamp(),
            msg: msg.to_string(),
        })
    });
}

/// 检查当前容器是否已装监听器；未装则标记并返回 false（需安装），已装返回 true。
pub(crate) fn mark_listeners_installed(container_id: &str) -> bool {
    // L-15：非 wasm 目标无 DOM，返回 true（跳过安装，幂等安全）。
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = container_id;
        true
    }
    #[cfg(target_arch = "wasm32")]
    {
        let window = match web_sys::window() {
            Some(w) => w,
            None => return true, // 无 window 无法装，按已装跳过避免反复尝试
        };
        let document = match window.document() {
            Some(d) => d,
            None => return true,
        };
        if let Some(container) = document.get_element_by_id(container_id) {
            if container.get_attribute(LISTENERS_ATTR).is_some() {
                return true;
            }
            let _ = container.set_attribute(LISTENERS_ATTR, "1");
            false
        } else {
            true
        }
    }
}

/// 容错获取 SHELL 锁。R-15：锁中毒（持锁线程 panic）时**丢弃中毒脏数据**，
/// 重置为 None 而非读半更新状态——下次 mount 重新初始化，避免渲染层基于脏数据
/// 产出不一致画面。旧实现 `into_inner()` 取脏数据是伪容错：脏数据可能半更新，
/// 继续渲染出错误 DOM/canvas。
pub(crate) fn shell_lock() -> std::sync::MutexGuard<'static, Option<WebShellInner>> {
    match SHELL.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            fd_log_error(
                "fd-host-web: SHELL 锁中毒（持锁线程 panic），丢弃脏数据重置为 None，下次 mount 重新初始化",
            );
            let mut guard = poisoned.into_inner();
            *guard = None;
            guard
        }
    }
}

// ── WebShell ──

/// 宿主外壳（wasm_bindgen 公开类型）。
#[wasm_bindgen]
pub struct WebShell {
    #[allow(dead_code)]
    inner: WebShellInner,
}

#[allow(dead_code)]
pub(crate) struct WebShellInner {
    pub(crate) canvas_id: String,
    ready: bool,
    // C11：缓存最近一次 render_dom 的 PenDocument JSON，替代 DOM 属性存储。
    // viewport_cull_update 从这里读取，避免每帧从 data-fd-doc 重新解析整文档。
    pub(crate) cached_doc_json: Option<String>,
    // PERF-2：缓存已解析的 PenDocument，避免 viewport_cull_update 每 rAF tick
    // 重解析 JSON（O(文档大小) × 帧率）。与 cached_doc_json 同步写入，json 为真相源，
    // doc 为缓存。doc=None 而 json=Some 时回退解析一次（安全网，不应常态触发）。
    pub(crate) cached_doc: Option<PenDocument>,
    // P-7：当前选中节点 id。select_node 只清上一个选中元素 + handles，
    // 免全局 [data-fd-selected] 扫描。None = 无选中。
    pub(crate) selected_id: Option<String>,
    // R-17：Shift 多选集合。toggle_node_selection 增删此集合，
    // select_node 清空集合后插入单选。与 selected_id 并存——
    // selected_id 是"主选中"（handles 锚点），selected_ids 是全量多选态。
    pub(crate) selected_ids: std::collections::HashSet<String>,
}

/// 初始化 Web 宿主。
///
/// 验证 canvas 元素存在，初始化 panic hook，返回 WebShell 实例。
/// 对应 `op-host-web::mount` 的 `<canvas>` 校验逻辑。
#[wasm_bindgen]
pub fn mount(canvas_id: &str) -> Result<WebShell, JsValue> {
    // R-A18：注册 panic hook，panic 落 console.error 而非静默 unreachable trap。
    // 否则事件回调任一 panic → 整个 WebShell 死亡 → WKWebView 永久白屏无诊断。
    console_error_panic_hook::set_once();
    // OPS-13：panic 是最需诊断的现场故障，但 console_error_panic_hook 仅落 console.*
    // （开发者工具可见，企业运维无门）。再包一层：取 panic_hook 库的 hook 作为 prev，
    // panic 时先写环形缓冲（host 可拉取落盘）再链调 prev，双写不互斥。
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        fd_log_error(&format!("fd-host-web: panic: {info}"));
        prev_hook(info);
    }));

    let window = web_sys::window().ok_or_else(|| JsValue::from_str("mount: window unavailable"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("mount: document unavailable"))?;
    let element = document
        .get_element_by_id(canvas_id)
        .ok_or_else(|| JsValue::from_str(&format!("mount: canvas '{canvas_id}' not found")))?;
    let _canvas = element
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .map_err(|_| JsValue::from_str("mount: target element is not <canvas>"))?;

    let inner = WebShellInner {
        canvas_id: canvas_id.to_string(),
        ready: true,
        cached_doc_json: None,
        cached_doc: None,
        selected_id: None,
        selected_ids: std::collections::HashSet::new(),
    };

    // 注册消息监听器
    bridge::setup_message_listener(&window)?;

    let shell = WebShell { inner };
    *shell_lock() = Some(WebShellInner {
        canvas_id: canvas_id.to_string(),
        ready: true,
        cached_doc_json: None,
        cached_doc: None,
        selected_id: None,
        selected_ids: std::collections::HashSet::new(),
    });

    Ok(shell)
}

// ── 吸附对齐 ──

/// 吸附阈值（px），小于此距离自动吸附。
const SNAP_THRESHOLD: f32 = 5.0;
/// 默认节点宽度（解析失败时回退值）。
pub(crate) const DEFAULT_NODE_WIDTH: f32 = 100.0;
/// 默认节点高度（解析失败时回退值）。
pub(crate) const DEFAULT_NODE_HEIGHT: f32 = 40.0;
/// 选框最小尺寸（px），小于此值忽略。
pub(crate) const MIN_MARQUEE_SIZE: f32 = 5.0;

/// 收集画布中所有节点的吸附候选线（边缘 + 中心）。
/// 返回 (x_lines, y_lines)，即垂直吸附线 X 坐标集合和水平吸附线 Y 坐标集合。
/// P-6：几何优先读 data-fd-* 缓存属性（O(1) 属性读），缺则回退 style 串解析。
pub(crate) fn collect_snap_candidates(exclude_id: &str) -> (Vec<f32>, Vec<f32>) {
    let mut x_lines: Vec<f32> = Vec::new();
    let mut y_lines: Vec<f32> = Vec::new();
    let window = match web_sys::window() {
        Some(w) => w,
        None => return (x_lines, y_lines),
    };
    let document = match window.document() {
        Some(d) => d,
        None => return (x_lines, y_lines),
    };
    let Ok(nodes) = document.query_selector_all("[data-node-id]") else {
        return (x_lines, y_lines);
    };
    for i in 0..nodes.length() {
        if let Some(el) = nodes.item(i) {
            if let Ok(element) = el.dyn_into::<web_sys::Element>() {
                let nid = element.get_attribute("data-node-id").unwrap_or_default();
                if nid == exclude_id {
                    continue;
                }
                let (x, y) = read_node_position(&element);
                let (w, h) = read_node_size(&element);
                // 6 条吸附线：左/右/中心H + 上/下/中心V
                x_lines.push(x);
                x_lines.push(x + w);
                x_lines.push(x + w / 2.0);
                y_lines.push(y);
                y_lines.push(y + h);
                y_lines.push(y + h / 2.0);
            }
        }
    }
    (x_lines, y_lines)
}

/// P-6：从 data-* 属性读 f32，缺/非法时 None。
pub(crate) fn read_attr_f32(el: &web_sys::Element, attr: &str) -> Option<f32> {
    el.get_attribute(attr).and_then(|s| s.parse().ok())
}

/// 对单轴查找最近吸附偏移。返回 (吸附偏移, 是否吸附, 吸附线坐标)。
pub(crate) fn find_snap_offset(value: f32, candidates: &[f32]) -> (f32, bool, f32) {
    let mut best_dist = SNAP_THRESHOLD + 1.0;
    let mut best_line = value;
    for &c in candidates {
        let dist = (value - c).abs();
        if dist < best_dist {
            best_dist = dist;
            best_line = c;
        }
    }
    if best_dist <= SNAP_THRESHOLD {
        let offset = best_line - value;
        (offset, true, best_line)
    } else {
        (0.0, false, value)
    }
}

/// 显示吸附辅助线（DOM 叠加层）。
pub(crate) fn show_snap_lines(x_lines: &[f32], y_lines: &[f32]) {
    hide_snap_lines();
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };
    let container = match document.get_element_by_id("fusion-dom-root") {
        Some(c) => c,
        None => return,
    };
    // 创建吸附线容器
    let overlay = match document.create_element("div") {
        Ok(el) => el,
        Err(_) => {
            web_sys::console::warn_1(&"fd-host-web: show_snap_lines 创建 overlay 失败".into());
            return;
        }
    };
    overlay.set_id("fd-snap-overlay");
    overlay.set_attribute("style",
        "position:absolute;top:0;left:0;width:100%;height:100%;pointer-events:none;z-index:9998;overflow:visible;"
    ).ok();

    // 垂直吸附线（X 坐标 → 竖线）
    for &x in x_lines {
        if let Ok(line) = document.create_element("div") {
            line.set_attribute("style", &format!(
                "position:absolute;left:{}px;top:0;width:1px;height:100%;background:#007AFF;opacity:0.5;",
                x
            )).ok();
            overlay.append_child(&line).ok();
        }
    }
    // 水平吸附线（Y 坐标 → 横线）
    for &y in y_lines {
        if let Ok(line) = document.create_element("div") {
            line.set_attribute("style", &format!(
                "position:absolute;top:{}px;left:0;height:1px;width:100%;background:#007AFF;opacity:0.5;",
                y
            )).ok();
            overlay.append_child(&line).ok();
        }
    }
    container.append_child(&overlay).ok();
}

/// 隐藏吸附辅助线。
pub(crate) fn hide_snap_lines() {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };
    if let Some(overlay) = document.get_element_by_id("fd-snap-overlay") {
        overlay.remove();
    }
}

// ── Bridge 辅助 ──

/// 注入设计 Token CSS 到页面 :root。
pub(crate) fn apply_tokens_css(css: &str) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };

    // 移除旧的 token style
    if let Some(old) = document.get_element_by_id("fusion-tokens") {
        old.remove();
    }

    // C10：sanitize 后再注入，剥离 @import/@font-face/url()/expression()/javascript:
    let safe = sanitize_token_css(css);

    // 注入新 token style
    let style_el = match document.create_element("style") {
        Ok(el) => el,
        Err(_) => {
            web_sys::console::warn_1(&"fd-host-web: apply_tokens_css 创建 <style> 失败".into());
            return;
        }
    };
    style_el.set_id("fusion-tokens");
    style_el.set_text_content(Some(&safe));
    if let Some(head) = document.head() {
        head.append_child(&style_el).ok();
    } else {
        web_sys::console::warn_1(&"fd-host-web: apply_tokens_css document.head 缺失".into());
    }
}

// C10：剥离危险 CSS 内容，只保留 --token: value 形式的声明。
// 命中以下关键词的整行剔除（大小写不敏感）：@import / @font-face / expression( / url( / javascript:
// E-40/P3：先剥离 /* */ 注释再匹配关键词，防 `u/**/rl(` 等注释拆词绕过 url() 检测。
// 返回净化后的 CSS；stripped 计数通过 warn 日志输出（不回显原始 payload）。
fn strip_css_comments(line: &str) -> String {
    // 简单状态机剥离 /* ... */，跨行注释按行处理（每行独立），未闭合注释剥到行尾。
    let mut out = String::with_capacity(line.len());
    let bytes: Vec<char> = line.chars().collect();
    let mut i = 0;
    let mut in_comment = false;
    while i < bytes.len() {
        if in_comment {
            if bytes[i] == '*' && i + 1 < bytes.len() && bytes[i + 1] == '/' {
                in_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if bytes[i] == '/' && i + 1 < bytes.len() && bytes[i + 1] == '*' {
            in_comment = true;
            i += 2;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn sanitize_token_css(css: &str) -> String {
    let (safe, stripped) = sanitize_token_css_inner(css);
    if stripped > 0 {
        web_sys::console::warn_1(
            &format!("fd-host-web: sanitize_token_css 剔除 {stripped} 行危险 CSS").into(),
        );
    }
    safe
}

// E-40/P3：纯逻辑核心，剥离注释 + 危险关键字整行剔除，返回 (净化CSS, 剔除行数)。
// 抽离以便原生测试（非 wasm 不触发 web_sys::console::warn_1 panic）。
fn sanitize_token_css_inner(css: &str) -> (String, u32) {
    let dangerous = [
        "@import",
        "@font-face",
        "expression(",
        "url(",
        "javascript:",
    ];
    let mut stripped: u32 = 0;
    let mut out: Vec<&str> = Vec::with_capacity(css.lines().count());
    for line in css.lines() {
        // E-40：剥注释后匹配，原始行保留（注释本身无害，危险的是拆词后的关键字）。
        let lowered = strip_css_comments(line).to_ascii_lowercase();
        if dangerous.iter().any(|needle| lowered.contains(needle)) {
            stripped += 1;
            continue;
        }
        out.push(line);
    }
    (out.join("\n"), stripped)
}

// E-39：CSS 颜色值净化（单值，非整段 CSS）。mutate_node 的 fill/stroke 来自后端消息
// 任意 String，原样拼进 `background-color:{f}` / `border:.. solid {s}` 可逃逸属性注入
// 任意 CSS（如 `red; } * { position:fixed; background:url(http://evil)` 破离线约束）。
// 拒绝 url()/expression()/@import，剔除可逃逸属性边界的 `;{}`，保留 hex/命名色/rgb()。
fn sanitize_css_color(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("url(")
        || lower.contains("expression(")
        || lower.contains("@import")
        || lower.contains("javascript:")
    {
        web_sys::console::warn_1(
            &format!("fd-host-web: CSS 颜色值含危险函数，降级 transparent: {raw}").into(),
        );
        return "transparent".to_string();
    }
    raw.chars()
        .filter(|&c| !matches!(c, ';' | '{' | '}' | '<' | '>'))
        .collect()
}

/// 选中节点（添加选中高亮 + resize handles）。
pub(crate) fn select_node(node_id: &str) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };

    // P-7：从 SHELL 取上一个选中 id。命中则只清该元素，免全局 [data-fd-selected] 扫描。
    // 未命中（冷启动/状态丢失）仍走全扫兜底，保证残留选中必被清。
    let prev_id: Option<String> = shell_lock().as_ref().and_then(|i| i.selected_id.clone());
    let same_selection = prev_id.as_deref() == Some(node_id);

    if same_selection {
        // 重复选中同一节点：状态与高亮已就位，直接返回免重建 handles。
        return;
    }

    // 清上一个选中元素：仅该一个，不再 querySelectorAll 全扫。
    let clear_prev = |document: &web_sys::Document| {
        let clear_el = |el: &web_sys::Element| {
            // L4：取消选中只移除我们注入的 box-shadow，不触碰用户 outline。
            let style = el.get_attribute("style").unwrap_or_default();
            let clean = strip_css_prop(&style, "box-shadow");
            el.set_attribute("style", &clean).ok();
            el.remove_attribute("data-fd-selected").ok();
        };
        match &prev_id {
            Some(pid) => {
                if let Some(el) = document.query_selector(&node_selector(pid)).unwrap_or(None) {
                    clear_el(&el);
                } else {
                    // 上一个元素已不在 DOM（被删/重渲）：全扫兜底清残留选中态。
                    if let Ok(selected) = document.query_selector_all("[data-fd-selected]") {
                        for i in 0..selected.length() {
                            if let Some(node) = selected.item(i) {
                                if let Ok(el) = node.dyn_into::<web_sys::Element>() {
                                    clear_el(&el);
                                }
                            }
                        }
                    }
                }
            }
            None => {
                // 无记录：全扫兜底（首选/状态丢失后）。
                if let Ok(selected) = document.query_selector_all("[data-fd-selected]") {
                    for i in 0..selected.length() {
                        if let Some(node) = selected.item(i) {
                            if let Ok(el) = node.dyn_into::<web_sys::Element>() {
                                clear_el(&el);
                            }
                        }
                    }
                }
            }
        }
        // 移除旧 resize handles（选中变化才重建，same_selection 已早返回）。
        if let Ok(handles) = document.query_selector_all(".fd-resize-handle") {
            for i in 0..handles.length() {
                if let Some(h) = handles.item(i) {
                    if let Ok(el) = h.dyn_into::<web_sys::Element>() {
                        el.remove();
                    }
                }
            }
        }
        // R-1 backfill：DOM handle 已移除，同步回收上一批 8 个 on_handle_mousedown Closure
        // （旧 .forget() 每次选中切换泄漏 8 个，长会话线性增长）。clear drop 全部。
        RESIZE_HANDLES.with(|c| c.borrow_mut().clear());
    };
    clear_prev(&document);

    // 设置新选中
    if let Some(el) = document
        .query_selector(&node_selector(node_id))
        .unwrap_or(None)
    {
        el.set_attribute("data-fd-selected", "true").ok();
        // L4：用 box-shadow 做选中高亮，保留节点自身 outline 不被覆盖。
        let style = el.get_attribute("style").unwrap_or_default();
        el.set_attribute("style", &format!("{};box-shadow:0 0 0 2px #007AFF;", style))
            .ok();

        // 添加 8 个 resize handles
        let (w, h) = read_node_size(&el);
        let (x, y) = read_node_position(&el);
        let positions = [
            ("nw", x, y),
            ("n", x + w / 2.0, y),
            ("ne", x + w, y),
            ("e", x + w, y + h / 2.0),
            ("se", x + w, y + h),
            ("s", x + w / 2.0, y + h),
            ("sw", x, y + h),
            ("w", x, y + h / 2.0),
        ];
        let container = document.get_element_by_id("fusion-dom-root");
        for (dir, hx, hy) in &positions {
            if let Some(handle) = create_resize_handle(&document, dir, *hx, *hy, node_id) {
                if let Some(c) = &container {
                    c.append_child(&handle).ok();
                } else if let Some(body) = document.body() {
                    body.append_child(&handle).ok();
                }
            }
        }
        // P-7/R-17：记录新选中 id。select_node 是单选语义——清空多选集合后只插这一个。
        if let Some(inner) = shell_lock().as_mut() {
            inner.selected_ids.clear();
            inner.selected_ids.insert(node_id.to_string());
            inner.selected_id = Some(node_id.to_string());
        }
    } else {
        // 选中目标不存在：清空记录。
        if let Some(inner) = shell_lock().as_mut() {
            inner.selected_ids.clear();
            inner.selected_id = None;
        }
    }
}

/// 创建单个 resize handle DOM 元素。
fn create_resize_handle(
    document: &web_sys::Document,
    dir: &str,
    x: f32,
    y: f32,
    node_id: &str,
) -> Option<web_sys::Element> {
    let handle = document.create_element("div").ok()?;
    handle.set_attribute("class", "fd-resize-handle").ok()?;
    handle.set_attribute("data-resize-dir", dir).ok()?;
    handle.set_attribute("data-resize-node", node_id).ok()?;
    let cursor = match dir {
        "nw" | "se" => "nwse-resize",
        "ne" | "sw" => "nesw-resize",
        "n" | "s" => "ns-resize",
        "e" | "w" => "ew-resize",
        _ => "default",
    };
    handle.set_attribute("style", &format!(
        "position:absolute;left:{}px;top:{}px;width:8px;height:8px;background:white;border:1px solid #007AFF;border-radius:1px;cursor:{};z-index:9999;margin-left:-4px;margin-top:-4px;",
        x, y, cursor
    )).ok()?;

    // mousedown on handle → resize drag
    let dir_str = dir.to_string();
    let nid = node_id.to_string();
    let on_handle_mousedown = Closure::wrap(Box::new(move |event: web_sys::Event| {
        // R-1 backfill：拖拽开始前回收上一轮残留 on_up/on_move（漏 mouseup 场景）。
        cleanup_pending_drag();
        let mouse = match event.dyn_ref::<web_sys::MouseEvent>() {
            Some(m) => m,
            None => return,
        };
        let start_x = mouse.client_x() as f32;
        let start_y = mouse.client_y() as f32;

        // read current node size + position
        let Some(win) = web_sys::window() else {
            return;
        };
        let Some(doc) = win.document() else {
            return;
        };
        let target = doc.query_selector(&node_selector(&nid)).unwrap_or(None);
        let (orig_x, orig_y, orig_w, orig_h) = match target {
            Some(el) => {
                let (ox, oy) = read_node_position(&el);
                let (ow, oh) = read_node_size(&el);
                (ox, oy, ow, oh)
            }
            None => (0.0, 0.0, DEFAULT_NODE_WIDTH, DEFAULT_NODE_HEIGHT),
        };

        let resize_id = nid.clone();
        let resize_dir = dir_str.clone();
        let on_move = Closure::wrap(Box::new(move |event: web_sys::Event| {
            let mm = match event.dyn_ref::<web_sys::MouseEvent>() {
                Some(m) => m,
                None => return,
            };
            let dx = mm.client_x() as f32 - start_x;
            let dy = mm.client_y() as f32 - start_y;

            let (nx, ny, nw, nh) =
                compute_resize(&resize_dir, orig_x, orig_y, orig_w, orig_h, dx, dy);

            // live preview: update element size + position
            let Some(win2) = web_sys::window() else {
                return;
            };
            let Some(doc2) = win2.document() else {
                return;
            };
            if let Some(el) = doc2
                .query_selector(&node_selector(&resize_id))
                .unwrap_or(None)
            {
                update_node_position(&el, nx, ny);
                update_node_size(&el, nw, nh);
            }
        }) as Box<dyn FnMut(web_sys::Event)>);

        let Some(window) = web_sys::window() else {
            return;
        };
        window
            .add_event_listener_with_callback("mousemove", on_move.as_ref().unchecked_ref())
            .ok();

        let rid = nid.clone();
        let rdir = dir_str.clone();
        let move_js: JsValue = on_move.as_ref().unchecked_ref::<js_sys::Function>().into();
        // R-1：存 on_move 到 thread_local，mouseup 时 take 回收（替代 forget 泄漏）。
        ACTIVE_DRAG_MOVE.with(|c| *c.borrow_mut() = Some(on_move));
        let on_up = Closure::wrap(Box::new(move |event: web_sys::Event| {
            let Some(w) = web_sys::window() else {
                return;
            };
            let move_ref: &js_sys::Function = move_js.unchecked_ref();
            w.remove_event_listener_with_callback("mousemove", move_ref)
                .ok();
            // R-1：移除监听后 take + drop on_move Closure，回收线性内存。
            ACTIVE_DRAG_MOVE.with(|c| *c.borrow_mut() = None);
            // R-1 backfill：同步回收 on_up 自身（PENDING_DRAG_UP）。
            PENDING_DRAG_UP.with(|c| *c.borrow_mut() = None);

            let mm = event.dyn_ref::<web_sys::MouseEvent>();
            let (dx, dy) = match mm {
                Some(m) => (m.client_x() as f32 - start_x, m.client_y() as f32 - start_y),
                None => (0.0, 0.0),
            };
            let (nx, ny, nw, nh) = compute_resize(&rdir, orig_x, orig_y, orig_w, orig_h, dx, dy);
            send_bridge_event(BridgeEvent::NodeResize {
                node_id: rid.clone(),
                w: nw,
                h: nh,
            });
            send_bridge_event(BridgeEvent::NodeDrag {
                node_id: rid.clone(),
                dx: nx - orig_x,
                dy: ny - orig_y,
            });

            // refresh handles after resize
            select_node(&rid);
        }) as Box<dyn FnMut(web_sys::Event)>);
        window
            .add_event_listener_with_callback("mouseup", on_up.as_ref().unchecked_ref())
            .ok();
        // R-1 backfill：on_up 托管到 PENDING_DRAG_UP（漏 mouseup 时 cleanup_pending_drag 回收）。
        PENDING_DRAG_UP.with(|c| *c.borrow_mut() = Some(on_up));
        // R-1：on_move 已托管到 SHELL（mouseup 时 take 回收），不再 forget。

        event.stop_propagation();
        event.prevent_default();
    }) as Box<dyn FnMut(web_sys::Event)>);
    handle
        .add_event_listener_with_callback("mousedown", on_handle_mousedown.as_ref().unchecked_ref())
        .ok();
    // R-1 backfill：handle mousedown Closure 托管到 RESIZE_HANDLES（select_node 重建前 clear 回收）。
    RESIZE_HANDLES.with(|c| c.borrow_mut().push(on_handle_mousedown));

    Some(handle)
}

/// 根据 resize 方向计算新位置/尺寸。
fn compute_resize(
    dir: &str,
    ox: f32,
    oy: f32,
    ow: f32,
    oh: f32,
    dx: f32,
    dy: f32,
) -> (f32, f32, f32, f32) {
    let min_size = 10.0;
    let (mut nx, mut ny, mut nw, mut nh) = (ox, oy, ow, oh);
    match dir {
        "se" => {
            nw = (ow + dx).max(min_size);
            nh = (oh + dy).max(min_size);
        }
        "e" => {
            nw = (ow + dx).max(min_size);
        }
        "s" => {
            nh = (oh + dy).max(min_size);
        }
        "nw" => {
            nx = ox + dx;
            ny = oy + dy;
            nw = (ow - dx).max(min_size);
            nh = (oh - dy).max(min_size);
        }
        "n" => {
            ny = oy + dy;
            nh = (oh - dy).max(min_size);
        }
        "ne" => {
            ny = oy + dy;
            nw = (ow + dx).max(min_size);
            nh = (oh - dy).max(min_size);
        }
        "sw" => {
            nx = ox + dx;
            nw = (ow - dx).max(min_size);
            nh = (oh + dy).max(min_size);
        }
        "w" => {
            nx = ox + dx;
            nw = (ow - dx).max(min_size);
        }
        _ => {}
    }
    (nx, ny, nw, nh)
}

/// 更新 DOM 元素的 width/height 样式（resize 实时预览）。
fn update_node_size(el: &web_sys::Element, w: f32, h: f32) {
    let style = el.get_attribute("style").unwrap_or_default();
    let new_style = replace_css_prop(&style, "width", &format!("{}px", w));
    let new_style = replace_css_prop(&new_style, "height", &format!("{}px", h));
    el.set_attribute("style", &new_style).ok();
    // P-6：同步刷新尺寸缓存。
    el.set_attribute("data-fd-w", &format!("{}", w)).ok();
    el.set_attribute("data-fd-h", &format!("{}", h)).ok();
}

/// MutateNode 命令处理：更新节点位置/尺寸/样式。
#[allow(clippy::too_many_arguments)]
pub(crate) fn mutate_node(
    node_id: &str,
    x: Option<f32>,
    y: Option<f32>,
    w: Option<f32>,
    h: Option<f32>,
    fill: &Option<String>,
    stroke: &Option<String>,
    stroke_width: Option<f32>,
    radius: Option<f32>,
    font_size: Option<f32>,
    font_family: &Option<String>,
    opacity: Option<f32>,
) {
    let window = match web_sys::window() {
        Some(win) => win,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };
    let el = match document
        .query_selector(&node_selector(node_id))
        .unwrap_or(None)
    {
        Some(e) => e,
        None => {
            web_sys::console::warn_1(
                &format!("fd-host-web: mutate_node 找不到节点 {node_id}").into(),
            );
            return;
        }
    };

    if let (Some(nx), Some(ny)) = (x, y) {
        update_node_position(&el, nx, ny);
    } else {
        if let Some(nx) = x {
            let style = el.get_attribute("style").unwrap_or_default();
            el.set_attribute(
                "style",
                &replace_css_prop(&style, "left", &format!("{}px", nx)),
            )
            .ok();
            // P-6：刷新 x 缓存。
            el.set_attribute("data-fd-x", &format!("{}", nx)).ok();
        }
        if let Some(ny) = y {
            let style = el.get_attribute("style").unwrap_or_default();
            el.set_attribute(
                "style",
                &replace_css_prop(&style, "top", &format!("{}px", ny)),
            )
            .ok();
            // P-6：刷新 y 缓存。
            el.set_attribute("data-fd-y", &format!("{}", ny)).ok();
        }
    }
    if let (Some(nw), Some(nh)) = (w, h) {
        update_node_size(&el, nw, nh);
    } else {
        if let Some(nw) = w {
            let style = el.get_attribute("style").unwrap_or_default();
            el.set_attribute(
                "style",
                &replace_css_prop(&style, "width", &format!("{}px", nw)),
            )
            .ok();
            // P-6：刷新 w 缓存。
            el.set_attribute("data-fd-w", &format!("{}", nw)).ok();
        }
        if let Some(nh) = h {
            let style = el.get_attribute("style").unwrap_or_default();
            el.set_attribute(
                "style",
                &replace_css_prop(&style, "height", &format!("{}px", nh)),
            )
            .ok();
            // P-6：刷新 h 缓存。
            el.set_attribute("data-fd-h", &format!("{}", nh)).ok();
        }
    }

    // Style mutations
    // E-39：fill/stroke 经 sanitize_css_color 净化后再注入，防 CSS 属性逃逸破离线约束。
    if let Some(f) = fill {
        let safe = sanitize_css_color(f);
        let style = el.get_attribute("style").unwrap_or_default();
        el.set_attribute(
            "style",
            &replace_css_prop(&style, "background-color", &safe),
        )
        .ok();
    }
    if let Some(s) = stroke {
        let sw = stroke_width.unwrap_or(1.0);
        let safe = sanitize_css_color(s);
        let style = el.get_attribute("style").unwrap_or_default();
        el.set_attribute(
            "style",
            &replace_css_prop(
                &replace_css_prop(&style, "border", &format!("{}px solid {}", sw, safe)),
                "border-color",
                &safe,
            ),
        )
        .ok();
    }
    if let Some(r) = radius {
        let style = el.get_attribute("style").unwrap_or_default();
        el.set_attribute(
            "style",
            &replace_css_prop(&style, "border-radius", &format!("{}px", r)),
        )
        .ok();
    }
    if let Some(fs) = font_size {
        let style = el.get_attribute("style").unwrap_or_default();
        el.set_attribute(
            "style",
            &replace_css_prop(&style, "font-size", &format!("{}px", fs)),
        )
        .ok();
    }
    if let Some(ff) = font_family {
        let style = el.get_attribute("style").unwrap_or_default();
        let ff = sanitize_css_value(ff, "");
        el.set_attribute("style", &replace_css_prop(&style, "font-family", &ff))
            .ok();
    }
    if let Some(o) = opacity {
        let style = el.get_attribute("style").unwrap_or_default();
        el.set_attribute(
            "style",
            &replace_css_prop(&style, "opacity", &o.to_string()),
        )
        .ok();
    }
}

/// 设置节点可见性：隐藏时 display:none，显示时恢复 display。
pub(crate) fn set_node_visibility(node_id: &str, visible: bool) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };
    if let Some(el) = document
        .query_selector(&node_selector(node_id))
        .unwrap_or(None)
    {
        if visible {
            el.remove_attribute("data-fd-hidden").ok();
            // L3：恢复显示时不强制 display:block，从 data-fd-prev-display 还原；
            // 缺失则移除 display 声明让 CSS 默认生效，避免破坏 flex/grid。
            let prev = el.get_attribute("data-fd-prev-display").unwrap_or_default();
            let style = el.get_attribute("style").unwrap_or_default();
            let new_style = if prev.is_empty() {
                strip_css_prop(&style, "display")
            } else {
                replace_css_prop(&style, "display", &prev)
            };
            el.set_attribute("style", &new_style).ok();
            el.remove_attribute("data-fd-prev-display").ok();
            web_sys::console::log_1(&format!("fd-host-web: node {node_id} set visible").into());
        } else {
            el.set_attribute("data-fd-hidden", "true").ok();
            // L3：隐藏前把当前 display 存起来，恢复时还原，避免硬编码 block 破坏布局。
            let style = el.get_attribute("style").unwrap_or_default();
            let prev_display = read_css_prop_value(&style, "display").unwrap_or_default();
            el.set_attribute("data-fd-prev-display", &prev_display).ok();
            let new_style = replace_css_prop(&style, "display", "none");
            el.set_attribute("style", &new_style).ok();
            web_sys::console::log_1(&format!("fd-host-web: node {node_id} set hidden").into());
        }
    }
}

/// 设置节点锁定状态：锁定节点禁止拖拽，虚线边框视觉反馈。
pub(crate) fn set_node_locked(node_id: &str, locked: bool) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };
    if let Some(el) = document
        .query_selector(&node_selector(node_id))
        .unwrap_or(None)
    {
        if locked {
            el.set_attribute("data-fd-locked", "true").ok();
            let style = el.get_attribute("style").unwrap_or_default();
            let with_border = replace_css_prop(&style, "border", "2px dashed #999");
            el.set_attribute("style", &with_border).ok();
            el.set_attribute("draggable", "false").ok();
            web_sys::console::log_1(&format!("fd-host-web: node {node_id} locked").into());
        } else {
            el.remove_attribute("data-fd-locked").ok();
            let style = el.get_attribute("style").unwrap_or_default();
            let with_border = replace_css_prop(&style, "border", "none");
            el.set_attribute("style", &with_border).ok();
            el.remove_attribute("draggable").ok();
            web_sys::console::log_1(&format!("fd-host-web: node {node_id} unlocked").into());
        }
    }
}

/// 重排序节点：将指定节点 DOM 移动到新位置。
pub(crate) fn reorder_node(node_id: &str, new_index: usize) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };
    let container = match document.get_element_by_id("fusion-dom-root") {
        Some(c) => c,
        None => return,
    };
    if let Some(el) = document
        .query_selector(&node_selector(node_id))
        .unwrap_or(None)
    {
        let node: &web_sys::Node = container.unchecked_ref();
        // L1：先把被移动节点从父容器剥离，再以剩余子节点为基准定位插入点。
        // 否则被移动节点本身仍占用一个槽位，导致目标 index 偏移一位、错位。
        node.remove_child(&el).ok();
        // L-17：插入点只在带 data-node-id 的子元素中算，忽略 resize handle/
        // snap overlay/plan preview 等辅助 DOM，否则 index 与节点序列错位。
        let child_nodes = node.child_nodes();
        let len = child_nodes.length();
        let mut node_children: Vec<web_sys::Element> = Vec::new();
        for i in 0..len {
            if let Some(c) = child_nodes.item(i) {
                if let Ok(el_child) = c.dyn_into::<web_sys::Element>() {
                    if el_child.get_attribute("data-node-id").is_some() {
                        node_children.push(el_child);
                    }
                }
            }
        }
        let count = node_children.len();
        // L1：clamp 到有效区间，防止 new_index 越界静默丢弃。
        let target = new_index.min(count);
        if target < count {
            if let Some(ref_child) = node_children.get(target) {
                node.insert_before(&el, Some(ref_child)).ok();
            } else {
                node.append_child(&el).ok();
            }
        } else {
            node.append_child(&el).ok();
        }
        web_sys::console::log_1(
            &format!("fd-host-web: node {node_id} reordered to {target}").into(),
        );
    }
}

// ── 单元测试（宿主目标，非 wasm32）──

#[cfg(test)]
mod tests;
