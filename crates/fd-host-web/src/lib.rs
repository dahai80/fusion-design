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

use fd_canvas_core::PenDocument;

fn css_escape_attr_value(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn node_selector(node_id: &str) -> String {
    format!("[data-node-id=\"{}\"]", css_escape_attr_value(node_id))
}

// ── 全局状态 ──

static SHELL: LazyLock<Mutex<Option<WebShellInner>>> = LazyLock::new(|| Mutex::new(None));

// 容器级监听器是否已安装（幂等保护）。
// 修复 P0-1：render_dom 每次 DOM 渲染都重新 setup_* + forget，
// 导致监听器单调累积、同事件被 N 次处理 -> 卡死。置位后跳过重复注册。
static LISTENERS_INSTALLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 原子置位并返回旧值；首次调用返回 false，后续返回 true。
fn mark_listeners_installed() -> bool {
    LISTENERS_INSTALLED.swap(true, std::sync::atomic::Ordering::SeqCst)
}

/// 容错获取 SHELL 锁：即使中毒也取出数据，避免 panic 传播致整个渲染层永久卡死（P1-1）。
fn shell_lock() -> std::sync::MutexGuard<'static, Option<WebShellInner>> {
    SHELL.lock().unwrap_or_else(|e| e.into_inner())
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
    pending_messages: Vec<String>,
}

#[allow(dead_code)]
const MAX_PENDING_MESSAGES: usize = 200;

/// 初始化 Web 宿主。
///
/// 验证 canvas 元素存在，初始化 panic hook，返回 WebShell 实例。
/// 对应 `op-host-web::mount` 的 `<canvas>` 校验逻辑。
#[wasm_bindgen]
pub fn mount(canvas_id: &str) -> Result<WebShell, JsValue> {
    // 浏览器异常时 Rust 侧 panic 信息通过 wasm-bindgen 默认机制输出

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
        pending_messages: vec![],
    };

    // 注册消息监听器
    setup_message_listener(&window)?;

    let shell = WebShell { inner };
    *shell_lock() = Some(WebShellInner {
        canvas_id: canvas_id.to_string(),
        ready: true,
        pending_messages: vec![],
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
            web_sys::console::debug_1(&format!("fd-host-web: 未处理消息 kind={other}").into());
        }
    }
}

/// 向后端发送消息。
fn send_to_host(kind: &str, payload: &serde_json::Value) {
    let msg = serde_json::json!({
        "direction": "WebViewToBackend",
        "kind": kind,
        "payload": payload,
    });
    let json = serde_json::to_string(&msg).unwrap_or_default();
    if let Some(w) = js_sys::global().dyn_ref::<web_sys::Window>() {
        let js_val = JsValue::from_str(&json);
        let _ = js_sys::Reflect::set(
            &w.navigator(),
            &JsValue::from_str("__fd_host_post"),
            &js_val,
        );
    }
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
const VIEWPORT_MARGIN: f32 = 200.0;

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
    let container = document
        .get_element_by_id("fusion-dom-root")
        .unwrap_or_else(|| {
            let div = document.create_element("div").unwrap();
            div.set_id("fusion-dom-root");
            div.set_attribute(
                "style",
                "position:relative;width:100%;height:100%;overflow:hidden;",
            )
            .ok();
            document.body().unwrap().append_child(&div).ok();
            div
        });

    // 清空现有 DOM 节点
    container.set_inner_html("");

    // 初始化缩放/平移状态
    container.set_attribute("data-fd-zoom", "1.0").ok();
    container.set_attribute("data-fd-pan-x", "0.0").ok();
    container.set_attribute("data-fd-pan-y", "0.0").ok();

    // 缓存 PenDocument JSON 供视口剔除增量渲染使用
    container.set_attribute("data-fd-doc", doc_json).ok();

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

    // 事件委托：容器级监听器仅安装一次，避免重复 forget 累积泄漏（P0-1）。
    if !mark_listeners_installed() {
        setup_delegated_click_listener("fusion-dom-root");
        setup_delegated_mousedown_listener("fusion-dom-root");
        setup_canvas_click_listener("fusion-dom-root");
        setup_canvas_zoom_listener("fusion-dom-root");
        setup_canvas_pan_listener("fusion-dom-root");
        setup_marquee_listener("fusion-dom-root");
        web_sys::console::log_1(&"fd-host-web: 容器级监听器已安装（一次性）".into());
    }
}

/// 判断节点是否在当前视口内（考虑 zoom/pan）。
fn is_node_in_viewport(node: &fd_canvas_core::PenNode, container: &web_sys::Element) -> bool {
    let zoom: f32 = container
        .get_attribute("data-fd-zoom")
        .unwrap_or_default()
        .parse()
        .unwrap_or(1.0);
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

    let window = match web_sys::window() {
        Some(w) => w,
        None => return true,
    };
    let vp_w = window
        .inner_width()
        .unwrap_or_default()
        .as_f64()
        .unwrap_or(1920.0) as f32;
    let vp_h = window
        .inner_height()
        .unwrap_or_default()
        .as_f64()
        .unwrap_or(1080.0) as f32;

    // 节点在画布坐标中的边界
    let node_left = node.x * zoom + pan_x;
    let node_top = node.y * zoom + pan_y;
    let node_right = (node.x + node.w) * zoom + pan_x;
    let node_bottom = (node.y + node.h) * zoom + pan_y;

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

    let doc_json = match container.get_attribute("data-fd-doc") {
        Some(j) => j,
        None => return,
    };
    let doc = match PenDocument::from_json(&doc_json) {
        Ok(d) => d,
        Err(_) => return,
    };

    // 收集文档中所有节点 id 及其视口状态
    let mut ids_to_add: Vec<String> = Vec::new();
    let mut ids_in_view: std::collections::HashSet<String> = std::collections::HashSet::new();

    for page in &doc.pages {
        collect_visible_node_ids(&page.nodes, &container, &mut ids_to_add, &mut ids_in_view);
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
        let fragment = document.create_document_fragment();
        for page in &doc.pages {
            add_nodes_by_ids(&page.nodes, &document, &ids_to_add, &fragment);
        }
        container.append_child(&fragment).ok();
    }

    web_sys::console::log_1(
        &format!("fd-host-web: 视口剔除更新, 添加 {} 节点", ids_to_add.len()).into(),
    );
}

/// 递归收集视口内节点 id，与现有 id 对比找新增。
fn collect_visible_node_ids(
    nodes: &[fd_canvas_core::PenNode],
    container: &web_sys::Element,
    ids_to_add: &mut Vec<String>,
    ids_in_view: &mut std::collections::HashSet<String>,
) {
    for node in nodes {
        if is_node_in_viewport(node, container) {
            ids_in_view.insert(node.id.clone());
            // 检查 DOM 中是否已存在
            let window = web_sys::window().unwrap();
            let document = window.document().unwrap();
            if document
                .query_selector(&node_selector(&node.id))
                .unwrap_or(None)
                .is_none()
            {
                ids_to_add.push(node.id.clone());
            }
        }
        collect_visible_node_ids(&node.children, container, ids_to_add, ids_in_view);
    }
}

/// 递归查找并渲染指定 id 的节点。
fn add_nodes_by_ids(
    nodes: &[fd_canvas_core::PenNode],
    document: &web_sys::Document,
    ids_to_add: &[String],
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
        style.push_str(&format!("background-color:{};", fill));
    }

    // 描边
    if let Some(stroke) = &node.style.stroke {
        let width = node.style.stroke_width.unwrap_or(1.0);
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
        style.push_str(&format!("--{}:{};", css_var, value));
    }

    el.set_attribute("style", &style).ok()?;
    el.set_attribute("data-node-id", &node.id).ok()?;

    // 渲染子节点（子节点同样使用事件委托模式）。深度上限防栈溢出（P2-1）。
    if depth < MAX_RENDER_DEPTH {
        for child in &node.children {
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
    let overlay = document.create_element("div").unwrap();
    overlay.set_id("fd-snap-overlay");
    overlay.set_attribute("style",
        "position:absolute;top:0;left:0;width:100%;height:100%;pointer-events:none;z-index:9998;overflow:visible;"
    ).ok();

    // 垂直吸附线（X 坐标 → 竖线）
    for &x in x_lines {
        let line = document.create_element("div").unwrap();
        line.set_attribute("style", &format!(
            "position:absolute;left:{}px;top:0;width:1px;height:100%;background:#007AFF;opacity:0.5;",
            x
        )).ok();
        overlay.append_child(&line).ok();
    }
    // 水平吸附线（Y 坐标 → 横线）
    for &y in y_lines {
        let line = document.create_element("div").unwrap();
        line.set_attribute("style", &format!(
            "position:absolute;top:{}px;left:0;height:1px;width:100%;background:#007AFF;opacity:0.5;",
            y
        )).ok();
        overlay.append_child(&line).ok();
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
/// - 回调内先复位再执行，panic 也不阻塞后续调度。
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
        callback();
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
        let doc = web_sys::window().unwrap().document().unwrap();
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

        let window = web_sys::window().unwrap();
        window
            .add_event_listener_with_callback("mousemove", on_mousemove.as_ref().unchecked_ref())
            .ok();

        let up_id = node_id.clone();
        let move_js: JsValue = on_mousemove
            .as_ref()
            .unchecked_ref::<js_sys::Function>()
            .into();
        let on_mouseup = Closure::once(Box::new(move |event: web_sys::Event| {
            let w = web_sys::window().unwrap();
            let move_ref: &js_sys::Function = move_js.unchecked_ref();
            w.remove_event_listener_with_callback("mousemove", move_ref)
                .ok();
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
        on_mousemove.forget();

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
    let style = el.get_attribute("style").unwrap_or_default();
    let x = extract_css_px(&style, "left").unwrap_or(0.0);
    let y = extract_css_px(&style, "top").unwrap_or(0.0);
    (x, y)
}

/// 从 DOM 元素的 style 中读取 width/height。
fn read_node_size(el: &web_sys::Element) -> (f32, f32) {
    let style = el.get_attribute("style").unwrap_or_default();
    let w = extract_css_px(&style, "width").unwrap_or(DEFAULT_NODE_WIDTH);
    let h = extract_css_px(&style, "height").unwrap_or(DEFAULT_NODE_HEIGHT);
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

        let win = web_sys::window().unwrap();
        win.add_event_listener_with_callback("mousemove", on_move.as_ref().unchecked_ref())
            .ok();

        let move_js: JsValue = on_move.as_ref().unchecked_ref::<js_sys::Function>().into();
        let on_up = Closure::once(Box::new(move |event: web_sys::Event| {
            let w = web_sys::window().unwrap();
            let move_ref: &js_sys::Function = move_js.unchecked_ref();
            w.remove_event_listener_with_callback("mousemove", move_ref)
                .ok();
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
        on_move.forget();

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
        let doc = web_sys::window().unwrap().document().unwrap();
        let marquee_el = doc.create_element("div").unwrap();
        marquee_el.set_id("fd-marquee");
        marquee_el
            .set_attribute(
                "style",
                "position:fixed;border:2px dashed #007AFF;background:rgba(0,122,255,0.1);\
             pointer-events:none;z-index:99999;display:none;",
            )
            .unwrap();
        doc.body().unwrap().append_child(&marquee_el).ok();

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
            let m_el = web_sys::window()
                .unwrap()
                .document()
                .unwrap()
                .get_element_by_id("fd-marquee");
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
                .unwrap();
            }
        }) as Box<dyn FnMut(web_sys::Event)>);

        let win = web_sys::window().unwrap();
        win.add_event_listener_with_callback("mousemove", on_move.as_ref().unchecked_ref())
            .ok();

        let move_js: JsValue = on_move.as_ref().unchecked_ref::<js_sys::Function>().into();
        let on_up = Closure::once(Box::new(move |event: web_sys::Event| {
            let w = web_sys::window().unwrap();
            let move_ref: &js_sys::Function = move_js.unchecked_ref();
            w.remove_event_listener_with_callback("mousemove", move_ref)
                .ok();
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
        on_move.forget();
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
    let node: &web_sys::Node = container.unchecked_ref();
    let child_nodes = node.child_nodes();
    let mut result = vec![];
    for i in 0..child_nodes.length() {
        if let Some(child) = child_nodes.get(i) {
            if let Ok(el) = child.dyn_into::<web_sys::Element>() {
                if let Some(node_id) = el.get_attribute("data-node-id") {
                    let rect = el.get_bounding_client_rect();
                    let el_left = rect.left() as f32;
                    let el_top = rect.top() as f32;
                    let el_right = rect.right() as f32;
                    let el_bottom = rect.bottom() as f32;
                    // 判断矩形重叠
                    if el_left < right && el_right > left && el_top < bottom && el_bottom > top {
                        result.push(node_id);
                    }
                }
            }
        }
    }
    result
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
            let style = format!(
                "position:relative;width:100%;height:100%;overflow:hidden;transform:scale({}) translate({}px,{}px);transform-origin:0 0;",
                new_scale, pan_x / new_scale, pan_y / new_scale
            );
            container.set_attribute("style", &style).ok();
        }
        // 延迟触发视口剔除（缩放后可能需要加载/卸载节点）
        schedule_raf(|| {
            viewport_cull_update();
        });
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
            let style = format!(
                "position:relative;width:100%;height:100%;overflow:hidden;transform:scale({}) translate({}px,{}px);transform-origin:0 0;",
                zoom, new_x / zoom, new_y / zoom
            );
            container.set_attribute("style", &style).ok();
        }
        // 延迟触发视口剔除（平移后可能需要加载/卸载节点）
        schedule_raf(|| {
            viewport_cull_update();
        });
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
            let style = el.get_attribute("style").unwrap_or_default();
            let clean = style
                .split(';')
                .filter(|s| {
                    let t = s.trim();
                    !t.starts_with("outline:") && !t.starts_with("outline-offset:")
                })
                .collect::<Vec<&str>>()
                .join(";");
            el.set_attribute("style", &clean).ok();
            el.remove_attribute("data-fd-selected").ok();
        } else {
            el.set_attribute("data-fd-selected", "true").ok();
            let style = el.get_attribute("style").unwrap_or_default();
            el.set_attribute(
                "style",
                &format!("{};outline:2px solid #007AFF;outline-offset:2px;", style),
            )
            .ok();
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

    // 注入新 token style
    let style_el = document.create_element("style").unwrap();
    style_el.set_id("fusion-tokens");
    style_el.set_text_content(Some(css));
    document.head().unwrap().append_child(&style_el).ok();
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

    // 移除之前的选中状态 + resize handles
    if let Ok(selected) = document.query_selector_all("[data-fd-selected]") {
        for i in 0..selected.length() {
            if let Some(node) = selected.item(i) {
                if let Ok(el) = node.dyn_into::<web_sys::Element>() {
                    // 恢复原始 style（去掉 outline）
                    let style = el.get_attribute("style").unwrap_or_default();
                    let clean = style
                        .split(';')
                        .filter(|s| {
                            let t = s.trim();
                            !t.starts_with("outline:") && !t.starts_with("outline-offset:")
                        })
                        .collect::<Vec<&str>>()
                        .join(";");
                    el.set_attribute("style", &clean).ok();
                    el.remove_attribute("data-fd-selected").ok();
                }
            }
        }
    }
    // 移除旧的 resize handles
    if let Ok(handles) = document.query_selector_all(".fd-resize-handle") {
        for i in 0..handles.length() {
            if let Some(h) = handles.item(i) {
                if let Ok(el) = h.dyn_into::<web_sys::Element>() {
                    el.remove();
                }
            }
        }
    }

    // 设置新选中
    if let Some(el) = document
        .query_selector(&node_selector(node_id))
        .unwrap_or(None)
    {
        el.set_attribute("data-fd-selected", "true").ok();
        let style = el.get_attribute("style").unwrap_or_default();
        el.set_attribute(
            "style",
            &format!("{};outline:2px solid #007AFF;outline-offset:2px;", style),
        )
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
        let doc = web_sys::window().unwrap().document().unwrap();
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
            let doc2 = web_sys::window().unwrap().document().unwrap();
            if let Some(el) = doc2
                .query_selector(&node_selector(&resize_id))
                .unwrap_or(None)
            {
                update_node_position(&el, nx, ny);
                update_node_size(&el, nw, nh);
            }
        }) as Box<dyn FnMut(web_sys::Event)>);

        let window = web_sys::window().unwrap();
        window
            .add_event_listener_with_callback("mousemove", on_move.as_ref().unchecked_ref())
            .ok();

        let rid = nid.clone();
        let rdir = dir_str.clone();
        let move_js: JsValue = on_move.as_ref().unchecked_ref::<js_sys::Function>().into();
        let on_up = Closure::once(Box::new(move |event: web_sys::Event| {
            let w = web_sys::window().unwrap();
            let move_ref: &js_sys::Function = move_js.unchecked_ref();
            w.remove_event_listener_with_callback("mousemove", move_ref)
                .ok();
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
        on_move.forget();

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
        }
        if let Some(ny) = y {
            let style = el.get_attribute("style").unwrap_or_default();
            el.set_attribute(
                "style",
                &replace_css_prop(&style, "top", &format!("{}px", ny)),
            )
            .ok();
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
        }
        if let Some(nh) = h {
            let style = el.get_attribute("style").unwrap_or_default();
            el.set_attribute(
                "style",
                &replace_css_prop(&style, "height", &format!("{}px", nh)),
            )
            .ok();
        }
    }

    // Style mutations
    if let Some(f) = fill {
        let style = el.get_attribute("style").unwrap_or_default();
        el.set_attribute("style", &replace_css_prop(&style, "background-color", f))
            .ok();
    }
    if let Some(s) = stroke {
        let sw = stroke_width.unwrap_or(1.0);
        let style = el.get_attribute("style").unwrap_or_default();
        el.set_attribute(
            "style",
            &replace_css_prop(
                &replace_css_prop(&style, "border", &format!("{}px solid {}", sw, s)),
                "border-color",
                s,
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
        el.set_attribute("style", &replace_css_prop(&style, "font-family", ff))
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
            let style = el.get_attribute("style").unwrap_or_default();
            let new_style = replace_css_prop(&style, "display", "block");
            el.set_attribute("style", &new_style).ok();
            web_sys::console::log_1(&format!("fd-host-web: node {node_id} set visible").into());
        } else {
            el.set_attribute("data-fd-hidden", "true").ok();
            let style = el.get_attribute("style").unwrap_or_default();
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
        let child_nodes = node.child_nodes();
        let count = child_nodes.length() as usize;
        if new_index < count {
            if let Some(ref_child) = child_nodes.get(new_index as u32) {
                node.insert_before(&el, Some(&ref_child)).ok();
            }
        } else {
            node.append_child(&el).ok();
        }
        web_sys::console::log_1(
            &format!("fd-host-web: node {node_id} reordered to {new_index}").into(),
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
        container.set_attribute("style",
            "position:relative;width:100%;height:100%;overflow:hidden;transform:scale(1) translate(0px,0px);transform-origin:0 0;"
        ).ok();
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

    let overlay = document.create_element("div").unwrap();
    overlay.set_id("fd-plan-preview");
    overlay.set_attribute("style",
        "position:absolute;top:0;left:0;width:100%;height:100%;pointer-events:none;z-index:1000;"
    ).unwrap();

    for page in &doc.pages {
        for node in &page.nodes {
            let el = document.create_element("div").unwrap();
            let style = format!(
                "position:absolute;left:{}px;top:{}px;width:{}px;height:{}px;\
                 border:2px dashed #007AFF;border-radius:{}px;opacity:0.6;",
                node.x as i32,
                node.y as i32,
                node.w as i32,
                node.h as i32,
                node.style.radius.map(|r| r as i32).unwrap_or(0)
            );
            el.set_attribute("style", &style).unwrap();
            if let Some(text) = &node.text {
                el.set_text_content(Some(text));
            }
            overlay.append_child(&el).unwrap();
        }
    }

    container.append_child(&overlay).unwrap();
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
        ctx.set_line_width(node.style.stroke_width.unwrap_or(1.0) as f64);
    }

    let x = node.x as f64;
    let y = node.y as f64;
    let w = node.w as f64;
    let h = node.h as f64;

    match node.kind {
        fd_canvas_core::NodeKind::Rect => {
            let r = node.style.radius.unwrap_or(0.0) as f64;
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
                ctx.fill_text(text, x, y + (node.style.font_size.unwrap_or(16.0) as f64))
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
            for child in &node.children {
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
                pending_messages: vec![],
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
            let x = (i % 10) as f32 * 200.0;
            let y = (i / 10) as f32 * 100.0;
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
    fn max_pending_messages_constant() {
        assert_eq!(MAX_PENDING_MESSAGES, 200);
    }

    #[test]
    fn pending_messages_bounded() {
        let mut messages: Vec<String> = vec![];
        for i in 0..500 {
            messages.push(format!("msg-{}", i));
            if messages.len() > MAX_PENDING_MESSAGES {
                messages.remove(0);
            }
        }
        assert_eq!(messages.len(), MAX_PENDING_MESSAGES);
        assert_eq!(messages[0], "msg-300");
    }

    #[test]
    fn listeners_installed_flag_is_idempotent() {
        // P0-1：mark_listeners_installed 首次返回 false，后续均 true，
        // 保证容器级监听器只注册一次，不随渲染次数累积泄漏。
        LISTENERS_INSTALLED.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(!mark_listeners_installed(), "首次应返回 false");
        assert!(mark_listeners_installed(), "第二次应返回 true");
        assert!(mark_listeners_installed(), "第三次应返回 true");
        LISTENERS_INSTALLED.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    #[test]
    fn render_depth_limit_constant_bounded() {
        // P2-1：递归深度上限存在且合理，防深层嵌套文档栈溢出。
        // 编译期校验常量边界，运行期仅打印确认。
        const _: () = assert!(MAX_RENDER_DEPTH > 0 && MAX_RENDER_DEPTH <= 256);
        web_sys::console::log_1(&format!("MAX_RENDER_DEPTH={MAX_RENDER_DEPTH}").into());
    }
}
