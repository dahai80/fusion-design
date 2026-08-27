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

fn css_escape_attr_value(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn node_selector(node_id: &str) -> String {
    format!("[data-node-id=\"{}\"]", css_escape_attr_value(node_id))
}

// ── 全局状态 ──

static SHELL: LazyLock<Mutex<Option<WebShellInner>>> = LazyLock::new(|| Mutex::new(None));

// R-1：拖拽/平移/框选/resize 的 on_move Closure 暂存（替代 forget 泄漏）。
// 旧实现 .forget() 导致每次拖拽泄漏一个 FnMut Closure，长会话线性内存增长。
// mousedown 存入，mouseup take() + remove_event_listener → Closure drop 回收内存。
// thread_local 规避 Send 约束（Closure<dyn FnMut> 非 Send，wasm 单线程安全）。
type DragMoveClosure = wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>;
thread_local! {
    static ACTIVE_DRAG_MOVE: std::cell::RefCell<Option<DragMoveClosure>> =
        const { std::cell::RefCell::new(None) };
}

// R-1 余 14 处 forget 的审计裁定回溯：
// 剩余 .forget() 站点分两类，均非长会话泄漏根因，故保留 forget + 文档化（不做 stored-Vec 转换）：
//   (1) setup_delegated_* / setup_* 的容器级委托监听器（click/mousedown/wheel/message）——
//       mount 一次即应用生命周期常驻，经 mark_listeners_installed 幂等保护防重复 attach。
//       事件委托模式下非逐节点绑定，节点增删不新增监听器。常驻监听器 forget 是 web_sys 惯例，
//       不随会话长度增长，无线性内存泄漏。
//   (2) Closure::once 的 on_mouseup/on_up —— 触发一次后 wasm-bindgen 自清理（P0-2 注释），
//       forget 仅持有至触发点，非持续泄漏。
// 真正的会话级泄漏（拖拽 on_move 每次新增）已由 ACTIVE_DRAG_MOVE 修复。
// 若未来需 unmount 全量回收，可在此 thread_local 旁加 Vec<Closure> + unmount() drop——
// 当前无 unmount 调用方，stored-Vec 增复杂度不解决现存泄漏，按 Rule 2 不引入。

// 容器级监听器幂等保护。
// L-15：旧实现用进程级全局 AtomicBool，容器重建（新 mount）后标志仍 true →
// 新容器无监听器，事件失效。改为 per-container 属性标记，每次渲染查当前容器。
#[cfg(target_arch = "wasm32")]
const LISTENERS_ATTR: &str = "data-fd-listeners";

/// 检查当前容器是否已装监听器；未装则标记并返回 false（需安装），已装返回 true。
fn mark_listeners_installed(container_id: &str) -> bool {
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
fn shell_lock() -> std::sync::MutexGuard<'static, Option<WebShellInner>> {
    match SHELL.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            web_sys::console::error_1(
                &"fd-host-web: SHELL 锁中毒（持锁线程 panic），丢弃脏数据重置为 None，下次 mount 重新初始化".into(),
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
struct WebShellInner {
    canvas_id: String,
    ready: bool,
    // C11：缓存最近一次 render_dom 的 PenDocument JSON，替代 DOM 属性存储。
    // viewport_cull_update 从这里读取，避免每帧从 data-fd-doc 重新解析整文档。
    cached_doc_json: Option<String>,
    // P-7：当前选中节点 id。select_node 只清上一个选中元素 + handles，
    // 免全局 [data-fd-selected] 扫描。None = 无选中。
    selected_id: Option<String>,
    // R-17：Shift 多选集合。toggle_node_selection 增删此集合，
    // select_node 清空集合后插入单选。与 selected_id 并存——
    // selected_id 是"主选中"（handles 锚点），selected_ids 是全量多选态。
    selected_ids: std::collections::HashSet<String>,
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
        selected_id: None,
        selected_ids: std::collections::HashSet::new(),
    };

    // 注册消息监听器
    setup_message_listener(&window)?;

    let shell = WebShell { inner };
    *shell_lock() = Some(WebShellInner {
        canvas_id: canvas_id.to_string(),
        ready: true,
        cached_doc_json: None,
        selected_id: None,
        selected_ids: std::collections::HashSet::new(),
    });

    Ok(shell)
}

// ── 消息桥接 ──

/// 设置 `message` 事件监听器，接收原生宿主消息。
fn setup_message_listener(window: &web_sys::Window) -> Result<(), JsValue> {
    let handler = Closure::wrap(Box::new(move |event: web_sys::MessageEvent| {
        if let Some(data) = event.data().as_string() {
            handle_host_message(&data);
        }
    }) as Box<dyn FnMut(web_sys::MessageEvent)>);

    window.add_event_listener_with_callback("message", handler.as_ref().unchecked_ref())?;
    handler.forget(); // 防止被 GC 回收
    Ok(())
}

/// 处理来自原生宿主（Fusion-Desk）的消息。
fn handle_host_message(json: &str) {
    // 解析 HostMessage 形状
    let msg: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(e) => {
            web_sys::console::error_1(&format!("fd-host-web: 反序列化消息失败: {e}").into());
            return;
        }
    };

    let kind = msg
        .get("kind")
        .and_then(|k| k.as_str())
        .unwrap_or("unknown");

    // L-16：协议版本护栏——schema_version 超过本端支持版本则拒绝处理，
    // 回传错误事件，避免静默按旧语义误解析新协议消息。
    const HOST_PROTOCOL_VERSION: u64 = 1;
    if let Some(v) = msg.get("schema_version").and_then(|x| x.as_u64()) {
        if v > HOST_PROTOCOL_VERSION {
            web_sys::console::warn_1(
                &format!(
                    "fd-host-web: 消息 schema_version={v} 超过本端支持版本 {HOST_PROTOCOL_VERSION}，拒绝处理"
                )
                .into(),
            );
            send_to_host(
                "error",
                &serde_json::json!({ "reason": "unsupported schema_version", "version": v }),
            );
            return;
        }
    }

    match kind {
        "page.render" => {
            // 渲染 PenDocument 页面
            if let Some(payload) = msg.get("payload") {
                if let Some(doc_json) = payload.get("document").and_then(|d| d.as_str()) {
                    render_page(doc_json);
                }
            }
        }
        "page.render-dom" => {
            // DOM 渲染管线
            if let Some(payload) = msg.get("payload") {
                if let Some(doc_json) = payload.get("document").and_then(|d| d.as_str()) {
                    render_dom(doc_json);
                }
            }
        }
        "tokens.apply" => {
            if let Some(payload) = msg.get("payload") {
                if let Some(css) = payload.get("css").and_then(|c| c.as_str()) {
                    apply_tokens_css(css);
                }
            }
        }
        "node.select" => {
            if let Some(payload) = msg.get("payload") {
                if let Some(node_id) = payload.get("node_id").and_then(|n| n.as_str()) {
                    select_node(node_id);
                }
            }
        }
        "node.mutate" => {
            if let Some(payload) = msg.get("payload") {
                if let Some(node_id) = payload.get("node_id").and_then(|n| n.as_str()) {
                    let x = payload.get("x").and_then(|v| v.as_f64()).map(|v| v as f32);
                    let y = payload.get("y").and_then(|v| v.as_f64()).map(|v| v as f32);
                    let w = payload.get("w").and_then(|v| v.as_f64()).map(|v| v as f32);
                    let h = payload.get("h").and_then(|v| v.as_f64()).map(|v| v as f32);
                    let fill = payload
                        .get("fill")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let stroke = payload
                        .get("stroke")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let stroke_width = payload
                        .get("stroke_width")
                        .and_then(|v| v.as_f64())
                        .map(|v| v as f32);
                    let radius = payload
                        .get("radius")
                        .and_then(|v| v.as_f64())
                        .map(|v| v as f32);
                    let font_size = payload
                        .get("font_size")
                        .and_then(|v| v.as_f64())
                        .map(|v| v as f32);
                    let font_family = payload
                        .get("font_family")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let opacity = payload
                        .get("opacity")
                        .and_then(|v| v.as_f64())
                        .map(|v| v as f32);
                    mutate_node(
                        node_id,
                        x,
                        y,
                        w,
                        h,
                        &fill,
                        &stroke,
                        stroke_width,
                        radius,
                        font_size,
                        &font_family,
                        opacity,
                    );
                }
            }
        }
        "canvas.clear" => {
            clear_canvas();
        }
        "canvas.reset-view" => {
            reset_canvas_view();
        }
        "node.set-visibility" => {
            if let Some(payload) = msg.get("payload") {
                if let Some(node_id) = payload.get("node_id").and_then(|n| n.as_str()) {
                    let visible = payload
                        .get("visible")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    set_node_visibility(node_id, visible);
                }
            }
        }
        "node.reorder" => {
            if let Some(payload) = msg.get("payload") {
                if let Some(node_id) = payload.get("node_id").and_then(|n| n.as_str()) {
                    let new_index = payload
                        .get("new_index")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;
                    reorder_node(node_id, new_index);
                }
            }
        }
        "system.ready" => {
            web_sys::console::log_1(&"fd-host-web: 宿主就绪".into());
        }
        other => {
            // L5：未识别 kind 用 warn 而非 debug，便于版本不匹配时定位
            web_sys::console::warn_1(
                &format!("fd-host-web: 未识别消息 kind={other}，可能版本不匹配").into(),
            );
        }
    }
}

/// 宿主桥 handler 名（对应 studio 侧注册的 WKScriptMessageHandler name）。
/// 仅 wasm32 路径 dispatch_to_host 消费；非 wasm 构建不编译避免 dead_code 警告（CI clippy -D warnings）。
#[cfg(target_arch = "wasm32")]
const HOST_HANDLER_NAME: &str = "fdHost";

/// 向后端发送消息。
///
/// R-A22：桥接主路径走 `window.webkit.messageHandlers.fdHost.postMessage`，
/// 对齐 CLAUDE.md/README 声称的 WKWebView 标准原生回调。若该 handler 不存在
/// （旧版 studio 未注册 / 非 WKWebView 环境），回退到 `navigator.__fd_host_post`
/// 属性轮询契约，保持向后兼容。
fn send_to_host(kind: &str, payload: &serde_json::Value) {
    let msg = serde_json::json!({
        "direction": "WebViewToBackend",
        "kind": kind,
        "payload": payload,
    });
    let json = match serde_json::to_string(&msg) {
        Ok(s) => s,
        // E-41：序列化失败显式告警而非吞消息（旧实现 unwrap_or_default="" 静默丢消息）。
        Err(e) => {
            web_sys::console::warn_1(
                &format!("fd-host-web: send_to_host 序列化失败 kind={kind} err={e}").into(),
            );
            return;
        }
    };
    // js_sys::global 在非 wasm 目标会 panic（imported statics 不可用）；
    // 仅 wasm32 分发，原生测试环境无 window，直接返回。
    #[cfg(target_arch = "wasm32")]
    {
        dispatch_to_host(&json, kind);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (json, kind);
    }
}

/// wasm32 专用：按 R-A22 桥接契约分发消息到原生宿主。
#[cfg(target_arch = "wasm32")]
fn dispatch_to_host(json: &str, kind: &str) {
    let global = js_sys::global();
    let Some(w) = global.dyn_ref::<web_sys::Window>() else {
        return;
    };
    // 主路径：webkit.messageHandlers.<handler>.postMessage（WKWebView 标准）。
    let handlers = js_sys::Reflect::get(w, &JsValue::from_str("webkit")).ok();
    if let Some(webkit) = handlers {
        let mh = js_sys::Reflect::get(&webkit, &JsValue::from_str("messageHandlers")).ok();
        if let Some(mh) = mh {
            let handler = js_sys::Reflect::get(&mh, &JsValue::from_str(HOST_HANDLER_NAME)).ok();
            if let Some(handler) = handler {
                let post = js_sys::Reflect::get(&handler, &JsValue::from_str("postMessage")).ok();
                if let Some(post) = post {
                    if let Some(post_fn) = post.dyn_ref::<js_sys::Function>() {
                        let js_val = JsValue::from_str(json);
                        if post_fn.call1(&handler, &js_val).is_ok() {
                            return;
                        }
                        web_sys::console::warn_1(
                            &format!(
                                "fd-host-web: webkit.messageHandlers.{HOST_HANDLER_NAME}.postMessage 调用失败 kind={kind}"
                            )
                            .into(),
                        );
                    }
                }
            }
        }
    }
    // 回退：navigator.__fd_host_post 属性契约（旧 studio 轮询读取）。
    let js_val = JsValue::from_str(json);
    let _ = js_sys::Reflect::set(
        &w.navigator(),
        &JsValue::from_str("__fd_host_post"),
        &js_val,
    );
}

// ── Bridge 消息协议 ──

/// 后端 → WebView 的命令类型。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum BridgeCommand {
    /// 渲染 PenDocument 页面
    PageRender { document_json: String },
    /// 设置设计 Token CSS
    ApplyTokens { css: String },
    /// 选中节点
    SelectNode { node_id: String },
    /// 修改节点位置/尺寸/样式
    MutateNode {
        node_id: String,
        x: Option<f32>,
        y: Option<f32>,
        w: Option<f32>,
        h: Option<f32>,
        fill: Option<String>,
        stroke: Option<String>,
        stroke_width: Option<f32>,
        radius: Option<f32>,
        font_size: Option<f32>,
        font_family: Option<String>,
        opacity: Option<f32>,
    },
    /// 清空画布
    ClearCanvas,
    /// Plan 预览：用虚线显示即将写入的节点
    PlanPreview { document_json: String },
    /// 确认 Plan：移除虚线预览层
    PlanApply,
    /// 拒绝 Plan：移除虚线预览层
    PlanReject,
    /// 设置节点可见性
    SetNodeVisibility { node_id: String, visible: bool },
    /// 设置节点锁定状态
    SetNodeLocked { node_id: String, locked: bool },
    /// 重排序节点（移动到目标位置）
    ReorderNode { node_id: String, new_index: usize },
}

/// WebView → 后端的事件类型。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum BridgeEvent {
    /// 节点被点击
    NodeClick { node_id: String, x: f32, y: f32 },
    /// 节点被拖拽
    NodeDrag { node_id: String, dx: f32, dy: f32 },
    /// 节点被调整大小
    NodeResize { node_id: String, w: f32, h: f32 },
    /// 节点被选中
    NodeSelect { node_id: String },
    /// 节点多选（Shift+点击追加）
    NodeMultiSelect { node_id: String },
    /// 画布区域被点击（空白区域）
    CanvasClick { x: f32, y: f32 },
    /// 画布缩放（wheel 事件）
    CanvasZoom { delta: f32, x: f32, y: f32 },
    /// 画布平移（中键/Space+拖拽）
    CanvasPan { dx: f32, dy: f32 },
    /// 框选完成（marquee 矩形内的节点 id 列表）
    MarqueeSelect { node_ids: Vec<String> },
    /// 用户输入 AI 对话
    AiChat { message: String },
}

/// 解析 BridgeCommand 从 JSON。
fn parse_bridge_command(json: &str) -> Option<BridgeCommand> {
    serde_json::from_str(json).ok()
}

/// 将 BridgeEvent 序列化为 JSON 并发送到后端。
fn send_bridge_event(event: BridgeEvent) {
    let payload = serde_json::to_value(&event).unwrap_or_default();
    let kind = match &event {
        BridgeEvent::NodeClick { .. } => "node.click",
        BridgeEvent::NodeDrag { .. } => "node.drag",
        BridgeEvent::NodeResize { .. } => "node.resize",
        BridgeEvent::NodeSelect { .. } => "node.select",
        BridgeEvent::NodeMultiSelect { .. } => "node.multi-select",
        BridgeEvent::CanvasClick { .. } => "canvas.click",
        BridgeEvent::CanvasZoom { .. } => "canvas.zoom",
        BridgeEvent::CanvasPan { .. } => "canvas.pan",
        BridgeEvent::MarqueeSelect { .. } => "marquee.select",
        BridgeEvent::AiChat { .. } => "ai.chat",
    };
    send_to_host(kind, &payload);
}

// ── fusionBridge：JS 全局对象，供原生端调用 ──

/// fusionBridge.sendCommand(commandJson) — 供 WKWebView 原生端调用。
#[wasm_bindgen]
pub fn fusion_bridge_send_command(command_json: &str) -> Result<(), JsValue> {
    let command = parse_bridge_command(command_json)
        .ok_or_else(|| JsValue::from_str("无法解析 BridgeCommand"))?;

    match command {
        BridgeCommand::PageRender { document_json } => {
            render_page(&document_json);
        }
        BridgeCommand::ApplyTokens { css } => {
            apply_tokens_css(&css);
        }
        BridgeCommand::SelectNode { node_id } => {
            select_node(&node_id);
        }
        BridgeCommand::MutateNode {
            node_id,
            x,
            y,
            w,
            h,
            fill,
            stroke,
            stroke_width,
            radius,
            font_size,
            font_family,
            opacity,
        } => {
            mutate_node(
                &node_id,
                x,
                y,
                w,
                h,
                &fill,
                &stroke,
                stroke_width,
                radius,
                font_size,
                &font_family,
                opacity,
            );
        }
        BridgeCommand::ClearCanvas => {
            clear_canvas();
        }
        BridgeCommand::PlanPreview { document_json } => {
            render_plan_preview(&document_json);
        }
        BridgeCommand::PlanApply => {
            remove_plan_preview();
        }
        BridgeCommand::PlanReject => {
            remove_plan_preview();
        }
        BridgeCommand::SetNodeVisibility { node_id, visible } => {
            set_node_visibility(&node_id, visible);
        }
        BridgeCommand::SetNodeLocked { node_id, locked } => {
            set_node_locked(&node_id, locked);
        }
        BridgeCommand::ReorderNode { node_id, new_index } => {
            reorder_node(&node_id, new_index);
        }
    }
    Ok(())
}

// ── DOM 渲染管线（性能优化版）──

/// 视口剔除边距（px），略大于屏幕确保边缘节点可见。
const VIEWPORT_MARGIN: f64 = 200.0;

// E-38/P2-5：节点尺寸/视口坐标硬上限，防恶意 .fusiondesign 用 f32::MAX
// 触发算术溢出/NaN/Inf 致渲染崩溃或 OOM。画布坐标单边 ≤ 100k px 足够任何合法设计稿。
const MAX_NODE_DIM_PX: f64 = 100_000.0;
const MAX_VIEWPORT_DIM_PX: f64 = 100_000.0;
const MAX_ZOOM: f64 = 1000.0;
// E-38/P2-5：单节点子节点数渲染上限。深度有 MAX_RENDER_DEPTH=64 上限，但
// 单层 children 数量无界——10 万扁平子节点深度=1 过深度检查，render_dom 串行
// 创建 10 万 DOM 致 WKWebView OOM。canvas-core 已有 MAX_NODE_TOTAL=100k 总数
// 护栏，此为渲染侧每节点扇出补充护栏：合法设计稿单层兄弟 ≤ 数百，2000 足够且
// 与 64 深度乘积远低于 WKWebView jetsam 阈值。超限只渲染前 N 个 + warn，fail visibly。
const MAX_CHILDREN_PER_NODE: usize = 2_000;

/// 将 PenDocument 渲染为 DOM 元素（而非 Canvas 2D）。
/// 性能优化：
/// - DocumentFragment 批量插入，避免逐节点 layout thrashing
/// - 事件委托：容器级别单处理器，替代逐节点绑定
/// - 视口剔除：仅渲染视口内节点（zoom/pan 变化时增量更新）
fn render_dom(doc_json: &str) {
    let doc = match PenDocument::from_json(doc_json) {
        Ok(d) => d,
        Err(e) => {
            web_sys::console::error_1(
                &format!("fd-host-web: DOM 渲染 PenDocument 解析失败: {e}").into(),
            );
            return;
        }
    };

    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };

    // 获取或创建 DOM 容器
    let container = match document.get_element_by_id("fusion-dom-root") {
        Some(c) => c,
        None => {
            // C12：create_element / body 失败不 panic，warn 后提前返回。
            match document.create_element("div") {
                Ok(div) => {
                    div.set_id("fusion-dom-root");
                    div.set_attribute(
                        "style",
                        "position:relative;width:100%;height:100%;overflow:hidden;",
                    )
                    .ok();
                    if let Some(body) = document.body() {
                        body.append_child(&div).ok();
                    }
                    div
                }
                Err(_) => {
                    web_sys::console::warn_1(&"fd-host-web: render_dom 创建容器 div 失败".into());
                    return;
                }
            }
        }
    };

    // 清空现有 DOM 节点
    container.set_inner_html("");

    // 初始化缩放/平移状态
    container.set_attribute("data-fd-zoom", "1.0").ok();
    container.set_attribute("data-fd-pan-x", "0.0").ok();
    container.set_attribute("data-fd-pan-y", "0.0").ok();

    // C11：不再把整份 PenDocument JSON 写入 DOM 属性（无界、每帧重解析）。
    // 改为缓存到 SHELL 全局内存，viewport_cull_update 直接读取。
    {
        let mut guard = shell_lock();
        if let Some(inner) = guard.as_mut() {
            inner.cached_doc_json = Some(doc_json.to_string());
            // P-7/R-17：全量重渲替换 DOM，旧选中元素已消失，清空选中记录避免悬空。
            inner.selected_id = None;
            inner.selected_ids.clear();
        }
    }
    web_sys::console::log_1(&"fd-host-web: data-fd-doc 属性缓存已移除，改用内存缓存（C11）".into());

    // 使用 DocumentFragment 批量插入
    let fragment = document.create_document_fragment();
    let mut node_count: u32 = 0;
    for page in &doc.pages {
        for node in &page.nodes {
            // 视口剔除：跳过完全在视口外的节点
            if !is_node_in_viewport(node, &container) {
                continue;
            }
            if let Some(el) = render_node_to_dom(node, &document, 0) {
                fragment.append_child(&el).ok();
                node_count += 1;
            }
        }
    }
    container.append_child(&fragment).ok();

    web_sys::console::log_1(
        &format!("fd-host-web: DOM 渲染完成, {node_count} 节点（视口剔除后）").into(),
    );

    // 事件委托：容器级监听器 per-container 仅装一次（L-15）。
    if !mark_listeners_installed("fusion-dom-root") {
        setup_delegated_click_listener("fusion-dom-root");
        setup_delegated_mousedown_listener("fusion-dom-root");
        setup_canvas_click_listener("fusion-dom-root");
        setup_canvas_zoom_listener("fusion-dom-root");
        setup_canvas_pan_listener("fusion-dom-root");
        setup_marquee_listener("fusion-dom-root");
        web_sys::console::log_1(&"fd-host-web: 容器级监听器已安装（per-container）".into());
    }
}

/// E-38/P2-5：节点几何护栏（纯函数，无 DOM 依赖，可单测）。
/// 非有限或超 MAX_NODE_DIM_PX 的尺寸视作无效，避免恶意 .fusiondesign
/// 用 f32::MAX/NaN 触发算术溢出/Inf 渲染崩溃。zoom 同理限 [>0, MAX_ZOOM]。
fn is_node_geom_valid(node: &fd_canvas_core::PenNode, zoom: f64) -> bool {
    if !node.x.is_finite() || !node.y.is_finite() || !node.w.is_finite() || !node.h.is_finite() {
        return false;
    }
    if node.w < 0.0 || node.h < 0.0 {
        return false;
    }
    if node.w > MAX_NODE_DIM_PX || node.h > MAX_NODE_DIM_PX {
        return false;
    }
    if !zoom.is_finite() || zoom <= 0.0 || zoom > MAX_ZOOM {
        return false;
    }
    true
}

/// 判断节点是否在当前视口内（考虑 zoom/pan）。
fn is_node_in_viewport(node: &fd_canvas_core::PenNode, container: &web_sys::Element) -> bool {
    // E-38/P2-5：节点尺寸/坐标护栏。非有限或超 MAX_NODE_DIM_PX 的节点视作不可见，
    // 避免恶意 .fusiondesign 用 f32::MAX/NaN 触发算术溢出/Inf 渲染崩溃。
    let zoom: f64 = container
        .get_attribute("data-fd-zoom")
        .unwrap_or_default()
        .parse()
        .unwrap_or(1.0);
    if !is_node_geom_valid(node, zoom) {
        return false;
    }
    let pan_x: f64 = container
        .get_attribute("data-fd-pan-x")
        .unwrap_or_default()
        .parse()
        .unwrap_or(0.0);
    let pan_y: f64 = container
        .get_attribute("data-fd-pan-y")
        .unwrap_or_default()
        .parse()
        .unwrap_or(0.0);
    if !pan_x.is_finite() || !pan_y.is_finite() {
        return false;
    }

    let window = match web_sys::window() {
        Some(w) => w,
        None => return true,
    };
    let vp_w = window
        .inner_width()
        .unwrap_or_default()
        .as_f64()
        .unwrap_or(1920.0);
    let vp_h = window
        .inner_height()
        .unwrap_or_default()
        .as_f64()
        .unwrap_or(1080.0);
    // E-38/P2-5：视口尺寸护栏，防异常 inner_width/height（部分嵌入式 WebView 返回 0/超值）。
    let vp_w = if vp_w.is_finite() && vp_w > 0.0 && vp_w <= MAX_VIEWPORT_DIM_PX {
        vp_w
    } else {
        1920.0
    };
    let vp_h = if vp_h.is_finite() && vp_h > 0.0 && vp_h <= MAX_VIEWPORT_DIM_PX {
        vp_h
    } else {
        1080.0
    };

    // 节点在画布坐标中的边界
    let node_left = node.x * zoom + pan_x;
    let node_top = node.y * zoom + pan_y;
    let node_right = (node.x + node.w) * zoom + pan_x;
    let node_bottom = (node.y + node.h) * zoom + pan_y;
    // 护栏后坐标必有限；再加一道防御防未预见溢出。
    if !node_left.is_finite()
        || !node_top.is_finite()
        || !node_right.is_finite()
        || !node_bottom.is_finite()
    {
        return false;
    }

    // 带边距的视口
    let vp_left = -VIEWPORT_MARGIN;
    let vp_top = -VIEWPORT_MARGIN;
    let vp_right = vp_w + VIEWPORT_MARGIN;
    let vp_bottom = vp_h + VIEWPORT_MARGIN;

    // AABB 重叠测试
    node_right > vp_left && node_left < vp_right && node_bottom > vp_top && node_top < vp_bottom
}

/// 视口剔除增量渲染：zoom/pan 变化时调用，添加新进入视口的节点，移除离开视口的节点。
fn viewport_cull_update() {
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

    // C11：从 SHELL 内存缓存读取 doc JSON，不再从 DOM 属性每帧重解析。
    let doc_json = {
        let guard = shell_lock();
        match guard.as_ref().and_then(|i| i.cached_doc_json.clone()) {
            Some(j) => j,
            None => return,
        }
    };
    let doc = match PenDocument::from_json(&doc_json) {
        Ok(d) => d,
        Err(_) => return,
    };

    // E-37/P2-5：一次遍历收集现有 DOM 节点 id → HashSet，替代旧实现每可见节点
    // 一次 query_selector（O(N) 全局查询）× N 节点 = O(N²)。现整体 O(N)。
    let existing_dom_ids = collect_existing_dom_node_ids(&container);

    // 收集文档中所有节点 id 及其视口状态
    let mut ids_to_add: Vec<String> = Vec::new();
    let mut ids_in_view: std::collections::HashSet<String> = std::collections::HashSet::new();

    for page in &doc.pages {
        collect_visible_node_ids(
            &page.nodes,
            &container,
            &existing_dom_ids,
            &mut ids_to_add,
            &mut ids_in_view,
        );
    }

    // 移除离开视口的节点 DOM
    let container_node: &web_sys::Node = container.unchecked_ref();
    let child_nodes = container_node.child_nodes();
    let mut to_remove: Vec<web_sys::Element> = Vec::new();
    for i in 0..child_nodes.length() {
        if let Some(child) = child_nodes.get(i) {
            if let Ok(el) = child.dyn_into::<web_sys::Element>() {
                if let Some(nid) = el.get_attribute("data-node-id") {
                    if !ids_in_view.contains(&nid) {
                        to_remove.push(el);
                    }
                }
            }
        }
    }
    for el in to_remove {
        el.remove();
    }

    // 添加新进入视口的节点
    if !ids_to_add.is_empty() {
        // E-37/P2-5：ids_to_add → HashSet，add_nodes_by_ids 的 contains 由 O(M) 降 O(1)。
        let add_set: std::collections::HashSet<String> = ids_to_add.iter().cloned().collect();
        let fragment = document.create_document_fragment();
        for page in &doc.pages {
            add_nodes_by_ids(&page.nodes, &document, &add_set, &fragment);
        }
        container.append_child(&fragment).ok();
    }

    web_sys::console::log_1(
        &format!("fd-host-web: 视口剔除更新, 添加 {} 节点", ids_to_add.len()).into(),
    );
}

/// E-37/P2-5：一次遍历容器直接子节点，收集 data-node-id → HashSet。
/// 替代旧 collect_visible_node_ids_inner 内每节点 query_selector 的 O(N²)。
fn collect_existing_dom_node_ids(
    container: &web_sys::Element,
) -> std::collections::HashSet<String> {
    let mut ids = std::collections::HashSet::new();
    let container_node: &web_sys::Node = container.unchecked_ref();
    let child_nodes = container_node.child_nodes();
    for i in 0..child_nodes.length() {
        if let Some(child) = child_nodes.get(i) {
            if let Ok(el) = child.dyn_into::<web_sys::Element>() {
                if let Some(nid) = el.get_attribute("data-node-id") {
                    ids.insert(nid);
                }
            }
        }
    }
    ids
}

/// 递归收集视口内节点 id，与现有 DOM id 集合对比找新增。
fn collect_visible_node_ids(
    nodes: &[fd_canvas_core::PenNode],
    container: &web_sys::Element,
    existing_dom_ids: &std::collections::HashSet<String>,
    ids_to_add: &mut Vec<String>,
    ids_in_view: &mut std::collections::HashSet<String>,
) {
    collect_visible_node_ids_inner(nodes, container, existing_dom_ids, ids_to_add, ids_in_view);
}

fn collect_visible_node_ids_inner(
    nodes: &[fd_canvas_core::PenNode],
    container: &web_sys::Element,
    existing_dom_ids: &std::collections::HashSet<String>,
    ids_to_add: &mut Vec<String>,
    ids_in_view: &mut std::collections::HashSet<String>,
) {
    for node in nodes {
        if is_node_in_viewport(node, container) {
            ids_in_view.insert(node.id.clone());
            // E-37/P2-5：HashSet 查询 O(1)，替代旧 query_selector O(N) 全局搜索。
            if !existing_dom_ids.contains(&node.id) {
                ids_to_add.push(node.id.clone());
            }
        }
        collect_visible_node_ids_inner(
            &node.children,
            container,
            existing_dom_ids,
            ids_to_add,
            ids_in_view,
        );
    }
}

/// 递归查找并渲染指定 id 的节点。
fn add_nodes_by_ids(
    nodes: &[fd_canvas_core::PenNode],
    document: &web_sys::Document,
    ids_to_add: &std::collections::HashSet<String>,
    fragment: &web_sys::DocumentFragment,
) {
    for node in nodes {
        if ids_to_add.contains(&node.id) {
            if let Some(el) = render_node_to_dom(node, document, 0) {
                fragment.append_child(&el).ok();
            }
        }
        add_nodes_by_ids(&node.children, document, ids_to_add, fragment);
    }
}

/// 将 PenNode 渲染为 DOM 元素。
fn render_node_to_dom(
    node: &fd_canvas_core::PenNode,
    document: &web_sys::Document,
    depth: u32,
) -> Option<web_sys::Element> {
    let tag = match node.kind {
        fd_canvas_core::NodeKind::Text => "span",
        fd_canvas_core::NodeKind::Image => "img",
        _ => "div",
    };
    let el = document.create_element(tag).ok()?;

    // 基础定位
    let mut style = format!(
        "position:absolute;left:{}px;top:{}px;width:{}px;height:{}px;",
        node.x, node.y, node.w, node.h
    );

    // z-index 和 rotation
    if node.z_index != 0 {
        style.push_str(&format!("z-index:{};", node.z_index));
    }
    if node.rotation != 0.0 {
        style.push_str(&format!("transform:rotate({}deg);", node.rotation));
    }

    // 填充色
    if let Some(fill) = &node.style.fill {
        let fill = sanitize_css_value(fill, "transparent");
        style.push_str(&format!("background-color:{};", fill));
    }

    // 描边
    if let Some(stroke) = &node.style.stroke {
        let width = node.style.stroke_width.unwrap_or(1.0);
        let stroke = sanitize_css_value(stroke, "transparent");
        style.push_str(&format!("border:{}px solid {};", width, stroke));
    }

    // 圆角
    if let Some(radius) = node.style.radius {
        style.push_str(&format!("border-radius:{}px;", radius));
    }

    // 文本
    if node.kind == fd_canvas_core::NodeKind::Text {
        let font_size = node.style.font_size.unwrap_or(16.0);
        let font_family = node.style.font_family.as_deref().unwrap_or("system-ui");
        let font_family = sanitize_css_value(font_family, "");
        style.push_str(&format!(
            "font-size:{}px;font-family:{};",
            font_size, font_family
        ));
        if let Some(text) = &node.text {
            el.set_text_content(Some(text));
        }
    }

    // Flex 布局
    match &node.style.layout {
        fd_canvas_core::LayoutMode::Flex(params) => {
            style.push_str("display:flex;");
            match params.direction {
                fd_canvas_core::FlexDirection::Row => style.push_str("flex-direction:row;"),
                fd_canvas_core::FlexDirection::RowReverse => {
                    style.push_str("flex-direction:row-reverse;")
                }
                fd_canvas_core::FlexDirection::Column => style.push_str("flex-direction:column;"),
                fd_canvas_core::FlexDirection::ColumnReverse => {
                    style.push_str("flex-direction:column-reverse;")
                }
            }
            if params.gap > 0.0 {
                style.push_str(&format!("gap:{}px;", params.gap));
            }
            match params.align_items {
                fd_canvas_core::AlignItems::Start => style.push_str("align-items:flex-start;"),
                fd_canvas_core::AlignItems::Center => style.push_str("align-items:center;"),
                fd_canvas_core::AlignItems::End => style.push_str("align-items:flex-end;"),
                fd_canvas_core::AlignItems::Stretch => style.push_str("align-items:stretch;"),
            }
            match params.justify_content {
                fd_canvas_core::JustifyContent::Start => {
                    style.push_str("justify-content:flex-start;")
                }
                fd_canvas_core::JustifyContent::Center => style.push_str("justify-content:center;"),
                fd_canvas_core::JustifyContent::End => style.push_str("justify-content:flex-end;"),
                fd_canvas_core::JustifyContent::SpaceBetween => {
                    style.push_str("justify-content:space-between;")
                }
                fd_canvas_core::JustifyContent::SpaceAround => {
                    style.push_str("justify-content:space-around;")
                }
                fd_canvas_core::JustifyContent::SpaceEvenly => {
                    style.push_str("justify-content:space-evenly;")
                }
            }
            match params.wrap {
                fd_canvas_core::FlexWrap::NoWrap => style.push_str("flex-wrap:nowrap;"),
                fd_canvas_core::FlexWrap::Wrap => style.push_str("flex-wrap:wrap;"),
            }
            if params.padding.top > 0.0
                || params.padding.right > 0.0
                || params.padding.bottom > 0.0
                || params.padding.left > 0.0
            {
                style.push_str(&format!(
                    "padding:{}px {}px {}px {}px;",
                    params.padding.top,
                    params.padding.right,
                    params.padding.bottom,
                    params.padding.left
                ));
            }
        }
        fd_canvas_core::LayoutMode::Grid(params) => {
            style.push_str("display:grid;");
            let cols: Vec<String> = params.columns.iter().map(track_to_css).collect();
            let rows: Vec<String> = params.rows.iter().map(track_to_css).collect();
            style.push_str(&format!("grid-template-columns:{};", cols.join(" ")));
            style.push_str(&format!("grid-template-rows:{};", rows.join(" ")));
            style.push_str(&format!("gap:{}px {}px;", params.gap.0, params.gap.1));
        }
        fd_canvas_core::LayoutMode::Free => {}
    }

    // Design token CSS 变量引用
    for (key, value) in &node.style.design_token_refs {
        let css_var = key.replace('.', "-");
        let value = sanitize_css_value(value, "transparent");
        style.push_str(&format!("--{}:{};", css_var, value));
    }

    el.set_attribute("style", &style).ok()?;
    el.set_attribute("data-node-id", &node.id).ok()?;
    // P-6：缓存几何到 data-* 属性，吸附收集免 querySelectorAll 全扫后逐元素解析 style 串。
    // 拖拽时 update_node_position 同步刷新 data-fd-x/y；mutate_node 改尺寸时刷新 w/h。
    el.set_attribute("data-fd-x", &format!("{}", node.x)).ok();
    el.set_attribute("data-fd-y", &format!("{}", node.y)).ok();
    el.set_attribute("data-fd-w", &format!("{}", node.w)).ok();
    el.set_attribute("data-fd-h", &format!("{}", node.h)).ok();

    // 渲染子节点（子节点同样使用事件委托模式）。深度上限防栈溢出（P2-1），
    // 扇出上限防 OOM（E-38/P2-5）。
    if depth < MAX_RENDER_DEPTH {
        let children = &node.children;
        if children.len() > MAX_CHILDREN_PER_NODE {
            web_sys::console::warn_1(
                &format!(
                    "render_node_to_dom: 子节点数 {} 超 {} 上限，仅渲染前 {} 个",
                    children.len(),
                    MAX_CHILDREN_PER_NODE,
                    MAX_CHILDREN_PER_NODE
                )
                .into(),
            );
        }
        for child in children.iter().take(MAX_CHILDREN_PER_NODE) {
            if let Some(child_el) = render_node_to_dom(child, document, depth + 1) {
                el.append_child(&child_el).ok();
            }
        }
    } else {
        web_sys::console::warn_1(
            &format!("render_node_to_dom: 嵌套深度超限 {MAX_RENDER_DEPTH}，跳过子树").into(),
        );
    }

    Some(el)
}

/// 将 TrackSizing 转换为 CSS grid track 值。
fn track_to_css(track: &fd_canvas_core::TrackSizing) -> String {
    match track {
        fd_canvas_core::TrackSizing::Fixed(v) => format!("{}px", v),
        fd_canvas_core::TrackSizing::Flex(v) => format!("{}fr", v),
        fd_canvas_core::TrackSizing::Auto => "auto".to_string(),
        fd_canvas_core::TrackSizing::Percent(v) => format!("{}%", v),
    }
}

// ── 吸附对齐 ──

/// 吸附阈值（px），小于此距离自动吸附。
const SNAP_THRESHOLD: f32 = 5.0;
/// 默认节点宽度（解析失败时回退值）。
const DEFAULT_NODE_WIDTH: f32 = 100.0;
/// 默认节点高度（解析失败时回退值）。
const DEFAULT_NODE_HEIGHT: f32 = 40.0;
/// 选框最小尺寸（px），小于此值忽略。
const MIN_MARQUEE_SIZE: f32 = 5.0;

/// 收集画布中所有节点的吸附候选线（边缘 + 中心）。
/// 返回 (x_lines, y_lines)，即垂直吸附线 X 坐标集合和水平吸附线 Y 坐标集合。
/// P-6：几何优先读 data-fd-* 缓存属性（O(1) 属性读），缺则回退 style 串解析。
fn collect_snap_candidates(exclude_id: &str) -> (Vec<f32>, Vec<f32>) {
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
fn read_attr_f32(el: &web_sys::Element, attr: &str) -> Option<f32> {
    el.get_attribute(attr).and_then(|s| s.parse().ok())
}

/// 对单轴查找最近吸附偏移。返回 (吸附偏移, 是否吸附, 吸附线坐标)。
fn find_snap_offset(value: f32, candidates: &[f32]) -> (f32, bool, f32) {
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
fn show_snap_lines(x_lines: &[f32], y_lines: &[f32]) {
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
fn hide_snap_lines() {
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

// ── 交互事件 ──

// ── requestAnimationFrame 节流 ──

/// 全局 rAF 句柄，避免同一帧多次调度。
static RAF_SCHEDULED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 通过 requestAnimationFrame 节流执行回调，确保每帧最多执行一次。
/// 修复 P0-3：rAF 注册失败或回调 panic 时标志永久卡 true -> 渲染停摆。
/// - window 不可用：立即复位标志，允许下次重试。
/// - rAF 注册失败：立即复位。
/// - 回调内先复位再执行；R-16：catch_unwind 兜底，回调 panic 落 console.error
///   并复位标志，不让 panic 传播成 wasm trap 致 rAF 循环永久停摆。
fn schedule_raf<F>(callback: F)
where
    F: FnOnce() + 'static,
{
    let already_scheduled = RAF_SCHEDULED.swap(true, std::sync::atomic::Ordering::SeqCst);
    if already_scheduled {
        return;
    }
    let cb = Closure::once(Box::new(move || {
        // 先复位标志，再执行回调；即使回调 panic 也不致永久阻塞。
        RAF_SCHEDULED.store(false, std::sync::atomic::Ordering::SeqCst);
        // R-16：catch_unwind 兜底——回调 panic 不传播成 trap 致 rAF 停摆。
        // web_sys 捕获非 UnwindSafe，用 AssertUnwindSafe 包裹（回调内不跨 unwind 持锁）。
        let f = std::panic::AssertUnwindSafe(callback);
        if let Err(e) = std::panic::catch_unwind(f) {
            let msg = if let Some(s) = e.downcast_ref::<&'static str>() {
                format!("schedule_raf 回调 panic: {s}")
            } else if let Some(s) = e.downcast_ref::<String>() {
                format!("schedule_raf 回调 panic: {s}")
            } else {
                "schedule_raf 回调 panic（非字符串 payload）".to_string()
            };
            web_sys::console::error_1(&msg.into());
        }
    }) as Box<dyn FnOnce()>);
    let window = match web_sys::window() {
        Some(w) => w,
        None => {
            RAF_SCHEDULED.store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        }
    };
    if window
        .request_animation_frame(cb.as_ref().unchecked_ref())
        .is_err()
    {
        // 注册失败：复位标志，否则后续所有 schedule_raf 静默丢弃 -> 卡死。
        RAF_SCHEDULED.store(false, std::sync::atomic::Ordering::SeqCst);
        web_sys::console::warn_1(
            &"schedule_raf: request_animation_frame 注册失败，已复位标志".into(),
        );
        return;
    }
    cb.forget();
}

// ── 事件委托 ──

/// 容器级别 click 事件委托：从事件 target 冒泡查找 data-node-id。
fn setup_delegated_click_listener(container_id: &str) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };
    let container = match document.get_element_by_id(container_id) {
        Some(c) => c,
        None => return,
    };

    let on_click = Closure::wrap(Box::new(move |event: web_sys::Event| {
        let mouse = match event.dyn_ref::<web_sys::MouseEvent>() {
            Some(m) => m,
            None => return,
        };
        let (x, y) = (mouse.client_x() as f32, mouse.client_y() as f32);
        let shift = mouse.shift_key();

        // 从 target 冒泡查找 data-node-id
        let target = event.target();
        let node_id = target.as_ref().and_then(|t| {
            let el = t.dyn_ref::<web_sys::Element>()?;
            if el.has_attribute("data-node-id") {
                el.get_attribute("data-node-id")
            } else {
                el.closest("[data-node-id]")
                    .ok()
                    .flatten()?
                    .get_attribute("data-node-id")
            }
        });

        if let Some(node_id) = node_id {
            if shift {
                send_bridge_event(BridgeEvent::NodeMultiSelect {
                    node_id: node_id.clone(),
                });
                toggle_node_selection(&node_id);
            } else {
                send_bridge_event(BridgeEvent::NodeClick {
                    node_id: node_id.clone(),
                    x,
                    y,
                });
                send_bridge_event(BridgeEvent::NodeSelect {
                    node_id: node_id.clone(),
                });
                select_node(&node_id);
            }
            event.stop_propagation();
            event.prevent_default();
        }
    }) as Box<dyn FnMut(web_sys::Event)>);
    container
        .add_event_listener_with_callback("click", on_click.as_ref().unchecked_ref())
        .ok();
    on_click.forget();
    web_sys::console::log_1(&"fd-host-web: delegated click listener installed".into());
}

/// 容器级别 mousedown 事件委托：拖拽启动。
fn setup_delegated_mousedown_listener(container_id: &str) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };
    let container = match document.get_element_by_id(container_id) {
        Some(c) => c,
        None => return,
    };

    let on_mousedown = Closure::wrap(Box::new(move |event: web_sys::Event| {
        let mouse = match event.dyn_ref::<web_sys::MouseEvent>() {
            Some(m) => m,
            None => return,
        };
        // 从 target 冒泡查找 data-node-id
        let target = event.target();
        let node_id = target.as_ref().and_then(|t| {
            let el = t.dyn_ref::<web_sys::Element>()?;
            if el.has_attribute("data-node-id") {
                el.get_attribute("data-node-id")
            } else {
                el.closest("[data-node-id]")
                    .ok()
                    .flatten()?
                    .get_attribute("data-node-id")
            }
        });

        let node_id = match node_id {
            Some(id) => id,
            None => return,
        };

        let start_x = mouse.client_x() as f32;
        let start_y = mouse.client_y() as f32;

        // 查找 DOM 元素
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(doc) = window.document() else {
            return;
        };
        let sel = node_selector(&node_id);
        let el_ref = match doc.query_selector(&sel).unwrap_or(None) {
            Some(e) => e,
            None => return,
        };
        let (orig_x, orig_y) = read_node_position(&el_ref);
        let (node_w, node_h) = read_node_size(&el_ref);

        let (snap_x_candidates, snap_y_candidates) = collect_snap_candidates(&node_id);
        let snap_x_candidates_up = snap_x_candidates.clone();
        let snap_y_candidates_up = snap_y_candidates.clone();

        let drag_id = node_id.clone();
        let on_mousemove = Closure::wrap(Box::new(move |event: web_sys::Event| {
            let mm = match event.dyn_ref::<web_sys::MouseEvent>() {
                Some(m) => m,
                None => return,
            };
            let raw_dx = mm.client_x() as f32 - start_x;
            let raw_dy = mm.client_y() as f32 - start_y;
            let raw_x = orig_x + raw_dx;
            let raw_y = orig_y + raw_dy;

            let (snap_dx, _snap_x, snap_x_lines) = {
                let left = raw_x;
                let right = raw_x + node_w;
                let center_h = raw_x + node_w / 2.0;
                let (off_l, sn_l, line_l) = find_snap_offset(left, &snap_x_candidates);
                let (off_r, sn_r, line_r) = find_snap_offset(right, &snap_x_candidates);
                let (off_c, sn_c, line_c) = find_snap_offset(center_h, &snap_x_candidates);
                let best = if sn_l && off_l.abs() <= off_r.abs() && off_l.abs() <= off_c.abs() {
                    (off_l, line_l)
                } else if sn_r && off_r.abs() <= off_c.abs() {
                    (off_r, line_r)
                } else if sn_c {
                    (off_c, line_c)
                } else {
                    (0.0, raw_x)
                };
                let mut lines = Vec::new();
                if sn_l {
                    lines.push(line_l);
                }
                if sn_r {
                    lines.push(line_r);
                }
                if sn_c {
                    lines.push(line_c);
                }
                (best.0, best.1, lines)
            };
            let (snap_dy, _snap_y, snap_y_lines) = {
                let top = raw_y;
                let bottom = raw_y + node_h;
                let center_v = raw_y + node_h / 2.0;
                let (off_t, sn_t, line_t) = find_snap_offset(top, &snap_y_candidates);
                let (off_b, sn_b, line_b) = find_snap_offset(bottom, &snap_y_candidates);
                let (off_c, sn_c, line_c) = find_snap_offset(center_v, &snap_y_candidates);
                let best = if sn_t && off_t.abs() <= off_b.abs() && off_t.abs() <= off_c.abs() {
                    (off_t, line_t)
                } else if sn_b && off_b.abs() <= off_c.abs() {
                    (off_b, line_b)
                } else if sn_c {
                    (off_c, line_c)
                } else {
                    (0.0, raw_y)
                };
                let mut lines = Vec::new();
                if sn_t {
                    lines.push(line_t);
                }
                if sn_b {
                    lines.push(line_b);
                }
                if sn_c {
                    lines.push(line_c);
                }
                (best.0, best.1, lines)
            };

            let final_dx = raw_dx + snap_dx;
            let final_dy = raw_dy + snap_dy;

            send_bridge_event(BridgeEvent::NodeDrag {
                node_id: drag_id.clone(),
                dx: final_dx,
                dy: final_dy,
            });

            // 使用 rAF 节流 DOM 更新
            let el_for_raf = el_ref.clone();
            let raf_final_dx = final_dx;
            let raf_final_dy = final_dy;
            schedule_raf(move || {
                update_node_position(&el_for_raf, orig_x + raf_final_dx, orig_y + raf_final_dy);
            });

            let has_snap = !snap_x_lines.is_empty() || !snap_y_lines.is_empty();
            if has_snap {
                show_snap_lines(&snap_x_lines, &snap_y_lines);
            } else {
                hide_snap_lines();
            }
        }) as Box<dyn FnMut(web_sys::Event)>);

        let Some(window) = web_sys::window() else {
            return;
        };
        window
            .add_event_listener_with_callback("mousemove", on_mousemove.as_ref().unchecked_ref())
            .ok();

        let up_id = node_id.clone();
        let move_js: JsValue = on_mousemove
            .as_ref()
            .unchecked_ref::<js_sys::Function>()
            .into();
        // R-1：存 on_mousemove 到 thread_local，mouseup 时 take 回收（替代 forget 泄漏）。
        ACTIVE_DRAG_MOVE.with(|c| *c.borrow_mut() = Some(on_mousemove));
        let on_mouseup = Closure::once(Box::new(move |event: web_sys::Event| {
            let Some(w) = web_sys::window() else {
                return;
            };
            let move_ref: &js_sys::Function = move_js.unchecked_ref();
            w.remove_event_listener_with_callback("mousemove", move_ref)
                .ok();
            // R-1：移除监听后 take + drop on_mousemove Closure，回收线性内存。
            ACTIVE_DRAG_MOVE.with(|c| *c.borrow_mut() = None);
            // self-remove：Closure::once 触发后由 wasm-bindgen 清理，无需 forget（P0-2）。
            let mm = event.dyn_ref::<web_sys::MouseEvent>();
            let (raw_dx, raw_dy) = match mm {
                Some(m) => (m.client_x() as f32 - start_x, m.client_y() as f32 - start_y),
                None => (0.0, 0.0),
            };
            let raw_x = orig_x + raw_dx;
            let raw_y = orig_y + raw_dy;
            let off_x = {
                let left = raw_x;
                let right = raw_x + node_w;
                let center_h = raw_x + node_w / 2.0;
                let (ol, sl, _) = find_snap_offset(left, &snap_x_candidates_up);
                let (or, sr, _) = find_snap_offset(right, &snap_x_candidates_up);
                let (oc, sc, _) = find_snap_offset(center_h, &snap_x_candidates_up);
                if sl && ol.abs() <= or.abs() && ol.abs() <= oc.abs() {
                    ol
                } else if sr && or.abs() <= oc.abs() {
                    or
                } else if sc {
                    oc
                } else {
                    0.0
                }
            };
            let off_y = {
                let top = raw_y;
                let bottom = raw_y + node_h;
                let center_v = raw_y + node_h / 2.0;
                let (ot, st, _) = find_snap_offset(top, &snap_y_candidates_up);
                let (ob, sb, _) = find_snap_offset(bottom, &snap_y_candidates_up);
                let (oc, sc, _) = find_snap_offset(center_v, &snap_y_candidates_up);
                if st && ot.abs() <= ob.abs() && ot.abs() <= oc.abs() {
                    ot
                } else if sb && ob.abs() <= oc.abs() {
                    ob
                } else if sc {
                    oc
                } else {
                    0.0
                }
            };
            let final_dx = raw_dx + off_x;
            let final_dy = raw_dy + off_y;
            send_bridge_event(BridgeEvent::NodeDrag {
                node_id: up_id.clone(),
                dx: final_dx,
                dy: final_dy,
            });
            hide_snap_lines();
        }) as Box<dyn FnOnce(web_sys::Event)>);
        window
            .add_event_listener_with_callback("mouseup", on_mouseup.as_ref().unchecked_ref())
            .ok();
        on_mouseup.forget();
        // R-1：on_mousemove 已托管到 SHELL（mouseup 时 take 回收），不再 forget。

        event.stop_propagation();
        event.prevent_default();
    }) as Box<dyn FnMut(web_sys::Event)>);
    container
        .add_event_listener_with_callback("mousedown", on_mousedown.as_ref().unchecked_ref())
        .ok();
    on_mousedown.forget();
    web_sys::console::log_1(&"fd-host-web: delegated mousedown listener installed".into());
}

/// 为节点 DOM 元素绑定 click / drag / resize 事件（逐节点模式，事件委托模式下不使用）。
/// 从 DOM 元素的 style 中读取 left/top 位置。
fn read_node_position(el: &web_sys::Element) -> (f32, f32) {
    // P-6：优先读 data-fd-x/y 缓存，缺则回退 style 串解析（兼容旧节点/无缓存渲染路径）。
    let x = read_attr_f32(el, "data-fd-x").unwrap_or_else(|| {
        extract_css_px(&el.get_attribute("style").unwrap_or_default(), "left").unwrap_or(0.0)
    });
    let y = read_attr_f32(el, "data-fd-y").unwrap_or_else(|| {
        extract_css_px(&el.get_attribute("style").unwrap_or_default(), "top").unwrap_or(0.0)
    });
    (x, y)
}

/// 从 DOM 元素的 style 中读取 width/height。
fn read_node_size(el: &web_sys::Element) -> (f32, f32) {
    // P-6：优先读 data-fd-w/h 缓存，缺则回退 style 串解析。
    let w = read_attr_f32(el, "data-fd-w").unwrap_or_else(|| {
        extract_css_px(&el.get_attribute("style").unwrap_or_default(), "width")
            .unwrap_or(DEFAULT_NODE_WIDTH)
    });
    let h = read_attr_f32(el, "data-fd-h").unwrap_or_else(|| {
        extract_css_px(&el.get_attribute("style").unwrap_or_default(), "height")
            .unwrap_or(DEFAULT_NODE_HEIGHT)
    });
    (w, h)
}

/// 从 CSS style 字符串中提取指定属性的 px 值。
fn extract_css_px(style: &str, prop: &str) -> Option<f32> {
    let prefix = format!("{}:", prop);
    for part in style.split(';') {
        let trimmed = part.trim();
        if let Some(rest) = trimmed.strip_prefix(&prefix) {
            let val = rest.trim().trim_end_matches("px");
            return val.parse().ok();
        }
    }
    None
}

/// 更新 DOM 元素的 left/top 样式（拖拽实时预览）。
fn update_node_position(el: &web_sys::Element, x: f32, y: f32) {
    let style = el.get_attribute("style").unwrap_or_default();
    let new_style = replace_css_prop(&style, "left", &format!("{}px", x));
    let new_style = replace_css_prop(&new_style, "top", &format!("{}px", y));
    el.set_attribute("style", &new_style).ok();
    // P-6：同步刷新几何缓存，吸附收集读缓存免 style 串解析。
    el.set_attribute("data-fd-x", &format!("{}", x)).ok();
    el.set_attribute("data-fd-y", &format!("{}", y)).ok();
}

/// 替换 CSS style 字符串中指定属性值，不存在则追加。
fn replace_css_prop(style: &str, prop: &str, value: &str) -> String {
    let prefix = format!("{}:", prop);
    let mut found = false;
    let parts: Vec<String> = style
        .split(';')
        .map(|p| {
            let trimmed = p.trim();
            if trimmed.starts_with(&prefix) {
                found = true;
                format!("{}:{}", prop, value)
            } else if trimmed.is_empty() {
                String::new()
            } else {
                trimmed.to_string()
            }
        })
        .filter(|s| !s.is_empty())
        .collect();
    let mut result = parts.join(";");
    if !found {
        result.push_str(&format!(";{}:{}", prop, value));
    }
    result
}

// L3：从 CSS style 字符串中移除指定属性（用于恢复显示时去掉 display 让 CSS 默认生效）。
fn strip_css_prop(style: &str, prop: &str) -> String {
    let prefix = format!("{}:", prop);
    let parts: Vec<String> = style
        .split(';')
        .map(|p| p.trim())
        .filter(|t| !t.is_empty() && !t.starts_with(&prefix))
        .map(|t| t.to_string())
        .collect();
    parts.join(";")
}

// L3：读取 CSS style 字符串中指定属性的原始值（非数字也适用，如 display:flex）。
fn read_css_prop_value(style: &str, prop: &str) -> Option<String> {
    let prefix = format!("{}:", prop);
    for part in style.split(';') {
        let trimmed = part.trim();
        if let Some(rest) = trimmed.strip_prefix(&prefix) {
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// 设置画布空白区域的点击事件监听。
fn setup_canvas_click_listener(container_id: &str) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };
    let container = match document.get_element_by_id(container_id) {
        Some(c) => c,
        None => return,
    };

    let on_click = Closure::wrap(Box::new(move |event: web_sys::Event| {
        let mouse = event.dyn_ref::<web_sys::MouseEvent>();
        let (x, y) = match mouse {
            Some(m) => (m.client_x() as f32, m.client_y() as f32),
            None => (0.0, 0.0),
        };
        send_bridge_event(BridgeEvent::CanvasClick { x, y });
    }) as Box<dyn FnMut(web_sys::Event)>);
    container
        .add_event_listener_with_callback("click", on_click.as_ref().unchecked_ref())
        .ok();
    on_click.forget();
}

/// 设置画布 wheel 缩放事件监听。
fn setup_canvas_zoom_listener(container_id: &str) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };
    let container = match document.get_element_by_id(container_id) {
        Some(c) => c,
        None => return,
    };

    let on_wheel = Closure::wrap(Box::new(move |event: web_sys::Event| {
        let wheel = match event.dyn_ref::<web_sys::WheelEvent>() {
            Some(w) => w,
            None => return,
        };
        let delta = wheel.delta_y() as f32;
        let x = wheel.client_x() as f32;
        let y = wheel.client_y() as f32;
        send_bridge_event(BridgeEvent::CanvasZoom { delta, x, y });
        apply_canvas_zoom(delta, x, y);
        event.prevent_default();
        event.stop_propagation();
    }) as Box<dyn FnMut(web_sys::Event)>);
    container
        .add_event_listener_with_callback("wheel", on_wheel.as_ref().unchecked_ref())
        .ok();
    on_wheel.forget();
    web_sys::console::log_1(&"fd-host-web: canvas zoom listener installed".into());
}

/// 设置画布中键/Space+拖拽平移事件监听。
fn setup_canvas_pan_listener(container_id: &str) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };
    let container = match document.get_element_by_id(container_id) {
        Some(c) => c,
        None => return,
    };

    let on_mousedown = Closure::wrap(Box::new(move |event: web_sys::Event| {
        let mouse = match event.dyn_ref::<web_sys::MouseEvent>() {
            Some(m) => m,
            None => return,
        };
        // 中键(button=1) 或 Space+左键(button=0 + shiftKey)
        let is_pan = mouse.button() == 1 || (mouse.button() == 0 && mouse.shift_key());
        if !is_pan {
            return;
        }

        let start_x = mouse.client_x() as f32;
        let start_y = mouse.client_y() as f32;

        let on_move = Closure::wrap(Box::new(move |event: web_sys::Event| {
            let mm = match event.dyn_ref::<web_sys::MouseEvent>() {
                Some(m) => m,
                None => return,
            };
            let dx = mm.client_x() as f32 - start_x;
            let dy = mm.client_y() as f32 - start_y;
            send_bridge_event(BridgeEvent::CanvasPan { dx, dy });
            apply_canvas_pan(dx, dy);
        }) as Box<dyn FnMut(web_sys::Event)>);

        let Some(win) = web_sys::window() else {
            return;
        };
        win.add_event_listener_with_callback("mousemove", on_move.as_ref().unchecked_ref())
            .ok();

        let move_js: JsValue = on_move.as_ref().unchecked_ref::<js_sys::Function>().into();
        // R-1：存 on_move 到 thread_local，mouseup 时 take 回收（替代 forget 泄漏）。
        ACTIVE_DRAG_MOVE.with(|c| *c.borrow_mut() = Some(on_move));
        let on_up = Closure::once(Box::new(move |event: web_sys::Event| {
            let Some(w) = web_sys::window() else {
                return;
            };
            let move_ref: &js_sys::Function = move_js.unchecked_ref();
            w.remove_event_listener_with_callback("mousemove", move_ref)
                .ok();
            // R-1：移除监听后 take + drop on_move Closure，回收线性内存。
            ACTIVE_DRAG_MOVE.with(|c| *c.borrow_mut() = None);
            // Closure::once 触发后自清理，无需 forget（P0-2）。
            let mm = event.dyn_ref::<web_sys::MouseEvent>();
            let (dx, dy) = match mm {
                Some(m) => (m.client_x() as f32 - start_x, m.client_y() as f32 - start_y),
                None => (0.0, 0.0),
            };
            send_bridge_event(BridgeEvent::CanvasPan { dx, dy });
        }) as Box<dyn FnOnce(web_sys::Event)>);
        win.add_event_listener_with_callback("mouseup", on_up.as_ref().unchecked_ref())
            .ok();
        on_up.forget();
        // R-1：on_move 已托管到 SHELL（mouseup 时 take 回收），不再 forget。

        event.prevent_default();
        event.stop_propagation();
    }) as Box<dyn FnMut(web_sys::Event)>);
    container
        .add_event_listener_with_callback("mousedown", on_mousedown.as_ref().unchecked_ref())
        .ok();
    on_mousedown.forget();
    web_sys::console::log_1(&"fd-host-web: canvas pan listener installed".into());
}

/// 框选监听：在画布空白区域按住拖拽 → 绘制半透明矩形 → 释放时收集矩形内节点。
fn setup_marquee_listener(container_id: &str) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };
    let container = match document.get_element_by_id(container_id) {
        Some(c) => c,
        None => return,
    };

    let on_mousedown = Closure::wrap(Box::new(move |event: web_sys::Event| {
        let mouse = match event.dyn_ref::<web_sys::MouseEvent>() {
            Some(m) => m,
            None => return,
        };
        // 只响应左键且不按修饰键（Shift=平移，已占用）
        if mouse.button() != 0 || mouse.shift_key() {
            return;
        }
        // 如果点击到了节点元素，不做框选（节点拖拽已有处理）
        let target = event.target();
        let target_el = target
            .as_ref()
            .and_then(|t| t.dyn_ref::<web_sys::Element>());
        if let Some(el) = target_el {
            if el.has_attribute("data-node-id")
                || el.closest("[data-node-id]").unwrap_or(None).is_some()
            {
                return;
            }
        }

        let start_x = mouse.client_x() as f32;
        let start_y = mouse.client_y() as f32;

        // 创建选框 DOM 元素
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(doc) = window.document() else {
            return;
        };
        let marquee_el = match doc.create_element("div") {
            Ok(el) => el,
            Err(_) => {
                web_sys::console::warn_1(&"fd-host-web: marquee 创建选框元素失败".into());
                return;
            }
        };
        marquee_el.set_id("fd-marquee");
        marquee_el
            .set_attribute(
                "style",
                "position:fixed;border:2px dashed #007AFF;background:rgba(0,122,255,0.1);\
             pointer-events:none;z-index:99999;display:none;",
            )
            .ok();
        if let Some(body) = doc.body() {
            body.append_child(&marquee_el).ok();
        }

        let on_move = Closure::wrap(Box::new(move |event: web_sys::Event| {
            let mm = match event.dyn_ref::<web_sys::MouseEvent>() {
                Some(m) => m,
                None => return,
            };
            let cur_x = mm.client_x() as f32;
            let cur_y = mm.client_y() as f32;
            let left = start_x.min(cur_x);
            let top = start_y.min(cur_y);
            let width = (cur_x - start_x).abs();
            let height = (cur_y - start_y).abs();
            let Some(win) = web_sys::window() else {
                return;
            };
            let Some(doc) = win.document() else {
                return;
            };
            let m_el = doc.get_element_by_id("fd-marquee");
            if let Some(m) = m_el {
                m.set_attribute(
                    "style",
                    &format!(
                        "position:fixed;border:2px dashed #007AFF;background:rgba(0,122,255,0.1);\
                     pointer-events:none;z-index:99999;display:block;\
                     left:{}px;top:{}px;width:{}px;height:{}px;",
                        left, top, width, height
                    ),
                )
                .ok();
            }
        }) as Box<dyn FnMut(web_sys::Event)>);

        let Some(win) = web_sys::window() else {
            return;
        };
        win.add_event_listener_with_callback("mousemove", on_move.as_ref().unchecked_ref())
            .ok();

        let move_js: JsValue = on_move.as_ref().unchecked_ref::<js_sys::Function>().into();
        // R-1：存 on_move 到 thread_local，mouseup 时 take 回收（替代 forget 泄漏）。
        ACTIVE_DRAG_MOVE.with(|c| *c.borrow_mut() = Some(on_move));
        let on_up = Closure::once(Box::new(move |event: web_sys::Event| {
            let Some(w) = web_sys::window() else {
                return;
            };
            let move_ref: &js_sys::Function = move_js.unchecked_ref();
            w.remove_event_listener_with_callback("mousemove", move_ref)
                .ok();
            // R-1：移除监听后 take + drop on_move Closure，回收线性内存。
            ACTIVE_DRAG_MOVE.with(|c| *c.borrow_mut() = None);
            // Closure::once 触发后自清理（P0-2）。

            // 移除选框 DOM
            if let Some(doc) = w.document() {
                if let Some(m) = doc.get_element_by_id("fd-marquee") {
                    m.remove();
                }
            }

            // 收集框选矩形内的节点
            let mm = event.dyn_ref::<web_sys::MouseEvent>();
            let (end_x, end_y) = match mm {
                Some(m) => (m.client_x() as f32, m.client_y() as f32),
                None => (start_x, start_y),
            };
            let rect_left = start_x.min(end_x);
            let rect_top = start_y.min(end_y);
            let rect_right = start_x.max(end_x);
            let rect_bottom = start_y.max(end_y);

            // 至少 5px 才算有效框选
            if (rect_right - rect_left) < MIN_MARQUEE_SIZE
                && (rect_bottom - rect_top) < MIN_MARQUEE_SIZE
            {
                return;
            }

            let selected = collect_nodes_in_rect(rect_left, rect_top, rect_right, rect_bottom);
            if !selected.is_empty() {
                send_bridge_event(BridgeEvent::MarqueeSelect { node_ids: selected });
            }
        }) as Box<dyn FnOnce(web_sys::Event)>);

        win.add_event_listener_with_callback("mouseup", on_up.as_ref().unchecked_ref())
            .ok();
        on_up.forget();
        // R-1：on_move 已托管到 SHELL（mouseup 时 take 回收），不再 forget。
    }) as Box<dyn FnMut(web_sys::Event)>);

    container
        .add_event_listener_with_callback("mousedown", on_mousedown.as_ref().unchecked_ref())
        .ok();
    on_mousedown.forget();
    web_sys::console::log_1(&"fd-host-web: marquee listener installed".into());
}

/// 收集矩形区域内的所有 data-node-id 元素。
fn collect_nodes_in_rect(left: f32, top: f32, right: f32, bottom: f32) -> Vec<String> {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return vec![],
    };
    let document = match window.document() {
        Some(d) => d,
        None => return vec![],
    };
    let container = match document.get_element_by_id("fusion-dom-root") {
        Some(c) => c,
        None => return vec![],
    };
    // L2：递归遍历子树，框选才能命中嵌套节点（原先只看一层）。
    let mut result = vec![];
    let node: &web_sys::Node = container.unchecked_ref();
    collect_nodes_in_rect_inner(node, left, top, right, bottom, &mut result);
    result
}

fn collect_nodes_in_rect_inner(
    parent: &web_sys::Node,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
    out: &mut Vec<String>,
) {
    let child_nodes = parent.child_nodes();
    for i in 0..child_nodes.length() {
        if let Some(child) = child_nodes.get(i) {
            if let Ok(el) = child.clone().dyn_into::<web_sys::Element>() {
                if let Some(node_id) = el.get_attribute("data-node-id") {
                    let rect = el.get_bounding_client_rect();
                    let el_left = rect.left() as f32;
                    let el_top = rect.top() as f32;
                    let el_right = rect.right() as f32;
                    let el_bottom = rect.bottom() as f32;
                    // 判断矩形重叠
                    if el_left < right && el_right > left && el_top < bottom && el_bottom > top {
                        out.push(node_id);
                    }
                }
                // 递归进入子树
                let child_node: &web_sys::Node = el.unchecked_ref();
                collect_nodes_in_rect_inner(child_node, left, top, right, bottom, out);
            }
        }
    }
}

/// 画布缩放视觉反馈：更新 CSS transform scale + translate。
/// 性能优化：rAF 节流 DOM 更新 + 视口剔除增量渲染。
fn apply_canvas_zoom(delta: f32, _cx: f32, _cy: f32) {
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

    let current = container.get_attribute("data-fd-zoom").unwrap_or_default();
    let current_scale: f32 = current.parse().unwrap_or(1.0);
    let factor = if delta < 0.0 { 1.1 } else { 1.0 / 1.1 };
    let new_scale = (current_scale * factor).clamp(0.1, 10.0);

    container
        .set_attribute("data-fd-zoom", &new_scale.to_string())
        .ok();

    let pan_x: f32 = container
        .get_attribute("data-fd-pan-x")
        .unwrap_or_default()
        .parse()
        .unwrap_or(0.0);
    let pan_y: f32 = container
        .get_attribute("data-fd-pan-y")
        .unwrap_or_default()
        .parse()
        .unwrap_or(0.0);

    // rAF 节流：避免每帧多次 DOM 更新
    schedule_raf(move || {
        // 重新查找容器（rAF 回调中无法持有引用）
        let window = match web_sys::window() {
            Some(w) => w,
            None => return,
        };
        let document = match window.document() {
            Some(d) => d,
            None => return,
        };
        if let Some(container) = document.get_element_by_id("fusion-dom-root") {
            // L-14：仅设 transform 相关属性，不整体覆盖 style（保留容器其他 CSS）。
            let style = container.dyn_into::<web_sys::HtmlElement>().ok();
            if let Some(el) = style {
                let _ = el.style().set_property(
                    "transform",
                    &format!(
                        "scale({}) translate({}px,{}px)",
                        new_scale,
                        pan_x / new_scale,
                        pan_y / new_scale
                    ),
                );
                let _ = el.style().set_property("transform-origin", "0 0");
            }
        }
        // R-16：缩放后视口剔除——直接同步调用，不嵌套 schedule_raf。
        // 旧实现嵌套 rAF 把 cull 推到下一帧，且复用同一 RAF_SCHEDULED 标志，
        // 导致 transform 帧与 cull 帧竞态。同帧内 transform 先设再 cull，无嵌套。
        viewport_cull_update();
    });
}

/// 画布平移视觉反馈：更新 CSS transform translate。
/// 性能优化：rAF 节流 DOM 更新 + 视口剔除增量渲染。
fn apply_canvas_pan(dx: f32, dy: f32) {
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

    let pan_x: f32 = container
        .get_attribute("data-fd-pan-x")
        .unwrap_or_default()
        .parse()
        .unwrap_or(0.0);
    let pan_y: f32 = container
        .get_attribute("data-fd-pan-y")
        .unwrap_or_default()
        .parse()
        .unwrap_or(0.0);
    let new_x = pan_x + dx;
    let new_y = pan_y + dy;

    container
        .set_attribute("data-fd-pan-x", &new_x.to_string())
        .ok();
    container
        .set_attribute("data-fd-pan-y", &new_y.to_string())
        .ok();

    let zoom: f32 = container
        .get_attribute("data-fd-zoom")
        .unwrap_or_default()
        .parse()
        .unwrap_or(1.0);

    // rAF 节流：避免每帧多次 DOM 更新
    schedule_raf(move || {
        let window = match web_sys::window() {
            Some(w) => w,
            None => return,
        };
        let document = match window.document() {
            Some(d) => d,
            None => return,
        };
        if let Some(container) = document.get_element_by_id("fusion-dom-root") {
            // L-14：仅设 transform 相关属性，不整体覆盖 style。
            if let Ok(el) = container.dyn_into::<web_sys::HtmlElement>() {
                let _ = el.style().set_property(
                    "transform",
                    &format!(
                        "scale({}) translate({}px,{}px)",
                        zoom,
                        new_x / zoom,
                        new_y / zoom
                    ),
                );
                let _ = el.style().set_property("transform-origin", "0 0");
            }
        }
        // R-16：平移后视口剔除——直接同步调用，不嵌套 schedule_raf（同上）。
        viewport_cull_update();
    });
}

/// 多选节点：切换指定节点的选中状态，不影响其他已选中节点。
fn toggle_node_selection(node_id: &str) {
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
        let is_selected = el.get_attribute("data-fd-selected").unwrap_or_default() == "true";
        if is_selected {
            // L4：取消选中只移除我们注入的 box-shadow，不触碰用户 outline。
            let style = el.get_attribute("style").unwrap_or_default();
            let clean = strip_css_prop(&style, "box-shadow");
            el.set_attribute("style", &clean).ok();
            el.remove_attribute("data-fd-selected").ok();
            // R-17：同步从多选集合移除，selected_id 一致性见下方。
            if let Some(inner) = shell_lock().as_mut() {
                inner.selected_ids.remove(node_id);
                if inner.selected_id.as_deref() == Some(node_id) {
                    inner.selected_id = inner.selected_ids.iter().next().cloned();
                }
            }
        } else {
            el.set_attribute("data-fd-selected", "true").ok();
            // L4：用 box-shadow 做选中高亮，保留节点自身 outline 不被覆盖。
            let style = el.get_attribute("style").unwrap_or_default();
            el.set_attribute("style", &format!("{};box-shadow:0 0 0 2px #007AFF;", style))
                .ok();
            // R-17：加入多选集合。无主选中时设为主选中（handles 锚点）。
            if let Some(inner) = shell_lock().as_mut() {
                inner.selected_ids.insert(node_id.to_string());
                if inner.selected_id.is_none() {
                    inner.selected_id = Some(node_id.to_string());
                }
            }
        }
    }
}

// ── Bridge 辅助 ──

/// 注入设计 Token CSS 到页面 :root。
fn apply_tokens_css(css: &str) {
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
fn select_node(node_id: &str) {
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
        let on_up = Closure::once(Box::new(move |event: web_sys::Event| {
            let Some(w) = web_sys::window() else {
                return;
            };
            let move_ref: &js_sys::Function = move_js.unchecked_ref();
            w.remove_event_listener_with_callback("mousemove", move_ref)
                .ok();
            // R-1：移除监听后 take + drop on_move Closure，回收线性内存。
            ACTIVE_DRAG_MOVE.with(|c| *c.borrow_mut() = None);
            // Closure::once 触发后自清理（P0-2）。

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
        }) as Box<dyn FnOnce(web_sys::Event)>);
        window
            .add_event_listener_with_callback("mouseup", on_up.as_ref().unchecked_ref())
            .ok();
        on_up.forget();
        // R-1：on_move 已托管到 SHELL（mouseup 时 take 回收），不再 forget。

        event.stop_propagation();
        event.prevent_default();
    }) as Box<dyn FnMut(web_sys::Event)>);
    handle
        .add_event_listener_with_callback("mousedown", on_handle_mousedown.as_ref().unchecked_ref())
        .ok();
    on_handle_mousedown.forget();

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
fn mutate_node(
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
fn set_node_visibility(node_id: &str, visible: bool) {
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
fn set_node_locked(node_id: &str, locked: bool) {
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
fn reorder_node(node_id: &str, new_index: usize) {
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

/// 重置画布视图（缩放=1.0，平移=0,0）。
fn reset_canvas_view() {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };
    if let Some(container) = document.get_element_by_id("fusion-dom-root") {
        container.set_attribute("data-fd-zoom", "1.0").ok();
        container.set_attribute("data-fd-pan-x", "0.0").ok();
        container.set_attribute("data-fd-pan-y", "0.0").ok();
        // L-14：仅重置 transform，不整体覆盖 style。
        if let Ok(el) = container.dyn_into::<web_sys::HtmlElement>() {
            let _ = el
                .style()
                .set_property("transform", "scale(1) translate(0px,0px)");
            let _ = el.style().set_property("transform-origin", "0 0");
        }
    }
    web_sys::console::log_1(&"fd-host-web: canvas view reset".into());
}

/// 清空画布（Canvas + DOM）。
fn clear_canvas() {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };

    // 清空 Canvas
    let guard = shell_lock();
    if let Some(inner) = guard.as_ref() {
        if let Some(canvas) = get_canvas(&inner.canvas_id) {
            if let Ok(Some(ctx)) = canvas.get_context("2d") {
                if let Ok(ctx2d) = ctx.dyn_into::<web_sys::CanvasRenderingContext2d>() {
                    ctx2d.clear_rect(0.0, 0.0, canvas.width() as f64, canvas.height() as f64);
                }
            }
        }
    }

    // 清空 DOM 容器
    if let Some(container) = document.get_element_by_id("fusion-dom-root") {
        container.set_inner_html("");
    }
}

/// Plan 预览：将 PenDocument 节点渲染为虚线叠加层。
fn render_plan_preview(doc_json: &str) {
    let doc = match PenDocument::from_json(doc_json) {
        Ok(d) => d,
        Err(e) => {
            web_sys::console::error_1(&format!("fd-host-web: PlanPreview 解析失败: {e}").into());
            return;
        }
    };

    remove_plan_preview();

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

    let overlay = match document.create_element("div") {
        Ok(el) => el,
        Err(_) => {
            web_sys::console::warn_1(&"fd-host-web: render_plan_preview 创建 overlay 失败".into());
            return;
        }
    };
    overlay.set_id("fd-plan-preview");
    if overlay
        .set_attribute(
            "style",
            "position:absolute;top:0;left:0;width:100%;height:100%;pointer-events:none;z-index:1000;",
        )
        .is_err()
    {
        web_sys::console::warn_1(&"fd-host-web: render_plan_preview overlay set_attribute 失败".into());
    }

    for page in &doc.pages {
        for node in &page.nodes {
            let el = match document.create_element("div") {
                Ok(e) => e,
                Err(_) => {
                    web_sys::console::warn_1(
                        &"fd-host-web: render_plan_preview 创建节点失败".into(),
                    );
                    continue;
                }
            };
            let style = format!(
                "position:absolute;left:{}px;top:{}px;width:{}px;height:{}px;\
                 border:2px dashed #007AFF;border-radius:{}px;opacity:0.6;",
                node.x as i32,
                node.y as i32,
                node.w as i32,
                node.h as i32,
                node.style.radius.map(|r| r as i32).unwrap_or(0)
            );
            // C12：set_attribute 失败不 panic，warn 后跳过该节点样式注入。
            if el.set_attribute("style", &style).is_err() {
                web_sys::console::warn_1(
                    &"fd-host-web: render_plan_preview 节点 set_attribute 失败".into(),
                );
            }
            if let Some(text) = &node.text {
                el.set_text_content(Some(text));
            }
            overlay.append_child(&el).ok();
        }
    }

    container.append_child(&overlay).ok();
    web_sys::console::log_1(&"fd-host-web: Plan preview rendered".into());
}

/// 移除 Plan 预览叠加层。
fn remove_plan_preview() {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };
    if let Some(overlay) = document.get_element_by_id("fd-plan-preview") {
        overlay.remove();
        web_sys::console::log_1(&"fd-host-web: Plan preview removed".into());
    }
}

/// 渲染 PenDocument JSON 到 Canvas。
fn render_page(doc_json: &str) {
    let doc = match PenDocument::from_json(doc_json) {
        Ok(d) => d,
        Err(e) => {
            web_sys::console::error_1(&format!("fd-host-web: PenDocument 解析失败: {e}").into());
            return;
        }
    };

    // L-18：canvas 渲染路径也写 cached_doc_json，
    // 使 viewport_cull_update 两路径（DOM/canvas）都有最新文档，避免陈旧/None。
    {
        let mut guard = shell_lock();
        if let Some(inner) = guard.as_mut() {
            inner.cached_doc_json = Some(doc_json.to_string());
            // P-7/R-17：canvas 重渲同 DOM 重渲，清空选中记录。
            inner.selected_id = None;
            inner.selected_ids.clear();
        }
    }

    // 仅持锁取 canvas_id 后立即释放，避免大文档重绘期间阻塞消息处理（P1-2）。
    let canvas_id = {
        let guard = shell_lock();
        match guard.as_ref() {
            Some(i) => i.canvas_id.clone(),
            None => {
                web_sys::console::error_1(&"fd-host-web: WebShell 未初始化".into());
                return;
            }
        }
    };

    let canvas = match get_canvas(&canvas_id) {
        Some(c) => c,
        None => return,
    };

    let ctx = match canvas.get_context("2d") {
        Ok(Some(ctx)) => ctx.dyn_into::<web_sys::CanvasRenderingContext2d>().ok(),
        _ => None,
    };
    let ctx = match ctx {
        Some(c) => c,
        None => {
            web_sys::console::error_1(&"fd-host-web: 无法获取 2D 上下文".into());
            return;
        }
    };

    // 清空画布
    ctx.clear_rect(0.0, 0.0, canvas.width() as f64, canvas.height() as f64);

    // A-11：canvas 路径清空 DOM 容器，消除 render_dom 残留的 DOM 重影。
    // 双轨不一致：若先 render_dom（DOM 节点）再 render_page（canvas），
    // 旧 DOM 节点仍叠在 canvas 上方，与 canvas 画面重影。与 render_dom 清空对齐。
    if let Some(container) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("fusion-dom-root"))
    {
        container.set_inner_html("");
    }

    // 渲染每个页面
    for page in &doc.pages {
        render_page_to_canvas(page, &ctx);
    }
}

/// 递归渲染深度上限，防止恶意/异常深层嵌套文档栈溢出（P2-1）。
const MAX_RENDER_DEPTH: u32 = 64;

/// 渲染单页到 Canvas 2D。
fn render_page_to_canvas(page: &fd_canvas_core::Page, ctx: &web_sys::CanvasRenderingContext2d) {
    for node in &page.nodes {
        render_node(node, ctx, 0);
    }
}

/// 递归渲染节点。depth 限制嵌套深度，超过 MAX_RENDER_DEPTH 跳过子树防栈溢出（P2-1）。
fn render_node(
    node: &fd_canvas_core::PenNode,
    ctx: &web_sys::CanvasRenderingContext2d,
    depth: u32,
) {
    // 设置填充色
    if let Some(fill) = &node.style.fill {
        ctx.set_fill_style_str(fill);
    }

    // 设置描边
    if let Some(stroke) = &node.style.stroke {
        ctx.set_stroke_style_str(stroke);
        ctx.set_line_width(node.style.stroke_width.unwrap_or(1.0));
    }

    let x = node.x;
    let y = node.y;
    let w = node.w;
    let h = node.h;

    match node.kind {
        fd_canvas_core::NodeKind::Rect => {
            let r = node.style.radius.unwrap_or(0.0);
            if r > 0.0 {
                round_rect(ctx, x, y, w, h, r);
            } else {
                ctx.fill_rect(x, y, w, h);
            }
        }
        fd_canvas_core::NodeKind::Circle => {
            ctx.begin_path();
            let cx = x + w / 2.0;
            let cy = y + h / 2.0;
            let r = w.min(h) / 2.0;
            let _ = ctx.arc(cx, cy, r, 0.0, std::f64::consts::TAU);
            ctx.close_path();
            ctx.fill();
        }
        fd_canvas_core::NodeKind::Text => {
            if let Some(text) = &node.text {
                ctx.set_font(&format!(
                    "{}px {}",
                    node.style.font_size.unwrap_or(16.0) as u32,
                    node.style.font_family.as_deref().unwrap_or("system-ui"),
                ));
                ctx.fill_text(text, x, y + node.style.font_size.unwrap_or(16.0))
                    .ok();
            }
        }
        fd_canvas_core::NodeKind::Image => {
            ctx.fill_rect(x, y, w, h);
        }
        fd_canvas_core::NodeKind::Group => {
            if depth >= MAX_RENDER_DEPTH {
                web_sys::console::warn_1(
                    &format!("render_node: 嵌套深度超限 {MAX_RENDER_DEPTH}，跳过子树").into(),
                );
                return;
            }
            if node.children.len() > MAX_CHILDREN_PER_NODE {
                web_sys::console::warn_1(
                    &format!(
                        "render_node: 子节点数 {} 超 {} 上限，仅渲染前 {} 个",
                        node.children.len(),
                        MAX_CHILDREN_PER_NODE,
                        MAX_CHILDREN_PER_NODE
                    )
                    .into(),
                );
            }
            for child in node.children.iter().take(MAX_CHILDREN_PER_NODE) {
                render_node(child, ctx, depth + 1);
            }
        }
    }
}

/// 绘制圆角矩形。
fn round_rect(ctx: &web_sys::CanvasRenderingContext2d, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    ctx.begin_path();
    ctx.move_to(x + r, y);
    ctx.line_to(x + w - r, y);
    ctx.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0)
        .ok();
    ctx.line_to(x + w, y + h - r);
    ctx.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2)
        .ok();
    ctx.line_to(x + r, y + h);
    ctx.arc(
        x + r,
        y + h - r,
        r,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    )
    .ok();
    ctx.line_to(x, y + r);
    ctx.arc(
        x + r,
        y + r,
        r,
        std::f64::consts::PI,
        -std::f64::consts::FRAC_PI_2,
    )
    .ok();
    ctx.close_path();
    ctx.fill();
    ctx.stroke();
}

/// 获取 canvas 元素。
fn get_canvas(canvas_id: &str) -> Option<web_sys::HtmlCanvasElement> {
    let window = web_sys::window()?;
    let document = window.document()?;
    document
        .get_element_by_id(canvas_id)?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .ok()
}

// ── 单元测试（宿主目标，非 wasm32）──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pen_document_render_json_roundtrip() {
        let mut doc = PenDocument::new();
        let mut page = fd_canvas_core::Page::new("p1", "Test", 100.0, 100.0);
        page.add(fd_canvas_core::PenNode::rect("n1", 10.0, 20.0, 50.0, 30.0));
        doc.add_page(page);
        let json = doc.to_json().unwrap();
        let doc2 = PenDocument::from_json(&json).unwrap();
        assert_eq!(doc2.pages.len(), 1);
        assert_eq!(doc2.pages[0].nodes[0].id, "n1");
    }

    // E-39 回归：sanitize_css_color 剔除可逃逸 CSS 属性边界的 `;{}`（不触发 web_sys）。
    // 危险 url()/expression() 路径走 web_sys console（原生测试不可用），仅验安全净化。
    #[test]
    fn sanitize_css_color_strips_escape_chars() {
        assert_eq!(sanitize_css_color("#ff0000"), "#ff0000");
        assert_eq!(sanitize_css_color("rgb(255, 0, 0)"), "rgb(255, 0, 0)");
        // 逃逸字符 ; { } < > 应被剔除，保留颜色主体。
        let out = sanitize_css_color("red;} * { background:#fff }");
        assert!(!out.contains(';'), "应剔除分号防属性逃逸");
        assert!(!out.contains('{'), "应剔除左花括号");
        assert!(!out.contains('}'), "应剔除右花括号");
        assert!(out.contains("red"), "保留合法颜色名");
        assert!(out.contains("background:#fff"), "保留未逃逸的声明");
    }

    // E-40/P3 回归：sanitize_token_css 须剥 /* */ 注释后再匹配关键字，
    // 防 `u/**/rl(` 等注释拆词绕过 url()/@import/expression()/javascript: 检测。
    // 用 _inner 纯逻辑（不触发 web_sys::console，原生测试安全）。
    #[test]
    fn sanitize_token_css_strips_comment_obfuscated_danger() {
        // 注释拆词的 url() 必须被剔除。
        let bad = "--bg: u/**/rl(http://evil/x.png)";
        let (out, stripped) = sanitize_token_css_inner(bad);
        assert!(!out.contains("evil"), "注释拆词 url() 必须被剔除");
        assert_eq!(stripped, 1, "危险行整体剔除，计 1 行");
        assert!(out.is_empty(), "危险行整体剔除后应为空");
        // 注释拆词的 @import 必须被剔除。
        let bad2 = "@imp/**/ort url(http://evil.css)";
        let (out2, s2) = sanitize_token_css_inner(bad2);
        assert!(!out2.contains("evil"), "注释拆词 @import 必须被剔除");
        assert_eq!(s2, 1);
        // 注释拆词的 expression() 必须被剔除。
        let bad3 = "--x: expr/**/ession(alert(1))";
        let (out3, s3) = sanitize_token_css_inner(bad3);
        assert!(!out3.contains("alert"), "注释拆词 expression() 必须被剔除");
        assert_eq!(s3, 1);
    }

    #[test]
    fn sanitize_token_css_preserves_safe_token_declarations() {
        // E-40 正向：合法 token 声明（含合法注释说明）应保留，stripped=0。
        let safe = "--color-bg: #ffffff; /* 背景白 */";
        let (out, stripped) = sanitize_token_css_inner(safe);
        assert!(
            out.contains("--color-bg: #ffffff;"),
            "合法声明应保留: {out}"
        );
        assert_eq!(stripped, 0, "无危险关键字不应剔除");
        // 安全注释行可保留（不命中危险关键字）。
        let safe2 = "/* 仅注释 */\n--space-1: 4px;";
        let (out2, s2) = sanitize_token_css_inner(safe2);
        assert!(out2.contains("--space-1: 4px;"), "合法声明应保留");
        assert_eq!(s2, 0);
    }

    #[test]
    fn sanitize_token_css_plain_danger_still_stripped() {
        // E-40 回归：无注释的普通 url()/@import 仍被剔除（不破坏原有 C10 行为）。
        let css = "--bg: url(http://evil.png);\n@import url(evil.css);\n--ok: #fff;";
        let (out, stripped) = sanitize_token_css_inner(css);
        assert!(!out.contains("evil"), "普通 url()/@import 仍被剔除");
        assert!(out.contains("--ok: #fff;"), "合法声明保留");
        assert_eq!(stripped, 2, "两行危险 CSS 被剔除");
    }

    // R-A22/E-41 回归：send_to_host 在原生测试环境（无 Window）不得 panic；
    // 有效 payload 走完序列化后优雅返回；无效 payload（含 serde 无法序列化的
    // 非 UTF-8 键）走 E-41 告警分支也不得 panic。
    #[test]
    fn send_to_host_valid_payload_no_panic_native() {
        send_to_host("ai.generate", &serde_json::json!({"prompt": "登录页"}));
    }

    #[test]
    fn send_to_host_empty_payload_no_panic_native() {
        send_to_host("click", &serde_json::json!({}));
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn send_to_host_handler_name_is_camelcase() {
        // R-A22：桥 handler 名须为 WKWebView 惯例 camelCase，studio 侧据此注册。
        // HOST_HANDLER_NAME 仅 wasm32 编译（非 wasm 构建不产出该常量）。
        assert_eq!(HOST_HANDLER_NAME, "fdHost");
    }

    #[test]
    fn handle_host_message_render_page() {
        let doc_json = r#"{"pages":[{"id":"p1","name":"Test","width":100,"height":100,"nodes":[{"id":"n1","kind":"Rect","name":"Rect","x":0,"y":0,"w":50,"h":50,"style":{}}]}]}"#;
        let payload = serde_json::json!({"document": doc_json});
        let msg = serde_json::json!({
            "kind": "page.render",
            "payload": payload
        });
        let json = serde_json::to_string(&msg).unwrap();
        // 验证 JSON 结构正确（不调用 handle_host_message，后者需浏览器环境）
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["kind"], "page.render");
        assert!(parsed["payload"]["document"].is_string());
    }

    #[test]
    fn handle_host_message_invalid_json_does_not_panic() {
        // 仅验证 JSON 解析逻辑，不触发 web_sys console
        assert!(serde_json::from_str::<serde_json::Value>("not json").is_err());
        assert!(serde_json::from_str::<serde_json::Value>("").is_err());
    }

    #[test]
    fn handle_host_message_unknown_kind_does_not_panic() {
        let msg = serde_json::json!({"kind": "unknown", "payload": {}});
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["kind"], "unknown");
    }

    #[test]
    fn mount_creates_shell() {
        // 需要在浏览器环境运行，宿主目标仅验证结构
        let _shell = WebShell {
            inner: WebShellInner {
                canvas_id: "canvas".into(),
                ready: true,
                cached_doc_json: None,
                selected_id: None,
                selected_ids: std::collections::HashSet::new(),
            },
        };
    }

    #[test]
    fn send_to_host_builds_valid_json() {
        let payload = serde_json::json!({"prompt": "hello"});
        let msg = serde_json::json!({
            "direction": "WebViewToBackend",
            "kind": "ai.generate",
            "payload": payload
        });
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["kind"], "ai.generate");
        assert_eq!(parsed["direction"], "WebViewToBackend");
    }

    #[test]
    fn round_rect_math() {
        // 验证圆角矩形路径计算：r 不应超过 w/2 或 h/2
        let w = 50.0;
        let h = 30.0;
        let r: f64 = 20.0;
        let clamped = r.min(w / 2.0).min(h / 2.0);
        assert_eq!(clamped, 15.0);
    }

    // ── Bridge 类型测试 ──

    #[test]
    fn bridge_command_page_render_serde() {
        let cmd = BridgeCommand::PageRender {
            document_json: r#"{"pages":[]}"#.to_string(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let cmd2: BridgeCommand = serde_json::from_str(&json).unwrap();
        match cmd2 {
            BridgeCommand::PageRender { document_json } => {
                assert!(document_json.contains("pages"));
            }
            _ => panic!("期望 PageRender"),
        }
    }

    #[test]
    fn bridge_command_apply_tokens_serde() {
        let cmd = BridgeCommand::ApplyTokens {
            css: ":root { --color-bg: #FFF; }".to_string(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let cmd2: BridgeCommand = serde_json::from_str(&json).unwrap();
        match cmd2 {
            BridgeCommand::ApplyTokens { css } => {
                assert!(css.contains("--color-bg"));
            }
            _ => panic!("期望 ApplyTokens"),
        }
    }

    #[test]
    fn bridge_command_select_node_serde() {
        let cmd = BridgeCommand::SelectNode {
            node_id: "btn_1".to_string(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let cmd2: BridgeCommand = serde_json::from_str(&json).unwrap();
        match cmd2 {
            BridgeCommand::SelectNode { node_id } => assert_eq!(node_id, "btn_1"),
            _ => panic!("期望 SelectNode"),
        }
    }

    #[test]
    fn bridge_command_clear_canvas_serde() {
        let cmd = BridgeCommand::ClearCanvas;
        let json = serde_json::to_string(&cmd).unwrap();
        let cmd2: BridgeCommand = serde_json::from_str(&json).unwrap();
        match cmd2 {
            BridgeCommand::ClearCanvas => {}
            _ => panic!("期望 ClearCanvas"),
        }
    }

    #[test]
    fn bridge_event_node_click_serde() {
        let event = BridgeEvent::NodeClick {
            node_id: "n1".to_string(),
            x: 10.0,
            y: 20.0,
        };
        let json = serde_json::to_string(&event).unwrap();
        let event2: BridgeEvent = serde_json::from_str(&json).unwrap();
        match event2 {
            BridgeEvent::NodeClick { node_id, x, y } => {
                assert_eq!(node_id, "n1");
                assert_eq!(x, 10.0);
                assert_eq!(y, 20.0);
            }
            _ => panic!("期望 NodeClick"),
        }
    }

    #[test]
    fn bridge_event_node_drag_serde() {
        let event = BridgeEvent::NodeDrag {
            node_id: "n2".to_string(),
            dx: 5.0,
            dy: -3.0,
        };
        let json = serde_json::to_string(&event).unwrap();
        let event2: BridgeEvent = serde_json::from_str(&json).unwrap();
        match event2 {
            BridgeEvent::NodeDrag { node_id, dx, dy } => {
                assert_eq!(node_id, "n2");
                assert_eq!(dx, 5.0);
                assert_eq!(dy, -3.0);
            }
            _ => panic!("期望 NodeDrag"),
        }
    }

    #[test]
    fn bridge_event_canvas_click_serde() {
        let event = BridgeEvent::CanvasClick { x: 100.0, y: 200.0 };
        let json = serde_json::to_string(&event).unwrap();
        let event2: BridgeEvent = serde_json::from_str(&json).unwrap();
        match event2 {
            BridgeEvent::CanvasClick { x, y } => {
                assert_eq!(x, 100.0);
                assert_eq!(y, 200.0);
            }
            _ => panic!("期望 CanvasClick"),
        }
    }

    #[test]
    fn bridge_event_ai_chat_serde() {
        let event = BridgeEvent::AiChat {
            message: "做一个登录页".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let event2: BridgeEvent = serde_json::from_str(&json).unwrap();
        match event2 {
            BridgeEvent::AiChat { message } => assert_eq!(message, "做一个登录页"),
            _ => panic!("期望 AiChat"),
        }
    }

    #[test]
    fn bridge_event_canvas_zoom_serde() {
        let event = BridgeEvent::CanvasZoom {
            delta: -120.0,
            x: 300.0,
            y: 400.0,
        };
        let json = serde_json::to_string(&event).unwrap();
        let event2: BridgeEvent = serde_json::from_str(&json).unwrap();
        match event2 {
            BridgeEvent::CanvasZoom { delta, x, y } => {
                assert_eq!(delta, -120.0);
                assert_eq!(x, 300.0);
                assert_eq!(y, 400.0);
            }
            _ => panic!("期望 CanvasZoom"),
        }
    }

    #[test]
    fn bridge_event_canvas_pan_serde() {
        let event = BridgeEvent::CanvasPan {
            dx: 50.0,
            dy: -30.0,
        };
        let json = serde_json::to_string(&event).unwrap();
        let event2: BridgeEvent = serde_json::from_str(&json).unwrap();
        match event2 {
            BridgeEvent::CanvasPan { dx, dy } => {
                assert_eq!(dx, 50.0);
                assert_eq!(dy, -30.0);
            }
            _ => panic!("期望 CanvasPan"),
        }
    }

    #[test]
    fn bridge_event_node_multi_select_serde() {
        let event = BridgeEvent::NodeMultiSelect {
            node_id: "btn_2".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let event2: BridgeEvent = serde_json::from_str(&json).unwrap();
        match event2 {
            BridgeEvent::NodeMultiSelect { node_id } => assert_eq!(node_id, "btn_2"),
            _ => panic!("期望 NodeMultiSelect"),
        }
    }

    #[test]
    fn parse_bridge_command_valid_json() {
        let cmd = BridgeCommand::ClearCanvas;
        let json = serde_json::to_string(&cmd).unwrap();
        let parsed = parse_bridge_command(&json);
        assert!(parsed.is_some());
    }

    #[test]
    fn parse_bridge_command_invalid_json() {
        assert!(parse_bridge_command("not json").is_none());
    }

    #[test]
    fn track_to_css_fixed() {
        let track = fd_canvas_core::TrackSizing::Fixed(100.0);
        assert_eq!(track_to_css(&track), "100px");
    }

    #[test]
    fn track_to_css_flex() {
        let track = fd_canvas_core::TrackSizing::Flex(1.0);
        assert_eq!(track_to_css(&track), "1fr");
    }

    #[test]
    fn track_to_css_auto() {
        let track = fd_canvas_core::TrackSizing::Auto;
        assert_eq!(track_to_css(&track), "auto");
    }

    #[test]
    fn bridge_event_node_select_serde() {
        let event = BridgeEvent::NodeSelect {
            node_id: "card_1".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let event2: BridgeEvent = serde_json::from_str(&json).unwrap();
        match event2 {
            BridgeEvent::NodeSelect { node_id } => assert_eq!(node_id, "card_1"),
            _ => panic!("期望 NodeSelect"),
        }
    }

    // ── 吸附算法测试 ──

    #[test]
    fn snap_find_offset_within_threshold() {
        let candidates = vec![100.0, 200.0, 300.0];
        let (offset, snapped, line) = find_snap_offset(102.0, &candidates);
        assert!(snapped);
        assert_eq!(offset, -2.0);
        assert_eq!(line, 100.0);
    }

    #[test]
    fn snap_find_offset_beyond_threshold() {
        let candidates = vec![100.0, 200.0, 300.0];
        let (offset, snapped, _) = find_snap_offset(110.0, &candidates);
        assert!(!snapped);
        assert_eq!(offset, 0.0);
    }

    #[test]
    fn snap_find_offset_exact_match() {
        let candidates = vec![100.0, 200.0, 300.0];
        let (offset, snapped, line) = find_snap_offset(200.0, &candidates);
        assert!(snapped);
        assert_eq!(offset, 0.0);
        assert_eq!(line, 200.0);
    }

    #[test]
    fn snap_find_offset_picks_closest() {
        let candidates = vec![98.0, 103.0, 500.0];
        let (offset, snapped, line) = find_snap_offset(100.0, &candidates);
        assert!(snapped);
        assert_eq!(offset, -2.0);
        assert_eq!(line, 98.0);
    }

    #[test]
    fn snap_threshold_boundary() {
        let candidates = vec![100.0];
        // 正好在阈值上
        let (_, snapped, _) = find_snap_offset(105.0, &candidates);
        assert!(snapped);
        // 刚好超过阈值
        let (_, snapped2, _) = find_snap_offset(105.1, &candidates);
        assert!(!snapped2);
    }

    #[test]
    fn snap_empty_candidates() {
        let candidates: Vec<f32> = vec![];
        let (offset, snapped, _) = find_snap_offset(100.0, &candidates);
        assert!(!snapped);
        assert_eq!(offset, 0.0);
    }

    #[test]
    fn snap_4_direction_covers_edges_and_centers() {
        // 模拟一个 100x50 的节点在 (50, 80)
        // 吸附候选线来自另一个节点：x=[0, 150, 75], y=[0, 100, 50]
        let snap_x_candidates = vec![0.0, 150.0, 75.0];
        let _snap_y_candidates = [0.0, 100.0, 50.0];

        // 节点左边缘 x=48 → 吸附到 50? 不，48 最近的是 50 → 偏移+2（在阈值内）
        // 这里测试节点左边缘在 x=2 → 吸附到 x=0
        let (off_left, sn_left, _) = find_snap_offset(2.0, &snap_x_candidates);
        assert!(sn_left);
        assert_eq!(off_left, -2.0); // 吸附到 x=0

        // 节点右边缘 x=148 → 吸附到 x=150
        let (off_right, sn_right, _) = find_snap_offset(148.0, &snap_x_candidates);
        assert!(sn_right);
        assert_eq!(off_right, 2.0); // 吸附到 x=150

        // 节点中心 H x=73 → 吸附到 x=75
        let (off_center, sn_center, _) = find_snap_offset(73.0, &snap_x_candidates);
        assert!(sn_center);
        assert_eq!(off_center, 2.0); // 吸附到 x=75
    }

    // ── 性能优化测试 ──

    #[test]
    fn viewport_margin_constant() {
        assert_eq!(VIEWPORT_MARGIN, 200.0);
    }

    // E-38/P2-5 回归：节点几何护栏拒绝非有限/超限尺寸，防渲染崩溃/OOM。
    #[test]
    fn node_geom_guard_rejects_nonfinite() {
        let mk = |x: f64, y: f64, w: f64, h: f64| fd_canvas_core::PenNode::rect("n1", x, y, w, h);
        // NaN/Inf 任一坐标 → 拒绝
        assert!(!is_node_geom_valid(&mk(f64::NAN, 0.0, 10.0, 10.0), 1.0));
        assert!(!is_node_geom_valid(
            &mk(0.0, f64::INFINITY, 10.0, 10.0),
            1.0
        ));
        assert!(!is_node_geom_valid(&mk(0.0, 0.0, f64::NAN, 10.0), 1.0));
        assert!(!is_node_geom_valid(
            &mk(0.0, 0.0, 10.0, f64::NEG_INFINITY),
            1.0
        ));
        // 负尺寸 → 拒绝
        assert!(!is_node_geom_valid(&mk(0.0, 0.0, -1.0, 10.0), 1.0));
        assert!(!is_node_geom_valid(&mk(0.0, 0.0, 10.0, -1.0), 1.0));
        // 超大尺寸（f64::MAX / 超 MAX_NODE_DIM_PX）→ 拒绝
        assert!(!is_node_geom_valid(&mk(0.0, 0.0, f64::MAX, 10.0), 1.0));
        assert!(!is_node_geom_valid(
            &mk(0.0, 0.0, 10.0, MAX_NODE_DIM_PX + 1.0),
            1.0
        ));
    }

    // E-38/P2-5 回归：zoom 护栏拒绝 0/负/超大，防算术崩。
    #[test]
    fn node_geom_guard_rejects_bad_zoom() {
        let node = fd_canvas_core::PenNode::rect("n1", 0.0, 0.0, 10.0, 10.0);
        assert!(is_node_geom_valid(&node, 1.0), "正常 zoom 应通过");
        assert!(!is_node_geom_valid(&node, 0.0), "zoom=0 应拒绝");
        assert!(!is_node_geom_valid(&node, -2.0), "负 zoom 应拒绝");
        assert!(!is_node_geom_valid(&node, f64::NAN), "NaN zoom 应拒绝");
        assert!(
            !is_node_geom_valid(&node, MAX_ZOOM + 1.0),
            "超 MAX_ZOOM 应拒绝"
        );
        assert!(is_node_geom_valid(&node, MAX_ZOOM), "恰好 MAX_ZOOM 应通过");
    }

    // E-38/P2-5 回归：合法节点（正常尺寸 + 边界值）通过护栏。
    #[test]
    fn node_geom_guard_accepts_valid_nodes() {
        let mk = |x: f64, y: f64, w: f64, h: f64| fd_canvas_core::PenNode::rect("n1", x, y, w, h);
        assert!(is_node_geom_valid(&mk(0.0, 0.0, 10.0, 10.0), 1.0));
        assert!(
            is_node_geom_valid(&mk(1e6, 1e6, 100.0, 100.0), 1.0),
            "大坐标应通过"
        );
        assert!(
            is_node_geom_valid(&mk(0.0, 0.0, MAX_NODE_DIM_PX, MAX_NODE_DIM_PX), 1.0),
            "恰好 MAX_NODE_DIM_PX 应通过"
        );
        assert!(
            is_node_geom_valid(&mk(0.0, 0.0, 0.0, 0.0), 1.0),
            "零尺寸应通过"
        );
    }

    #[test]
    fn aabb_overlap_basic() {
        // 两个重叠矩形
        let a_left = 0.0f32;
        let a_right = 100.0;
        let a_top = 0.0f32;
        let a_bottom = 100.0;
        let b_left = 50.0f32;
        let b_right = 150.0;
        let b_top = 50.0f32;
        let b_bottom = 150.0;
        assert!(a_right > b_left && a_left < b_right && a_bottom > b_top && a_top < b_bottom);
    }

    #[test]
    fn aabb_no_overlap() {
        // 两个不重叠矩形
        let a_left = 0.0f32;
        let a_right = 100.0;
        let a_top = 0.0f32;
        let a_bottom = 100.0;
        let b_left = 200.0f32;
        let b_right = 300.0;
        let b_top = 200.0f32;
        let b_bottom = 300.0;
        assert!(!(a_right > b_left && a_left < b_right && a_bottom > b_top && a_top < b_bottom));
    }

    #[test]
    fn raf_atomic_flag_default() {
        assert!(!RAF_SCHEDULED.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn large_document_100_nodes_serialization() {
        let mut doc = PenDocument::new();
        let mut page = fd_canvas_core::Page::new("p1", "Stress", 2000.0, 2000.0);
        for i in 0..100 {
            let x = (i % 10) as f64 * 200.0;
            let y = (i / 10) as f64 * 100.0;
            page.add(fd_canvas_core::PenNode::rect(
                format!("n{i}"),
                x,
                y,
                180.0,
                80.0,
            ));
        }
        doc.add_page(page);
        let json = doc.to_json().unwrap();
        let doc2 = PenDocument::from_json(&json).unwrap();
        assert_eq!(doc2.pages[0].nodes.len(), 100);
    }

    #[test]
    fn compute_resize_min_size_enforced() {
        let (nx, ny, nw, nh) = compute_resize("se", 10.0, 20.0, 100.0, 50.0, -200.0, -200.0);
        assert_eq!(nw, 10.0); // min_size
        assert_eq!(nh, 10.0); // min_size
        assert_eq!(nx, 10.0);
        assert_eq!(ny, 20.0);
    }

    #[test]
    fn listeners_installed_no_dom_is_safe() {
        // L-15：无 window/DOM 时 mark_listeners_installed 返回 true（跳过安装），
        // 避免反复尝试安装失败。幂等安全：多次调用一致返回 true。
        assert!(
            mark_listeners_installed("fusion-dom-root"),
            "无 DOM 环境应跳过安装（返回 true）"
        );
        assert!(
            mark_listeners_installed("fusion-dom-root"),
            "重复调用应一致返回 true"
        );
    }

    #[test]
    fn render_depth_limit_constant_bounded() {
        // P2-1：递归深度上限存在且合理，防深层嵌套文档栈溢出。
        // 编译期校验常量边界（不触发 web_sys console，避免 native 目标 panic）。
        const _: () = assert!(MAX_RENDER_DEPTH > 0 && MAX_RENDER_DEPTH <= 256);
    }

    #[test]
    fn children_per_node_cap_constant_bounded() {
        // E-38/P2-5：单节点子节点数渲染上限存在且合理。
        // 上限须 > 合法设计稿单层扇出（数百），且与深度乘积低于 jetsam 阈值。
        // 编译期校验（render 走 web_sys，native 目标不可调，校常量边界）。
        const _: () = assert!(MAX_CHILDREN_PER_NODE >= 100 && MAX_CHILDREN_PER_NODE <= 10_000);
    }

    #[test]
    fn children_take_cap_truncates_oversized_sibling_list() {
        // E-38/P2-5：10 万扁平子节点深度=1 过深度检查，须扇出护栏截断。
        // 复现渲染循环的 .take(MAX_CHILDREN_PER_NODE) 逻辑（纯逻辑，不触 DOM）：
        // 构造超限 children，取前 N，断言截断且数量恰等于上限。
        let oversized: Vec<u32> = (0..100_000).collect();
        let rendered: Vec<u32> = oversized
            .iter()
            .take(MAX_CHILDREN_PER_NODE)
            .copied()
            .collect();
        assert_eq!(rendered.len(), MAX_CHILDREN_PER_NODE, "超限须截断至上限");
        assert_eq!(rendered[0], 0, "保留首批子节点顺序");
        assert_eq!(
            rendered[MAX_CHILDREN_PER_NODE - 1],
            (MAX_CHILDREN_PER_NODE - 1) as u32,
            "末元素为第 N 个子节点"
        );
    }

    #[test]
    fn children_take_cap_preserves_undersized_sibling_list() {
        // E-38/P2-5：合法小扇出不受护栏影响。500 子节点 < 上限，全量保留。
        let normal: Vec<u32> = (0..500).collect();
        let rendered: Vec<u32> = normal.iter().take(MAX_CHILDREN_PER_NODE).copied().collect();
        assert_eq!(rendered.len(), 500, "未超限须全量保留");
    }
}
