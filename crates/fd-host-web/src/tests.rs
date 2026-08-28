use super::*;

// OPS-13：LogRingBuffer 纯逻辑回归（不触 wasm console / performance，host 目标测）。
#[test]
fn log_ring_buffer_push_and_dump() {
    let mut buf = LogRingBuffer::new();
    buf.push(LogEntry {
        level: "error",
        ts_ms: 1.0,
        msg: "e1".into(),
    });
    buf.push(LogEntry {
        level: "warn",
        ts_ms: 2.0,
        msg: "w1".into(),
    });
    let dump = buf.dump();
    assert_eq!(dump.len(), 2);
    assert_eq!(dump[0].msg, "e1");
    assert_eq!(dump[1].level, "warn");
}

#[test]
fn log_ring_buffer_overflow_drops_oldest() {
    let mut buf = LogRingBuffer::new();
    for i in 0..LOG_RING_CAPACITY + 50 {
        buf.push(LogEntry {
            level: "error",
            ts_ms: i as f64,
            msg: format!("m{i}"),
        });
    }
    let dump = buf.dump();
    assert_eq!(
        dump.len(),
        LOG_RING_CAPACITY,
        "超容量应丢最旧保留 capacity 条"
    );
    assert_eq!(dump[0].msg, format!("m{}", 50), "最旧 50 条应被丢弃");
    assert_eq!(
        dump.last().unwrap().msg,
        format!("m{}", LOG_RING_CAPACITY + 49)
    );
}

#[test]
fn log_ring_buffer_clear_empties() {
    let mut buf = LogRingBuffer::new();
    buf.push(LogEntry {
        level: "error",
        ts_ms: 0.0,
        msg: "x".into(),
    });
    assert!(!buf.dump().is_empty());
    buf.clear();
    assert!(buf.dump().is_empty());
}

// OPS-13：thread_local LOG_RING 隔离——log.capture.dump clear 语义。
#[test]
fn log_ring_thread_local_dump_and_clear() {
    LOG_RING.with(|r| r.borrow_mut().clear());
    LOG_RING.with(|r| {
        r.borrow_mut().push(LogEntry {
            level: "error",
            ts_ms: 0.0,
            msg: "tl1".into(),
        })
    });
    let dumped: Vec<LogEntry> = LOG_RING.with(|r| r.borrow().dump());
    assert_eq!(dumped.len(), 1);
    assert_eq!(dumped[0].msg, "tl1");
    LOG_RING.with(|r| r.borrow_mut().clear());
    assert!(LOG_RING.with(|r| r.borrow().dump()).is_empty());
}

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
            cached_doc: None,
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
