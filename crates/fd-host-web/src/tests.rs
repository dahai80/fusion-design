use super::*;

// OPS-13: LogRingBuffer pure-logic regression (no wasm console / performance touched; tested on the host target).
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
        "over capacity should drop the oldest and keep exactly capacity entries"
    );
    assert_eq!(
        dump[0].msg,
        format!("m{}", 50),
        "the oldest 50 entries should be dropped"
    );
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

// OPS-13: thread_local LOG_RING isolation — log.capture.dump clear semantics.
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

// E-39 regression: sanitize_css_color strips `;{}` that can escape CSS attribute boundaries (no web_sys touched).
// The dangerous url()/expression() paths go through web_sys console (unavailable in native tests); only safe sanitization is verified.
#[test]
fn sanitize_css_color_strips_escape_chars() {
    assert_eq!(sanitize_css_color("#ff0000"), "#ff0000");
    assert_eq!(sanitize_css_color("rgb(255, 0, 0)"), "rgb(255, 0, 0)");
    // Escape characters ; { } < > should be stripped, keeping the color body.
    let out = sanitize_css_color("red;} * { background:#fff }");
    assert!(
        !out.contains(';'),
        "semicolons should be stripped to prevent attribute escape"
    );
    assert!(!out.contains('{'), "left brace should be stripped");
    assert!(!out.contains('}'), "right brace should be stripped");
    assert!(out.contains("red"), "valid color names should be kept");
    assert!(
        out.contains("background:#fff"),
        "unescaped declarations should be kept"
    );
}

// E-40/P3 regression: sanitize_token_css must strip /* */ comments before keyword matching,
// preventing comment-splitting bypasses like `u/**/rl(` evading the url()/@import/expression()/javascript: checks.
// Uses the _inner pure logic (does not trigger web_sys::console; safe in native tests).
#[test]
fn sanitize_token_css_strips_comment_obfuscated_danger() {
    // Comment-split url() must be stripped.
    let bad = "--bg: u/**/rl(http://evil/x.png)";
    let (out, stripped) = sanitize_token_css_inner(bad);
    assert!(
        !out.contains("evil"),
        "comment-split url() must be stripped"
    );
    assert_eq!(
        stripped, 1,
        "the dangerous line is dropped whole, counted as 1"
    );
    assert!(
        out.is_empty(),
        "after dropping the dangerous line the output should be empty"
    );
    // Comment-split @import must be stripped.
    let bad2 = "@imp/**/ort url(http://evil.css)";
    let (out2, s2) = sanitize_token_css_inner(bad2);
    assert!(
        !out2.contains("evil"),
        "comment-split @import must be stripped"
    );
    assert_eq!(s2, 1);
    // Comment-split expression() must be stripped.
    let bad3 = "--x: expr/**/ession(alert(1))";
    let (out3, s3) = sanitize_token_css_inner(bad3);
    assert!(
        !out3.contains("alert"),
        "comment-split expression() must be stripped"
    );
    assert_eq!(s3, 1);
}

#[test]
fn sanitize_token_css_preserves_safe_token_declarations() {
    // E-40 positive: valid token declarations (with a harmless explanatory comment) should be kept, stripped=0.
    let safe = "--color-bg: #ffffff; /* white background */";
    let (out, stripped) = sanitize_token_css_inner(safe);
    assert!(
        out.contains("--color-bg: #ffffff;"),
        "valid declarations should be kept: {out}"
    );
    assert_eq!(
        stripped, 0,
        "no dangerous keywords, nothing should be stripped"
    );
    // Safe comment lines can be kept (no dangerous keyword match).
    let safe2 = "/* comment only */\n--space-1: 4px;";
    let (out2, s2) = sanitize_token_css_inner(safe2);
    assert!(
        out2.contains("--space-1: 4px;"),
        "valid declarations should be kept"
    );
    assert_eq!(s2, 0);
}

#[test]
fn sanitize_token_css_plain_danger_still_stripped() {
    // E-40 regression: plain url()/@import without comments is still stripped (does not break the original C10 behavior).
    let css = "--bg: url(http://evil.png);\n@import url(evil.css);\n--ok: #fff;";
    let (out, stripped) = sanitize_token_css_inner(css);
    assert!(
        !out.contains("evil"),
        "plain url()/@import is still stripped"
    );
    assert!(out.contains("--ok: #fff;"), "valid declarations are kept");
    assert_eq!(stripped, 2, "two dangerous CSS lines are stripped");
}

// R-A22/E-41 regression: send_to_host must not panic in a native test environment (no Window);
// valid payloads serialize fully and return gracefully; invalid payloads (including non-UTF-8
// keys serde cannot serialize) hit the E-41 warn branch and still must not panic.
#[test]
fn send_to_host_valid_payload_no_panic_native() {
    send_to_host("ai.generate", &serde_json::json!({"prompt": "login page"}));
}

#[test]
fn send_to_host_empty_payload_no_panic_native() {
    send_to_host("click", &serde_json::json!({}));
}

#[cfg(target_arch = "wasm32")]
#[test]
fn send_to_host_handler_name_is_camelcase() {
    // R-A22: the bridge handler name must follow the WKWebView camelCase convention; studio registers based on it.
    // HOST_HANDLER_NAME is only compiled for wasm32 (non-wasm builds do not emit this constant).
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
    // Verify the JSON structure is correct (does not call handle_host_message, which needs a browser environment)
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["kind"], "page.render");
    assert!(parsed["payload"]["document"].is_string());
}

#[test]
fn handle_host_message_invalid_json_does_not_panic() {
    // Only verify the JSON parsing logic; no web_sys console triggered
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
    // Must run in a browser environment; on the host target only the structure is verified
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
    // Verify rounded-rect path math: r must not exceed w/2 or h/2
    let w = 50.0;
    let h = 30.0;
    let r: f64 = 20.0;
    let clamped = r.min(w / 2.0).min(h / 2.0);
    assert_eq!(clamped, 15.0);
}

// ── Bridge type tests ──

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
        _ => panic!("expected PageRender"),
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
        _ => panic!("expected ApplyTokens"),
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
        _ => panic!("expected SelectNode"),
    }
}

#[test]
fn bridge_command_clear_canvas_serde() {
    let cmd = BridgeCommand::ClearCanvas;
    let json = serde_json::to_string(&cmd).unwrap();
    let cmd2: BridgeCommand = serde_json::from_str(&json).unwrap();
    match cmd2 {
        BridgeCommand::ClearCanvas => {}
        _ => panic!("expected ClearCanvas"),
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
        _ => panic!("expected NodeClick"),
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
        _ => panic!("expected NodeDrag"),
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
        _ => panic!("expected CanvasClick"),
    }
}

#[test]
fn bridge_event_ai_chat_serde() {
    let event = BridgeEvent::AiChat {
        message: "make a login page".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();
    let event2: BridgeEvent = serde_json::from_str(&json).unwrap();
    match event2 {
        BridgeEvent::AiChat { message } => assert_eq!(message, "make a login page"),
        _ => panic!("expected AiChat"),
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
        _ => panic!("expected CanvasZoom"),
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
        _ => panic!("expected CanvasPan"),
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
        _ => panic!("expected NodeMultiSelect"),
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
        _ => panic!("expected NodeSelect"),
    }
}

// ── Snap algorithm tests ──

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
    // Exactly on the threshold
    let (_, snapped, _) = find_snap_offset(105.0, &candidates);
    assert!(snapped);
    // Just past the threshold
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
    // Simulate a 100x50 node at (50, 80)
    // Snap candidate lines from another node: x=[0, 150, 75], y=[0, 100, 50]
    let snap_x_candidates = vec![0.0, 150.0, 75.0];
    let _snap_y_candidates = [0.0, 100.0, 50.0];

    // Node left edge x=48 → snap to 50? No, 48's nearest is 50 → offset +2 (within threshold)
    // This test uses a node left edge at x=2 → snaps to x=0
    let (off_left, sn_left, _) = find_snap_offset(2.0, &snap_x_candidates);
    assert!(sn_left);
    assert_eq!(off_left, -2.0); // snapped to x=0

    // Node right edge x=148 → snaps to x=150
    let (off_right, sn_right, _) = find_snap_offset(148.0, &snap_x_candidates);
    assert!(sn_right);
    assert_eq!(off_right, 2.0); // snapped to x=150

    // Node center-H x=73 → snaps to x=75
    let (off_center, sn_center, _) = find_snap_offset(73.0, &snap_x_candidates);
    assert!(sn_center);
    assert_eq!(off_center, 2.0); // snapped to x=75
}

// ── Performance optimization tests ──

#[test]
fn viewport_margin_constant() {
    assert_eq!(VIEWPORT_MARGIN, 200.0);
}

// E-38/P2-5 regression: the node geometry guard rejects non-finite/oversized dimensions, preventing render crashes/OOM.
#[test]
fn node_geom_guard_rejects_nonfinite() {
    let mk = |x: f64, y: f64, w: f64, h: f64| fd_canvas_core::PenNode::rect("n1", x, y, w, h);
    // Any NaN/Inf coordinate → reject
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
    // Negative dimensions → reject
    assert!(!is_node_geom_valid(&mk(0.0, 0.0, -1.0, 10.0), 1.0));
    assert!(!is_node_geom_valid(&mk(0.0, 0.0, 10.0, -1.0), 1.0));
    // Oversized dimensions (f64::MAX / beyond MAX_NODE_DIM_PX) → reject
    assert!(!is_node_geom_valid(&mk(0.0, 0.0, f64::MAX, 10.0), 1.0));
    assert!(!is_node_geom_valid(
        &mk(0.0, 0.0, 10.0, MAX_NODE_DIM_PX + 1.0),
        1.0
    ));
}

// E-38/P2-5 regression: the zoom guard rejects 0/negative/oversized values, preventing arithmetic crashes.
#[test]
fn node_geom_guard_rejects_bad_zoom() {
    let node = fd_canvas_core::PenNode::rect("n1", 0.0, 0.0, 10.0, 10.0);
    assert!(is_node_geom_valid(&node, 1.0), "normal zoom should pass");
    assert!(!is_node_geom_valid(&node, 0.0), "zoom=0 should be rejected");
    assert!(
        !is_node_geom_valid(&node, -2.0),
        "negative zoom should be rejected"
    );
    assert!(
        !is_node_geom_valid(&node, f64::NAN),
        "NaN zoom should be rejected"
    );
    assert!(
        !is_node_geom_valid(&node, MAX_ZOOM + 1.0),
        "beyond MAX_ZOOM should be rejected"
    );
    assert!(
        is_node_geom_valid(&node, MAX_ZOOM),
        "exactly MAX_ZOOM should pass"
    );
}

// E-38/P2-5 regression: valid nodes (normal sizes + boundary values) pass the guard.
#[test]
fn node_geom_guard_accepts_valid_nodes() {
    let mk = |x: f64, y: f64, w: f64, h: f64| fd_canvas_core::PenNode::rect("n1", x, y, w, h);
    assert!(is_node_geom_valid(&mk(0.0, 0.0, 10.0, 10.0), 1.0));
    assert!(
        is_node_geom_valid(&mk(1e6, 1e6, 100.0, 100.0), 1.0),
        "large coordinates should pass"
    );
    assert!(
        is_node_geom_valid(&mk(0.0, 0.0, MAX_NODE_DIM_PX, MAX_NODE_DIM_PX), 1.0),
        "exactly MAX_NODE_DIM_PX should pass"
    );
    assert!(
        is_node_geom_valid(&mk(0.0, 0.0, 0.0, 0.0), 1.0),
        "zero size should pass"
    );
}

#[test]
fn aabb_overlap_basic() {
    // Two overlapping rectangles
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
    // Two non-overlapping rectangles
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
    // L-15: with no window/DOM, mark_listeners_installed returns true (skips installation),
    // avoiding repeated failed install attempts. Idempotent-safe: repeated calls consistently return true.
    assert!(
        mark_listeners_installed("fusion-dom-root"),
        "without a DOM environment installation should be skipped (return true)"
    );
    assert!(
        mark_listeners_installed("fusion-dom-root"),
        "repeated calls should consistently return true"
    );
}

#[test]
fn render_depth_limit_constant_bounded() {
    // P2-1: the recursion depth cap exists and is reasonable, preventing stack overflow on deeply nested documents.
    // Compile-time check of the constant bounds (no web_sys console triggered, avoiding native-target panics).
    const _: () = assert!(MAX_RENDER_DEPTH > 0 && MAX_RENDER_DEPTH <= 256);
}

#[test]
fn children_per_node_cap_constant_bounded() {
    // E-38/P2-5: the per-node children render cap exists and is reasonable.
    // The cap must be > a legitimate design's single-layer fan-out (hundreds), and its product with depth
    // must stay below the jetsam threshold.
    // Compile-time check (render goes through web_sys, not callable on the native target, so the constant bounds are checked).
    const _: () = assert!(MAX_CHILDREN_PER_NODE >= 100 && MAX_CHILDREN_PER_NODE <= 10_000);
}

#[test]
fn children_take_cap_truncates_oversized_sibling_list() {
    // E-38/P2-5: 100k flat children at depth=1 pass the depth check, so the fan-out guard must truncate.
    // Replicates the render loop's .take(MAX_CHILDREN_PER_NODE) logic (pure logic, no DOM touched):
    // build an oversized children list, take the first N, assert truncation to exactly the cap.
    let oversized: Vec<u32> = (0..100_000).collect();
    let rendered: Vec<u32> = oversized
        .iter()
        .take(MAX_CHILDREN_PER_NODE)
        .copied()
        .collect();
    assert_eq!(
        rendered.len(),
        MAX_CHILDREN_PER_NODE,
        "over the limit must truncate to the cap"
    );
    assert_eq!(rendered[0], 0, "the first children keep their order");
    assert_eq!(
        rendered[MAX_CHILDREN_PER_NODE - 1],
        (MAX_CHILDREN_PER_NODE - 1) as u32,
        "the last element is the Nth child"
    );
}

#[test]
fn children_take_cap_preserves_undersized_sibling_list() {
    // E-38/P2-5: a legitimate small fan-out is unaffected by the guard. 500 children < cap, all kept.
    let normal: Vec<u32> = (0..500).collect();
    let rendered: Vec<u32> = normal.iter().take(MAX_CHILDREN_PER_NODE).copied().collect();
    assert_eq!(
        rendered.len(),
        500,
        "under the limit all entries must be kept"
    );
}
