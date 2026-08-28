//! ARCH-10：host-web 消息桥接。从 lib.rs 320-766 迁出，零逻辑改。
//!
//! 职责：
//! - `setup_message_listener`：监听原生宿主 message 事件
//! - `handle_host_message`：派发 HostMessage kind → 渲染/选中/清除等
//! - `send_to_host` / `dispatch_to_host`：WebView → 后端（WKWebView webkit.messageHandlers）
//! - `BridgeCommand` / `BridgeEvent`：桥消息协议 enum
//! - `parse_bridge_command` / `send_bridge_event`：协议序列化辅助
//! - `fusion_bridge_send_command`：wasm_bindgen JS 全局入口

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::{
    apply_tokens_css, clear_canvas, fd_log_error, fd_log_warn, mutate_node, remove_plan_preview,
    render_dom, render_page, render_plan_preview, reorder_node, reset_canvas_view, select_node,
    set_node_locked, set_node_visibility, LogEntry, LOG_RING,
};

// ── 消息桥接 ──

/// 设置 `message` 事件监听器，接收原生宿主消息。
pub(crate) fn setup_message_listener(window: &web_sys::Window) -> Result<(), JsValue> {
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
pub(crate) fn handle_host_message(json: &str) {
    // 解析 HostMessage 形状
    let msg: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(e) => {
            fd_log_error(&format!("fd-host-web: 反序列化消息失败: {e}"));
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
        // OPS-13：host 拉取 WASM 日志环形缓冲。payload 可选 { "clear": true } 拉取后清空。
        // 回传 kind=log_capture_dump，payload.entries=[{level,ts_ms,msg}]。
        // host 侧落盘诊断件；WASM 沙箱无文件系统，此为唯一外排路径。
        "log.capture.dump" => {
            let clear = msg
                .get("payload")
                .and_then(|p| p.get("clear"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let entries: Vec<LogEntry> = LOG_RING.with(|r| {
                let buf = r.borrow();
                let dump = buf.dump();
                drop(buf);
                if clear {
                    r.borrow_mut().clear();
                }
                dump
            });
            send_to_host(
                "log_capture_dump",
                &serde_json::json!({ "entries": entries }),
            );
        }
        other => {
            // L5：未识别 kind 用 warn 而非 debug，便于版本不匹配时定位
            fd_log_warn(&format!(
                "fd-host-web: 未识别消息 kind={other}，可能版本不匹配"
            ));
        }
    }
}

/// 宿主桥 handler 名（对应 studio 侧注册的 WKScriptMessageHandler name）。
/// 仅 wasm32 路径 dispatch_to_host 消费；非 wasm 构建不编译避免 dead_code 警告（CI clippy -D warnings）。
#[cfg(target_arch = "wasm32")]
pub(crate) const HOST_HANDLER_NAME: &str = "fdHost";

/// 向后端发送消息。
///
/// R-A22：桥接主路径走 `window.webkit.messageHandlers.fdHost.postMessage`，
/// 对齐 CLAUDE.md/README 声称的 WKWebView 标准原生回调。若该 handler 不存在
/// （旧版 studio 未注册 / 非 WKWebView 环境），回退到 `navigator.__fd_host_post`
/// 属性轮询契约，保持向后兼容。
pub(crate) fn send_to_host(kind: &str, payload: &serde_json::Value) {
    let msg = serde_json::json!({
        "direction": "WebViewToBackend",
        "kind": kind,
        "payload": payload,
    });
    let json = match serde_json::to_string(&msg) {
        Ok(s) => s,
        // E-41：序列化失败显式告警而非吞消息（旧实现 unwrap_or_default="" 静默丢消息）。
        Err(e) => {
            fd_log_warn(&format!(
                "fd-host-web: send_to_host 序列化失败 kind={kind} err={e}"
            ));
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
    // 回退：navigator.__fd_host_post 事件队列（非 WKWebView 宿主用，如 headless harness）。
    // OPS-11/F-19：原单值 Reflect::set 是 last-write-wins——同 tick 多事件
    // （如 click 同时发 NodeClick+NodeSelect+CanvasClick）后者覆盖前者，静默丢事件。
    // 改累积队列：__fd_host_post 维护字符串数组，宿主轮询 shift 消费。studio 走 webkit
    // 路径不经此，无回归。
    let nav = w.navigator();
    let prop = JsValue::from_str("__fd_host_post");
    let existing = js_sys::Reflect::get(&nav, &prop).unwrap_or(JsValue::undefined());
    let queue = if let Some(arr) = existing.dyn_ref::<js_sys::Array>() {
        arr.clone()
    } else {
        let arr = js_sys::Array::new();
        let _ = js_sys::Reflect::set(&nav, &prop, arr.as_ref());
        arr
    };
    queue.push(&JsValue::from_str(json));
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
pub(crate) fn parse_bridge_command(json: &str) -> Option<BridgeCommand> {
    serde_json::from_str(json).ok()
}

/// 将 BridgeEvent 序列化为 JSON 并发送到后端。
pub(crate) fn send_bridge_event(event: BridgeEvent) {
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
