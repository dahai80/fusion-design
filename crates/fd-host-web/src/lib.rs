//! Fusion-Design Web host — wasm32-unknown-unknown browser entry point.
//!
//! Zero jian dependencies, designed specifically for the Fusion-Desk WKWebView.
//! Responsibilities:
//! - `mount()`: initialize the WebShell and verify the canvas exists
//! - Message bridging: WKWebView ↔ native host (via `window.webkit.messageHandlers`)
//! - Rendering: PenDocument → HTML Canvas / DOM
//! - Interaction events: click/drag/select → BridgeEvent → backend
//! - Offline hard constraint: all communication goes through the `127.0.0.1` local backend

use std::sync::{LazyLock, Mutex};

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use fd_canvas_core::{sanitize_css_value, PenDocument};

// ARCH-10 round2: split the message-bridging cluster into its own module (moved out of the original lines 320-766).
// This commit also lands OPS-13 WASM log ring buffer (LogRingBuffer/fd_log_*) +
// F-19 dispatch_to_host accumulation queue + the log.capture.dump dispatch arm (see commit message).
// Re-exports the protocol enum + event-sending fns + HOST_HANDLER_NAME so the lib.rs event handler
// and tests.rs (use super::*) can keep using them.
pub(crate) mod bridge;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
pub(crate) use bridge::HOST_HANDLER_NAME;
#[allow(unused_imports)]
pub(crate) use bridge::{
    parse_bridge_command, send_bridge_event, send_to_host, BridgeCommand, BridgeEvent,
};

// ARCH-10 round2: split the DOM rendering pipeline cluster into its own module (moved out of the original lines 335-902).
// Zero logic changes; only relocation + the necessary pub(crate) visibility. The module name render_dom
// collides with the migrated fn render_dom; resolved via the pub(crate) use re-export below, so the
// call sites in bridge.rs/tests.rs stay untouched. Re-export set: 4 fns (called by bridge/tests/lib.rs)
// + 3 consts (called by lib.rs render_node + tests.rs).
pub(crate) mod render_dom;
#[allow(unused_imports)]
pub(crate) use render_dom::{
    is_node_geom_valid, render_dom, track_to_css, viewport_cull_update, MAX_CHILDREN_PER_NODE,
    MAX_NODE_DIM_PX, MAX_ZOOM, VIEWPORT_MARGIN,
};

// ARCH-10 round2: split the Canvas rendering pipeline cluster into its own module (moved out of the original lines 2246-2599).
// Zero logic changes; only relocation + the necessary pub(crate) visibility (canvas_id field). The module name render_canvas
// does not collide with any migrated fn. Re-export set: 5 fns (called by bridge.rs) + 1 const (called by
// render_dom.rs + tests.rs), cross-module visibility resolved via pub(crate) use, so the call sites in
// bridge.rs/render_dom.rs/tests.rs stay untouched.
pub(crate) mod render_canvas;
#[allow(unused_imports)]
pub(crate) use render_canvas::{
    clear_canvas, remove_plan_preview, render_page, render_plan_preview, reset_canvas_view,
    MAX_RENDER_DEPTH,
};

// ARCH-10 round2: split the interaction-event pipeline cluster into its own module (moved out of the original lines 499-1528).
// Zero logic changes; only relocation + the necessary pub(crate) visibility. The module name events
// does not collide with any migrated fn.
// Re-export set: 6 setup_*_listener fns (called by render_dom.rs) + 6 CSS helpers (called by lib.rs node ops)
// — 12 items in total, cross-module visibility resolved via pub(crate) use, so the call sites in
// render_dom.rs/tests.rs stay untouched.
// ARCH-10 round-3: drag thread_locals (ACTIVE_DRAG_MOVE/PENDING_DRAG_UP/RESIZE_HANDLES) +
// DragMoveClosure + cleanup_pending_drag moved to events.rs (their main consumer). The lib.rs select_node
// path consumes them via a reverse use. R-1 comments moved along.
pub(crate) mod events;
#[allow(unused_imports)]
pub(crate) use events::{
    read_css_prop_value, read_node_position, read_node_size, replace_css_prop,
    setup_canvas_click_listener, setup_canvas_pan_listener, setup_canvas_zoom_listener,
    setup_delegated_click_listener, setup_delegated_mousedown_listener, setup_marquee_listener,
    strip_css_prop, update_node_position, RAF_SCHEDULED,
};
// ARCH-10 round-3: reverse import of the drag global state (definitions moved to events.rs; consumed by this crate's select_node path).
#[allow(unused_imports)]
pub(crate) use events::{cleanup_pending_drag, ACTIVE_DRAG_MOVE, PENDING_DRAG_UP, RESIZE_HANDLES};

fn css_escape_attr_value(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(crate) fn node_selector(node_id: &str) -> String {
    format!("[data-node-id=\"{}\"]", css_escape_attr_value(node_id))
}

// ── Global state ──

pub(crate) static SHELL: LazyLock<Mutex<Option<WebShellInner>>> =
    LazyLock::new(|| Mutex::new(None));

// Idempotence guard for container-level listeners.
// L-15: the old implementation used a process-global AtomicBool; after a container rebuild
// (new mount) the flag stayed true → the new container had no listeners and events broke.
// Switched to a per-container attribute marker, checked against the current container on every render.
#[cfg(target_arch = "wasm32")]
const LISTENERS_ATTR: &str = "data-fd-listeners";

// ── OPS-13: WASM log ring buffer ──
// Audit verdict: in-WKWebView field failures previously had zero diagnostics. The WASM sandbox has no
// filesystem and console.* is only visible in developer tools, leaving enterprise operators no access.
// Added a ring buffer capturing the most recent N error/warn entries; the host pulls them via the
// message bridge `log.capture.dump` → persisted on the Swift side (host-side handler tracked in
// fusion-studio#372, out of scope for this repo).
// Capacity 200 balances diagnostic coverage against linear memory use (~hundreds of bytes each, capped at ~100KB).

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

// Performance timestamp: on wasm32 use performance.now() (monotonic clock, milliseconds); non-wasm tests fill in 0.0.
// Date::now() is subject to host clock drift and restricted in some environments; performance.now() is more stable.
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

// OPS-13: error/warn logs are dual-written — console.* (visible in developer tools) + ring buffer (host can pull).
// The existing 32 bare console::error_1/warn_1 sites are not migrated one by one for now (large regression surface);
// key field-failure paths are wired first — panic hook + bridge failure + deserialization failure — the rest migrate incrementally.
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

/// Check whether the current container already has listeners installed; if not, mark it and return
/// false (installation needed); if already installed, return true.
pub(crate) fn mark_listeners_installed(container_id: &str) -> bool {
    // L-15: non-wasm targets have no DOM; return true (skip installation, safe under idempotence).
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = container_id;
        true
    }
    #[cfg(target_arch = "wasm32")]
    {
        let window = match web_sys::window() {
            Some(w) => w,
            None => return true, // no window means we cannot install; treat as installed to avoid repeated attempts
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

/// Fault-tolerant acquisition of the SHELL lock. R-15: on lock poisoning (panic while holding the
/// lock) **discard the poisoned dirty data** and reset to None instead of reading a half-updated
/// state — the next mount reinitializes, preventing the render layer from producing inconsistent
/// output based on dirty data. The old `into_inner()` approach was pseudo fault-tolerance: dirty
/// data may be half-updated and would keep rendering a wrong DOM/canvas.
pub(crate) fn shell_lock() -> std::sync::MutexGuard<'static, Option<WebShellInner>> {
    match SHELL.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            fd_log_error(
                "fd-host-web: SHELL lock poisoned (panic while holding the lock); discarding dirty data and resetting to None, next mount reinitializes",
            );
            let mut guard = poisoned.into_inner();
            *guard = None;
            guard
        }
    }
}

// ── WebShell ──

/// Host shell (wasm_bindgen exposed type).
#[wasm_bindgen]
pub struct WebShell {
    #[allow(dead_code)]
    inner: WebShellInner,
}

#[allow(dead_code)]
pub(crate) struct WebShellInner {
    pub(crate) canvas_id: String,
    ready: bool,
    // C11: cache the PenDocument JSON of the most recent render_dom, replacing DOM attribute storage.
    // viewport_cull_update reads from here, avoiding re-parsing the whole document from data-fd-doc every frame.
    pub(crate) cached_doc_json: Option<String>,
    // PERF-2: cache the parsed PenDocument so viewport_cull_update does not re-parse the JSON
    // on every rAF tick (O(document size) x frame rate). Written in sync with cached_doc_json;
    // json is the source of truth, doc is the cache. If doc=None while json=Some, fall back to
    // parsing once (safety net, should not trigger regularly).
    pub(crate) cached_doc: Option<PenDocument>,
    // P-7: current selected node id. select_node only clears the previously selected element + handles,
    // avoiding a global [data-fd-selected] scan. None = no selection.
    pub(crate) selected_id: Option<String>,
    // R-17: Shift multi-select set. toggle_node_selection adds/removes from this set;
    // select_node clears the set then inserts the single selection. Coexists with selected_id —
    // selected_id is the "primary selection" (handles anchor), selected_ids is the full multi-select state.
    pub(crate) selected_ids: std::collections::HashSet<String>,
}

/// Initialize the web host.
///
/// Verifies the canvas element exists, installs the panic hook, and returns a WebShell instance.
/// Mirrors the `<canvas>` validation logic of `op-host-web::mount`.
#[wasm_bindgen]
pub fn mount(canvas_id: &str) -> Result<WebShell, JsValue> {
    // R-A18: register the panic hook so panics land in console.error instead of a silent unreachable trap.
    // Otherwise any panic in an event callback kills the whole WebShell → permanent WKWebView white screen with no diagnostics.
    console_error_panic_hook::set_once();
    // OPS-13: panics are the field failures most in need of diagnostics, but console_error_panic_hook only
    // writes console.* (developer tools only; enterprise operators have no access). Wrap another layer:
    // take the panic_hook crate's hook as prev; on panic write to the ring buffer first (host can pull and
    // persist) then chain-call prev — the dual writes are not mutually exclusive.
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

    // Register the message listener
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

// ── Snap alignment ──

/// Snap threshold (px); below this distance, snapping applies automatically.
const SNAP_THRESHOLD: f32 = 5.0;
/// Default node width (fallback when parsing fails).
pub(crate) const DEFAULT_NODE_WIDTH: f32 = 100.0;
/// Default node height (fallback when parsing fails).
pub(crate) const DEFAULT_NODE_HEIGHT: f32 = 40.0;
/// Minimum marquee size (px); smaller than this is ignored.
pub(crate) const MIN_MARQUEE_SIZE: f32 = 5.0;

/// Collect snap candidate lines (edges + centers) for all nodes on the canvas.
/// Returns (x_lines, y_lines): the set of X coordinates of vertical snap lines and the set of Y
/// coordinates of horizontal snap lines.
/// P-6: geometry prefers reading data-fd-* cached attributes (O(1) attribute reads), falling back to style-string parsing when missing.
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
                // 6 snap lines: left/right/center-H + top/bottom/center-V
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

/// P-6: read an f32 from a data-* attribute; None when missing/invalid.
pub(crate) fn read_attr_f32(el: &web_sys::Element, attr: &str) -> Option<f32> {
    el.get_attribute(attr).and_then(|s| s.parse().ok())
}

/// Find the nearest snap offset for a single axis. Returns (snap offset, whether snapped, snap line coordinate).
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

/// Show snap guide lines (DOM overlay).
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
    // Create the snap-line overlay container
    let overlay = match document.create_element("div") {
        Ok(el) => el,
        Err(_) => {
            web_sys::console::warn_1(&"fd-host-web: show_snap_lines failed to create overlay".into());
            return;
        }
    };
    overlay.set_id("fd-snap-overlay");
    overlay.set_attribute("style",
        "position:absolute;top:0;left:0;width:100%;height:100%;pointer-events:none;z-index:9998;overflow:visible;"
    ).ok();

    // Vertical snap lines (X coordinates → vertical lines)
    for &x in x_lines {
        if let Ok(line) = document.create_element("div") {
            line.set_attribute("style", &format!(
                "position:absolute;left:{}px;top:0;width:1px;height:100%;background:#007AFF;opacity:0.5;",
                x
            )).ok();
            overlay.append_child(&line).ok();
        }
    }
    // Horizontal snap lines (Y coordinates → horizontal lines)
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

/// Hide snap guide lines.
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

// ── Bridge helpers ──

/// Inject design token CSS into the page :root.
pub(crate) fn apply_tokens_css(css: &str) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };

    // Remove the old token style
    if let Some(old) = document.get_element_by_id("fusion-tokens") {
        old.remove();
    }

    // C10: sanitize before injecting; strips @import/@font-face/url()/expression()/javascript:
    let safe = sanitize_token_css(css);

    // Inject the new token style
    let style_el = match document.create_element("style") {
        Ok(el) => el,
        Err(_) => {
            web_sys::console::warn_1(&"fd-host-web: apply_tokens_css failed to create <style>".into());
            return;
        }
    };
    style_el.set_id("fusion-tokens");
    style_el.set_text_content(Some(&safe));
    if let Some(head) = document.head() {
        head.append_child(&style_el).ok();
    } else {
        web_sys::console::warn_1(&"fd-host-web: apply_tokens_css document.head missing".into());
    }
}

// C10: strip dangerous CSS content, keeping only --token: value style declarations.
// Any line containing the following keywords is dropped (case-insensitive): @import / @font-face / expression( / url( / javascript:
// E-40/P3: strip /* */ comments before keyword matching to prevent comment-splitting bypasses like `u/**/rl(` evading the url() check.
// Returns the sanitized CSS; the stripped count is emitted via a warn log (the original payload is never echoed back).
fn strip_css_comments(line: &str) -> String {
    // Simple state machine stripping /* ... */; multi-line comments are handled per line (each line
    // independent); an unterminated comment is stripped to end of line.
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
            &format!("fd-host-web: sanitize_token_css stripped {stripped} dangerous CSS lines").into(),
        );
    }
    safe
}

// E-40/P3: pure-logic core; strips comments + drops whole lines containing dangerous keywords, returns (sanitized CSS, stripped line count).
// Split out so native tests can run it (non-wasm would otherwise hit the web_sys::console::warn_1 panic).
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
        // E-40: match after stripping comments; keep the original line (comments themselves are harmless, the danger is the comment-split keyword).
        let lowered = strip_css_comments(line).to_ascii_lowercase();
        if dangerous.iter().any(|needle| lowered.contains(needle)) {
            stripped += 1;
            continue;
        }
        out.push(line);
    }
    (out.join("\n"), stripped)
}

// E-39: CSS color value sanitization (single value, not a whole CSS block). mutate_node's fill/stroke come from
// arbitrary backend-message Strings; splicing them raw into `background-color:{f}` / `border:.. solid {s}` allows
// escaping the attribute and injecting arbitrary CSS (e.g. `red; } * { position:fixed; background:url(http://evil)`
// breaking the offline constraint).
// Reject url()/expression()/@import, strip `;{}<>` that can escape attribute boundaries, keep hex/named colors/rgb().
fn sanitize_css_color(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("url(")
        || lower.contains("expression(")
        || lower.contains("@import")
        || lower.contains("javascript:")
    {
        web_sys::console::warn_1(
            &format!("fd-host-web: CSS color value contains a dangerous function, downgrading to transparent: {raw}").into(),
        );
        return "transparent".to_string();
    }
    raw.chars()
        .filter(|&c| !matches!(c, ';' | '{' | '}' | '<' | '>'))
        .collect()
}

/// Select a node (add selection highlight + resize handles).
pub(crate) fn select_node(node_id: &str) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };

    // P-7: get the previously selected id from SHELL. On a hit, clear only that element, avoiding a
    // global [data-fd-selected] scan.
    // On a miss (cold start/state loss) still fall back to a full scan, guaranteeing any residual
    // selection is always cleared.
    let prev_id: Option<String> = shell_lock().as_ref().and_then(|i| i.selected_id.clone());
    let same_selection = prev_id.as_deref() == Some(node_id);

    if same_selection {
        // Re-selecting the same node: state and highlight are already in place; return early, no handle rebuild.
        return;
    }

    // Clear the previously selected element: just that one, no more querySelectorAll full scan.
    let clear_prev = |document: &web_sys::Document| {
        let clear_el = |el: &web_sys::Element| {
            // L4: on deselect only remove the box-shadow we injected; do not touch the user's outline.
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
                    // Previous element no longer in DOM (deleted/re-rendered): full-scan fallback to clear residual selection.
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
                // No record: full-scan fallback (first selection / after state loss).
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
        // Remove old resize handles (rebuilt only on selection change; same_selection already returned early).
        if let Ok(handles) = document.query_selector_all(".fd-resize-handle") {
            for i in 0..handles.length() {
                if let Some(h) = handles.item(i) {
                    if let Ok(el) = h.dyn_into::<web_sys::Element>() {
                        el.remove();
                    }
                }
            }
        }
        // R-1 backfill: DOM handles removed; also reclaim the previous batch of 8 on_handle_mousedown Closures
        // (the old .forget() leaked 8 per selection change, growing linearly over long sessions). clear drops all.
        RESIZE_HANDLES.with(|c| c.borrow_mut().clear());
    };
    clear_prev(&document);

    // Set the new selection
    if let Some(el) = document
        .query_selector(&node_selector(node_id))
        .unwrap_or(None)
    {
        el.set_attribute("data-fd-selected", "true").ok();
        // L4: use box-shadow for the selection highlight, keeping the node's own outline from being overridden.
        let style = el.get_attribute("style").unwrap_or_default();
        el.set_attribute("style", &format!("{};box-shadow:0 0 0 2px #007AFF;", style))
            .ok();

        // Add 8 resize handles
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
        // P-7/R-17: record the new selected id. select_node is single-select semantics — clear the
        // multi-select set then insert only this one.
        if let Some(inner) = shell_lock().as_mut() {
            inner.selected_ids.clear();
            inner.selected_ids.insert(node_id.to_string());
            inner.selected_id = Some(node_id.to_string());
        }
    } else {
        // Selected target does not exist: clear the records.
        if let Some(inner) = shell_lock().as_mut() {
            inner.selected_ids.clear();
            inner.selected_id = None;
        }
    }
}

/// Create a single resize handle DOM element.
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
        // R-1 backfill: reclaim leftover on_up/on_move from the previous round before starting a drag (missed mouseup case).
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
        // R-1: store on_move in a thread_local, take-reclaimed on mouseup (replaces the forget leak).
        ACTIVE_DRAG_MOVE.with(|c| *c.borrow_mut() = Some(on_move));
        let on_up = Closure::wrap(Box::new(move |event: web_sys::Event| {
            let Some(w) = web_sys::window() else {
                return;
            };
            let move_ref: &js_sys::Function = move_js.unchecked_ref();
            w.remove_event_listener_with_callback("mousemove", move_ref)
                .ok();
            // R-1: after removing the listener, take + drop the on_move Closure to reclaim linear memory.
            ACTIVE_DRAG_MOVE.with(|c| *c.borrow_mut() = None);
            // R-1 backfill: also reclaim on_up itself (PENDING_DRAG_UP).
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
        // R-1 backfill: on_up is stashed in PENDING_DRAG_UP (cleanup_pending_drag reclaims it if mouseup is missed).
        PENDING_DRAG_UP.with(|c| *c.borrow_mut() = Some(on_up));
        // R-1: on_move is stashed in SHELL (take-reclaimed on mouseup); no more forget.

        event.stop_propagation();
        event.prevent_default();
    }) as Box<dyn FnMut(web_sys::Event)>);
    handle
        .add_event_listener_with_callback("mousedown", on_handle_mousedown.as_ref().unchecked_ref())
        .ok();
    // R-1 backfill: the handle mousedown Closure is stashed in RESIZE_HANDLES (cleared by select_node before rebuilding).
    RESIZE_HANDLES.with(|c| c.borrow_mut().push(on_handle_mousedown));

    Some(handle)
}

/// Compute the new position/size for a resize direction.
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

/// Update a DOM element's width/height styles (live resize preview).
fn update_node_size(el: &web_sys::Element, w: f32, h: f32) {
    let style = el.get_attribute("style").unwrap_or_default();
    let new_style = replace_css_prop(&style, "width", &format!("{}px", w));
    let new_style = replace_css_prop(&new_style, "height", &format!("{}px", h));
    el.set_attribute("style", &new_style).ok();
    // P-6: refresh the size cache in sync.
    el.set_attribute("data-fd-w", &format!("{}", w)).ok();
    el.set_attribute("data-fd-h", &format!("{}", h)).ok();
}

/// MutateNode command handler: update node position/size/style.
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
                &format!("fd-host-web: mutate_node node not found {node_id}").into(),
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
            // P-6: refresh the x cache.
            el.set_attribute("data-fd-x", &format!("{}", nx)).ok();
        }
        if let Some(ny) = y {
            let style = el.get_attribute("style").unwrap_or_default();
            el.set_attribute(
                "style",
                &replace_css_prop(&style, "top", &format!("{}px", ny)),
            )
            .ok();
            // P-6: refresh the y cache.
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
            // P-6: refresh the w cache.
            el.set_attribute("data-fd-w", &format!("{}", nw)).ok();
        }
        if let Some(nh) = h {
            let style = el.get_attribute("style").unwrap_or_default();
            el.set_attribute(
                "style",
                &replace_css_prop(&style, "height", &format!("{}px", nh)),
            )
            .ok();
            // P-6: refresh the h cache.
            el.set_attribute("data-fd-h", &format!("{}", nh)).ok();
        }
    }

    // Style mutations
    // E-39: fill/stroke are sanitized via sanitize_css_color before injection, preventing CSS attribute escape from breaking the offline constraint.
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

/// Set node visibility: hidden applies display:none; showing restores the previous display.
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
            // L3: when restoring visibility, do not force display:block; restore from data-fd-prev-display.
            // If missing, strip the display declaration so CSS defaults apply, avoiding breaking flex/grid.
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
            // L3: before hiding, save the current display and restore it later, avoiding a hardcoded block that breaks layout.
            let style = el.get_attribute("style").unwrap_or_default();
            let prev_display = read_css_prop_value(&style, "display").unwrap_or_default();
            el.set_attribute("data-fd-prev-display", &prev_display).ok();
            let new_style = replace_css_prop(&style, "display", "none");
            el.set_attribute("style", &new_style).ok();
            web_sys::console::log_1(&format!("fd-host-web: node {node_id} set hidden").into());
        }
    }
}

/// Set node locked state: locked nodes cannot be dragged, with a dashed border as visual feedback.
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

/// Reorder a node: move the given node's DOM element to a new position.
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
        // L1: first detach the moved node from the parent container, then locate the insertion point
        // relative to the remaining children. Otherwise the moved node itself still occupies a slot,
        // shifting the target index by one and misordering.
        node.remove_child(&el).ok();
        // L-17: count the insertion point only among children carrying data-node-id, ignoring auxiliary
        // DOM such as resize handles / snap overlay / plan preview, otherwise the index desyncs from the node sequence.
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
        // L1: clamp to the valid range so an out-of-range new_index is silently dropped no more.
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

// ── Unit tests (host target, non wasm32) ──

#[cfg(test)]
mod tests;
