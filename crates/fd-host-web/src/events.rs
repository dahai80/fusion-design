//! ARCH-10：host-web 交互事件管线。从 lib.rs 499-1528 迁出，零逻辑改。
//!
//! 职责：
//! - rAF 节流（schedule_raf）
//! - 事件委托（click/mousedown delegate）
//! - canvas zoom/pan/marquee 监听器
//! - CSS 像素读写辅助（read_node_position/size/extract_css_px/replace/strip/read_css_prop_value）

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::{
    collect_snap_candidates, find_snap_offset, hide_snap_lines, node_selector, read_attr_f32,
    select_node, send_bridge_event, shell_lock, show_snap_lines, viewport_cull_update, BridgeEvent,
    DEFAULT_NODE_HEIGHT, DEFAULT_NODE_WIDTH, MIN_MARQUEE_SIZE,
};

// ── 拖拽全局态（ARCH-10 round-3 从 lib.rs 迁入，主消费方归位）──
// ARCH-10 round-3：3 个 thread_local + DragMoveClosure + cleanup_pending_drag 原集中
// 定义于 lib.rs god-file，被本模块（events.rs 拖拽 on_move/on_up 存储与 take 回收）
// 跨模块引用。主消费方在此——状态就近定义。lib.rs select_node/create_resize_handle
// 路径经反向 pub(crate) use 消费（见 lib.rs use events::{...}）。零逻辑改，仅迁位置。
// 可见性 pub(crate) 不变（lib.rs 仍要读）。

// R-1：拖拽/平移/框选/resize 的 on_move Closure 暂存（替代 forget 泄漏）。
// 旧实现 .forget() 导致每次拖拽泄漏一个 FnMut Closure，长会话线性内存增长。
// mousedown 存入，mouseup take() + remove_event_listener → Closure drop 回收内存。
// thread_local 规避 Send 约束（Closure<dyn FnMut> 非 Send，wasm 单线程安全）。
pub(crate) type DragMoveClosure = wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>;
thread_local! {
    pub(crate) static ACTIVE_DRAG_MOVE: std::cell::RefCell<Option<DragMoveClosure>> =
        const { std::cell::RefCell::new(None) };
    // R-1 backfill：拖拽/平移/框选/resize 的 on_up Closure 暂存。
    // 旧 .forget() 在 mouseup 未触发（鼠标拖拽中途离开窗口）时永久泄漏该 Closure。
    // 每次 mousedown 前调 cleanup_pending_drag() 回收上一轮残留的 on_up + on_move，
    // 消除"漏 mouseup 即泄漏"路径。on_up 触发时也 take + drop。
    pub(crate) static PENDING_DRAG_UP: std::cell::RefCell<Option<DragMoveClosure>> =
        const { std::cell::RefCell::new(None) };
    // R-1 backfill：resize handle 的 on_handle_mousedown Closure 暂存（每选中节点 8 个）。
    // 旧 .forget() 每次 select_node 切换选中泄漏 8 个 Closure（长会话线性增长）。
    // select_node 重建前 clear() 丢弃上一批 8 个。
    pub(crate) static RESIZE_HANDLES: std::cell::RefCell<Vec<DragMoveClosure>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

// R-1 backfill：拖拽开始前回收上一轮残留监听。
// 处理"mouseup 漏触发"场景——鼠标拖拽中途离开窗口，mouseup 永不到达，
// 旧 on_up（PENDING_DRAG_UP）+ on_move（ACTIVE_DRAG_MOVE）成为悬空监听 + 泄漏 Closure。
// 此处 remove_event_listener 后 drop Closure，避免下一轮拖拽叠加 + 线性内存增长。
// 失败仅 .ok()：监听器可能已被 on_up 触发时移除，重复移除无碍。
pub(crate) fn cleanup_pending_drag() {
    if let Some(w) = web_sys::window() {
        ACTIVE_DRAG_MOVE.with(|c| {
            if let Some(cl) = c.borrow_mut().take() {
                let mv_ref: &js_sys::Function = cl.as_ref().unchecked_ref();
                w.remove_event_listener_with_callback("mousemove", mv_ref)
                    .ok();
                drop(cl);
                web_sys::console::warn_1(
                    &"fd-host-web: R-1 回收上一轮残留 on_move（mouseup 漏触发）".into(),
                );
            }
        });
        PENDING_DRAG_UP.with(|c| {
            if let Some(cl) = c.borrow_mut().take() {
                let up_ref: &js_sys::Function = cl.as_ref().unchecked_ref();
                w.remove_event_listener_with_callback("mouseup", up_ref)
                    .ok();
                drop(cl);
                web_sys::console::warn_1(
                    &"fd-host-web: R-1 回收上一轮残留 on_up（mouseup 漏触发）".into(),
                );
            }
        });
    }
}

// R-1 余 forget 站点审计裁定回溯：
// 剩余 .forget() 站点为容器级委托监听器（setup_delegated_* / setup_* 的 click/mousedown/
// wheel/message）——mount 一次即应用生命周期常驻，经 mark_listeners_installed 幂等保护防重复
// attach。事件委托模式下非逐节点绑定，节点增删不新增监听器。常驻监听器 forget 是 web_sys 惯例，
// 不随会话长度增长，无线性内存泄漏。
// 拖拽热路径（on_move / on_up / resize handle）已全部转 stored-thread_local：
//   - on_move → ACTIVE_DRAG_MOVE（mouseup take 回收）
//   - on_up   → PENDING_DRAG_UP（mouseup take + 漏触发时 cleanup_pending_drag 回收）
//   - resize handle → RESIZE_HANDLES（select_node 重建前 clear 回收上一批 8 个）
// 若未来需 unmount 全量回收，可在此 thread_local 旁加 Vec<Closure> + unmount() drop——
// 当前无 unmount 调用方，stored-Vec 增复杂度不解决现存泄漏，按 Rule 2 不引入。

// ── 交互事件 ──

// ── requestAnimationFrame 节流 ──

/// 全局 rAF 句柄，避免同一帧多次调度。
pub(crate) static RAF_SCHEDULED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

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
pub(crate) fn setup_delegated_click_listener(container_id: &str) {
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
pub(crate) fn setup_delegated_mousedown_listener(container_id: &str) {
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
        // R-1 backfill：拖拽开始前回收上一轮残留 on_up/on_move（漏 mouseup 场景）。
        cleanup_pending_drag();
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
        let on_mouseup = Closure::wrap(Box::new(move |event: web_sys::Event| {
            let Some(w) = web_sys::window() else {
                return;
            };
            let move_ref: &js_sys::Function = move_js.unchecked_ref();
            w.remove_event_listener_with_callback("mousemove", move_ref)
                .ok();
            // R-1：移除监听后 take + drop on_mousemove Closure，回收线性内存。
            ACTIVE_DRAG_MOVE.with(|c| *c.borrow_mut() = None);
            // R-1 backfill：同步回收 on_up 自身（PENDING_DRAG_UP）。
            PENDING_DRAG_UP.with(|c| *c.borrow_mut() = None);
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
        }) as Box<dyn FnMut(web_sys::Event)>);
        window
            .add_event_listener_with_callback("mouseup", on_mouseup.as_ref().unchecked_ref())
            .ok();
        // R-1 backfill：on_up 托管到 PENDING_DRAG_UP（漏 mouseup 时 cleanup_pending_drag 回收）。
        PENDING_DRAG_UP.with(|c| *c.borrow_mut() = Some(on_mouseup));
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
pub(crate) fn read_node_position(el: &web_sys::Element) -> (f32, f32) {
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
pub(crate) fn read_node_size(el: &web_sys::Element) -> (f32, f32) {
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
pub(crate) fn update_node_position(el: &web_sys::Element, x: f32, y: f32) {
    let style = el.get_attribute("style").unwrap_or_default();
    let new_style = replace_css_prop(&style, "left", &format!("{}px", x));
    let new_style = replace_css_prop(&new_style, "top", &format!("{}px", y));
    el.set_attribute("style", &new_style).ok();
    // P-6：同步刷新几何缓存，吸附收集读缓存免 style 串解析。
    el.set_attribute("data-fd-x", &format!("{}", x)).ok();
    el.set_attribute("data-fd-y", &format!("{}", y)).ok();
}

/// 替换 CSS style 字符串中指定属性值，不存在则追加。
pub(crate) fn replace_css_prop(style: &str, prop: &str, value: &str) -> String {
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
pub(crate) fn strip_css_prop(style: &str, prop: &str) -> String {
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
pub(crate) fn read_css_prop_value(style: &str, prop: &str) -> Option<String> {
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
pub(crate) fn setup_canvas_click_listener(container_id: &str) {
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
pub(crate) fn setup_canvas_zoom_listener(container_id: &str) {
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
pub(crate) fn setup_canvas_pan_listener(container_id: &str) {
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
        // R-1 backfill：拖拽开始前回收上一轮残留 on_up/on_move（漏 mouseup 场景）。
        cleanup_pending_drag();
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
            send_bridge_event(BridgeEvent::CanvasPan { dx, dy });
        }) as Box<dyn FnMut(web_sys::Event)>);
        win.add_event_listener_with_callback("mouseup", on_up.as_ref().unchecked_ref())
            .ok();
        // R-1 backfill：on_up 托管到 PENDING_DRAG_UP（漏 mouseup 时 cleanup_pending_drag 回收）。
        PENDING_DRAG_UP.with(|c| *c.borrow_mut() = Some(on_up));
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
pub(crate) fn setup_marquee_listener(container_id: &str) {
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
        // R-1 backfill：拖拽开始前回收上一轮残留 on_up/on_move（漏 mouseup 场景）。
        cleanup_pending_drag();
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
        }) as Box<dyn FnMut(web_sys::Event)>);

        win.add_event_listener_with_callback("mouseup", on_up.as_ref().unchecked_ref())
            .ok();
        // R-1 backfill：on_up 托管到 PENDING_DRAG_UP（漏 mouseup 时 cleanup_pending_drag 回收）。
        PENDING_DRAG_UP.with(|c| *c.borrow_mut() = Some(on_up));
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
