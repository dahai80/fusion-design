//! ARCH-10: host-web message bridging. Moved out of lib.rs lines 320-766 with zero logic changes.
//!
//! Responsibilities:
//! - `setup_message_listener`: listens for native host message events
//! - `handle_host_message`: dispatches HostMessage kinds → render/select/clear etc.
//! - `send_to_host` / `dispatch_to_host`: WebView → backend (WKWebView webkit.messageHandlers)
//! - `BridgeCommand` / `BridgeEvent`: bridge message protocol enums
//! - `parse_bridge_command` / `send_bridge_event`: protocol serialization helpers
//! - `fusion_bridge_send_command`: wasm_bindgen JS global entry point

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::{
    apply_tokens_css, clear_canvas, fd_log_error, fd_log_warn, mutate_node, remove_plan_preview,
    render_dom, render_page, render_plan_preview, reorder_node, reset_canvas_view, select_node,
    set_node_locked, set_node_visibility, LogEntry, LOG_RING,
};

// ── Message bridging ──

/// Set up the `message` event listener for receiving native host messages.
pub(crate) fn setup_message_listener(window: &web_sys::Window) -> Result<(), JsValue> {
    let handler = Closure::wrap(Box::new(move |event: web_sys::MessageEvent| {
        if let Some(data) = event.data().as_string() {
            handle_host_message(&data);
        }
    }) as Box<dyn FnMut(web_sys::MessageEvent)>);

    window.add_event_listener_with_callback("message", handler.as_ref().unchecked_ref())?;
    handler.forget(); // prevent GC collection
    Ok(())
}

/// Handle a message from the native host (Fusion-Desk).
pub(crate) fn handle_host_message(json: &str) {
    // Parse the HostMessage shape
    let msg: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(e) => {
            fd_log_error(&format!("fd-host-web: message deserialization failed: {e}"));
            return;
        }
    };

    let kind = msg
        .get("kind")
        .and_then(|k| k.as_str())
        .unwrap_or("unknown");

    // L-16: protocol version guard — if schema_version exceeds the version this side supports, refuse
    // to process it and send back an error event, avoiding silently mis-parsing new protocol messages
    // with old semantics.
    const HOST_PROTOCOL_VERSION: u64 = 1;
    if let Some(v) = msg.get("schema_version").and_then(|x| x.as_u64()) {
        if v > HOST_PROTOCOL_VERSION {
            web_sys::console::warn_1(
                &format!(
                    "fd-host-web: message schema_version={v} exceeds the supported version {HOST_PROTOCOL_VERSION}, refusing to process"
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
            // Render the PenDocument page
            if let Some(payload) = msg.get("payload") {
                if let Some(doc_json) = payload.get("document").and_then(|d| d.as_str()) {
                    render_page(doc_json);
                }
            }
        }
        "page.render-dom" => {
            // DOM rendering pipeline
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
            web_sys::console::log_1(&"fd-host-web: host ready".into());
        }
        // OPS-13: the host pulls the WASM log ring buffer. Optional payload { "clear": true } clears it after the pull.
        // Replies with kind=log_capture_dump, payload.entries=[{level,ts_ms,msg}].
        // The host side persists the diagnostics; the WASM sandbox has no filesystem, so this is the only outbound path.
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
            // L5: unknown kinds warn instead of debug, easing version-mismatch triage
            fd_log_warn(&format!(
                "fd-host-web: unrecognized message kind={other}, possible version mismatch"
            ));
        }
    }
}

/// Host bridge handler name (matching the WKScriptMessageHandler name registered on the studio side).
/// Consumed only by the wasm32 dispatch_to_host path; not compiled on non-wasm builds to avoid a
/// dead_code warning (CI clippy -D warnings).
#[cfg(target_arch = "wasm32")]
pub(crate) const HOST_HANDLER_NAME: &str = "fdHost";

/// Send a message to the backend.
///
/// R-A22: the main bridging path uses `window.webkit.messageHandlers.fdHost.postMessage`,
/// aligning with the WKWebView standard native callback claimed by CLAUDE.md/README. If that handler
/// does not exist (older studio without registration / non-WKWebView environment), fall back to the
/// `navigator.__fd_host_post` property-polling contract for backward compatibility.
pub(crate) fn send_to_host(kind: &str, payload: &serde_json::Value) {
    let msg = serde_json::json!({
        "direction": "WebViewToBackend",
        "kind": kind,
        "payload": payload,
    });
    let json = match serde_json::to_string(&msg) {
        Ok(s) => s,
        // E-41: serialization failure warns explicitly instead of swallowing the message (the old
        // unwrap_or_default="" silently dropped messages).
        Err(e) => {
            fd_log_warn(&format!(
                "fd-host-web: send_to_host serialization failed kind={kind} err={e}"
            ));
            return;
        }
    };
    // js_sys::global panics on non-wasm targets (imported statics unavailable);
    // dispatch only on wasm32; native test environments have no window, so return directly.
    #[cfg(target_arch = "wasm32")]
    {
        dispatch_to_host(&json, kind);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (json, kind);
    }
}

/// wasm32-only: dispatch a message to the native host per the R-A22 bridging contract.
#[cfg(target_arch = "wasm32")]
fn dispatch_to_host(json: &str, kind: &str) {
    let global = js_sys::global();
    let Some(w) = global.dyn_ref::<web_sys::Window>() else {
        return;
    };
    // Main path: webkit.messageHandlers.<handler>.postMessage (WKWebView standard).
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
                                "fd-host-web: webkit.messageHandlers.{HOST_HANDLER_NAME}.postMessage call failed kind={kind}"
                            )
                            .into(),
                        );
                    }
                }
            }
        }
    }
    // Fallback: navigator.__fd_host_post event queue (for non-WKWebView hosts, e.g. headless harness).
    // OPS-11/F-19: the original single-value Reflect::set was last-write-wins — multiple events in the
    // same tick (e.g. a click emitting NodeClick+NodeSelect+CanvasClick together) overwrote each other,
    // silently dropping events.
    // Switched to an accumulating queue: __fd_host_post holds a string array the host polls via shift.
    // studio goes through the webkit path and never reaches this; no regression.
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

// ── Bridge message protocol ──

/// Command types sent backend → WebView.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum BridgeCommand {
    /// Render a PenDocument page
    PageRender { document_json: String },
    /// Set the design token CSS
    ApplyTokens { css: String },
    /// Select a node
    SelectNode { node_id: String },
    /// Modify node position/size/style
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
    /// Clear the canvas
    ClearCanvas,
    /// Plan preview: show the nodes about to be written with a dashed outline
    PlanPreview { document_json: String },
    /// Apply the plan: remove the dashed preview layer
    PlanApply,
    /// Reject the plan: remove the dashed preview layer
    PlanReject,
    /// Set node visibility
    SetNodeVisibility { node_id: String, visible: bool },
    /// Set node locked state
    SetNodeLocked { node_id: String, locked: bool },
    /// Reorder a node (move to the target position)
    ReorderNode { node_id: String, new_index: usize },
}

/// Event types sent WebView → backend.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum BridgeEvent {
    /// A node was clicked
    NodeClick { node_id: String, x: f32, y: f32 },
    /// A node was dragged
    NodeDrag { node_id: String, dx: f32, dy: f32 },
    /// A node was resized
    NodeResize { node_id: String, w: f32, h: f32 },
    /// A node was selected
    NodeSelect { node_id: String },
    /// Node multi-select (Shift+click appends)
    NodeMultiSelect { node_id: String },
    /// The canvas area was clicked (blank area)
    CanvasClick { x: f32, y: f32 },
    /// Canvas zoom (wheel event)
    CanvasZoom { delta: f32, x: f32, y: f32 },
    /// Canvas pan (middle button / Space+drag)
    CanvasPan { dx: f32, dy: f32 },
    /// Marquee selection finished (node id list inside the marquee rectangle)
    MarqueeSelect { node_ids: Vec<String> },
    /// User entered an AI chat message
    AiChat { message: String },
}

/// Parse a BridgeCommand from JSON.
pub(crate) fn parse_bridge_command(json: &str) -> Option<BridgeCommand> {
    serde_json::from_str(json).ok()
}

/// Serialize a BridgeEvent to JSON and send it to the backend.
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

// ── fusionBridge: JS global object for the native side to call ──

/// fusionBridge.sendCommand(commandJson) — called by the WKWebView native side.
#[wasm_bindgen]
pub fn fusion_bridge_send_command(command_json: &str) -> Result<(), JsValue> {
    let command = parse_bridge_command(command_json)
        .ok_or_else(|| JsValue::from_str("failed to parse BridgeCommand"))?;

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
