//! ARCH-10: host-web interaction event pipeline. Moved out of lib.rs lines 499-1528 with zero logic changes.
//!
//! Responsibilities:
//! - rAF throttling (schedule_raf)
//! - Event delegation (click/mousedown delegate)
//! - canvas zoom/pan/marquee listeners
//! - CSS pixel read/write helpers (read_node_position/size/extract_css_px/replace/strip/read_css_prop_value)

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::{
    collect_snap_candidates, find_snap_offset, hide_snap_lines, node_selector, read_attr_f32,
    select_node, send_bridge_event, shell_lock, show_snap_lines, viewport_cull_update, BridgeEvent,
    DEFAULT_NODE_HEIGHT, DEFAULT_NODE_WIDTH, MIN_MARQUEE_SIZE,
};

// ── Drag global state (ARCH-10 round-3, moved from lib.rs to sit next to its main consumer) ──
// ARCH-10 round-3: the 3 thread_locals + DragMoveClosure + cleanup_pending_drag were originally
// defined in the lib.rs god-file and referenced cross-module by this module (events.rs drag
// on_move/on_up storage and take-reclaim). The main consumer lives here — the state is defined
// close to it. The lib.rs select_node/create_resize_handle path consumes them via a reverse
// pub(crate) use (see lib.rs use events::{...}). Zero logic changes; relocation only.
// Visibility stays pub(crate) (lib.rs still reads them).

// R-1: stash for the drag/pan/marquee/resize on_move Closure (replaces the forget leak).
// The old .forget() leaked one FnMut Closure per drag, growing linear memory over long sessions.
// Stored on mousedown; on mouseup take() + remove_event_listener → Closure drop reclaims memory.
// thread_local sidesteps the Send constraint (Closure<dyn FnMut> is not Send; safe under wasm's single thread).
pub(crate) type DragMoveClosure = wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>;
thread_local! {
    pub(crate) static ACTIVE_DRAG_MOVE: std::cell::RefCell<Option<DragMoveClosure>> =
        const { std::cell::RefCell::new(None) };
    // R-1 backfill: stash for the drag/pan/marquee/resize on_up Closure.
    // The old .forget() leaked that Closure permanently when mouseup never fired (mouse dragged out of the window mid-drag).
    // cleanup_pending_drag() is called before every mousedown to reclaim the previous round's leftover on_up + on_move,
    // eliminating the "missed mouseup means leak" path. Also take + drop when on_up fires.
    pub(crate) static PENDING_DRAG_UP: std::cell::RefCell<Option<DragMoveClosure>> =
        const { std::cell::RefCell::new(None) };
    // R-1 backfill: stash for resize-handle on_handle_mousedown Closures (8 per selected node).
    // The old .forget() leaked 8 Closures per select_node selection change (linear growth over long sessions).
    // select_node clear()s the previous batch of 8 before rebuilding.
    pub(crate) static RESIZE_HANDLES: std::cell::RefCell<Vec<DragMoveClosure>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

// R-1 backfill: reclaim the previous round's leftover listeners before a drag starts.
// Handles the "missed mouseup" case — the mouse leaves the window mid-drag, mouseup never arrives,
// and the old on_up (PENDING_DRAG_UP) + on_move (ACTIVE_DRAG_MOVE) become dangling listeners + leaked Closures.
// Here remove_event_listener is followed by dropping the Closure, avoiding listener stacking on the next drag + linear memory growth.
// Failures are just .ok(): the listener may already have been removed when on_up fired; removing twice is harmless.
pub(crate) fn cleanup_pending_drag() {
    if let Some(w) = web_sys::window() {
        ACTIVE_DRAG_MOVE.with(|c| {
            if let Some(cl) = c.borrow_mut().take() {
                let mv_ref: &js_sys::Function = cl.as_ref().unchecked_ref();
                w.remove_event_listener_with_callback("mousemove", mv_ref)
                    .ok();
                drop(cl);
                web_sys::console::warn_1(
                    &"fd-host-web: R-1 reclaimed leftover on_move from previous round (missed mouseup)".into(),
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
                    &"fd-host-web: R-1 reclaimed leftover on_up from previous round (missed mouseup)".into(),
                );
            }
        });
    }
}

// R-1 audit verdict on the remaining forget sites:
// The remaining .forget() sites are container-level delegated listeners (setup_delegated_* / setup_*
// click/mousedown/wheel/message) — attached once on mount and resident for the app's lifetime, with
// mark_listeners_installed idempotence guarding against duplicate attach. Under event delegation there
// is no per-node binding, so adding/removing nodes adds no listeners. forgetting resident listeners is
// the web_sys convention; it does not grow with session length — no linear memory leak.
// The drag hot paths (on_move / on_up / resize handle) have all moved to stored-thread_local:
//   - on_move → ACTIVE_DRAG_MOVE (take-reclaimed on mouseup)
//   - on_up   → PENDING_DRAG_UP (take on mouseup + cleanup_pending_drag when mouseup is missed)
//   - resize handle → RESIZE_HANDLES (select_node clear()s the previous batch of 8 before rebuilding)
// If a full unmount reclaim is needed in the future, a Vec<Closure> + unmount() drop could be added
// alongside these thread_locals — but there is no unmount caller today, a stored Vec adds complexity
// without solving any existing leak, and per Rule 2 it is not introduced.

// ── Interaction events ──

// ── requestAnimationFrame throttling ──

/// Global rAF handle, avoiding multiple schedules within the same frame.
pub(crate) static RAF_SCHEDULED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Run a callback throttled via requestAnimationFrame, at most once per frame.
/// Fix P0-3: when rAF registration fails or the callback panics, the flag used to get stuck true forever → rendering stalls.
/// - window unavailable: reset the flag immediately, allowing a retry next time.
/// - rAF registration failure: reset immediately.
/// - Inside the callback, reset the flag before executing; R-16: catch_unwind as a safety net so a
///   callback panic lands in console.error and resets the flag, instead of propagating into a wasm
///   trap that permanently stalls the rAF loop.
fn schedule_raf<F>(callback: F)
where
    F: FnOnce() + 'static,
{
    let already_scheduled = RAF_SCHEDULED.swap(true, std::sync::atomic::Ordering::SeqCst);
    if already_scheduled {
        return;
    }
    let cb = Closure::once(Box::new(move || {
        // Reset the flag first, then run the callback; even a panicking callback cannot block forever.
        RAF_SCHEDULED.store(false, std::sync::atomic::Ordering::SeqCst);
        // R-16: catch_unwind safety net — a callback panic does not propagate into a trap that stalls rAF.
        // web_sys captures are not UnwindSafe, so wrap in AssertUnwindSafe (no locks held across the unwind inside the callback).
        let f = std::panic::AssertUnwindSafe(callback);
        if let Err(e) = std::panic::catch_unwind(f) {
            let msg = if let Some(s) = e.downcast_ref::<&'static str>() {
                format!("schedule_raf callback panic: {s}")
            } else if let Some(s) = e.downcast_ref::<String>() {
                format!("schedule_raf callback panic: {s}")
            } else {
                "schedule_raf callback panic (non-string payload)".to_string()
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
        // Registration failed: reset the flag, otherwise every later schedule_raf is silently dropped → deadlock.
        RAF_SCHEDULED.store(false, std::sync::atomic::Ordering::SeqCst);
        web_sys::console::warn_1(
            &"schedule_raf: request_animation_frame registration failed, flag reset".into(),
        );
        return;
    }
    cb.forget();
}

// ── Event delegation ──

/// Container-level click event delegation: bubble up from the event target looking for data-node-id.
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

        // Bubble up from the target looking for data-node-id
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

/// Container-level mousedown event delegation: drag initiation.
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
        // R-1 backfill: reclaim the previous round's leftover on_up/on_move before starting a drag (missed mouseup case).
        cleanup_pending_drag();
        let mouse = match event.dyn_ref::<web_sys::MouseEvent>() {
            Some(m) => m,
            None => return,
        };
        // Bubble up from the target looking for data-node-id
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

        // Look up the DOM element
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

            // Use rAF throttling for DOM updates
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
        // R-1: store on_mousemove in a thread_local, take-reclaimed on mouseup (replaces the forget leak).
        ACTIVE_DRAG_MOVE.with(|c| *c.borrow_mut() = Some(on_mousemove));
        let on_mouseup = Closure::wrap(Box::new(move |event: web_sys::Event| {
            let Some(w) = web_sys::window() else {
                return;
            };
            let move_ref: &js_sys::Function = move_js.unchecked_ref();
            w.remove_event_listener_with_callback("mousemove", move_ref)
                .ok();
            // R-1: after removing the listener, take + drop the on_mousemove Closure to reclaim linear memory.
            ACTIVE_DRAG_MOVE.with(|c| *c.borrow_mut() = None);
            // R-1 backfill: also reclaim on_up itself (PENDING_DRAG_UP).
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
        // R-1 backfill: on_up is stashed in PENDING_DRAG_UP (cleanup_pending_drag reclaims it if mouseup is missed).
        PENDING_DRAG_UP.with(|c| *c.borrow_mut() = Some(on_mouseup));
        // R-1: on_mousemove is stashed in SHELL (take-reclaimed on mouseup); no more forget.

        event.stop_propagation();
        event.prevent_default();
    }) as Box<dyn FnMut(web_sys::Event)>);
    container
        .add_event_listener_with_callback("mousedown", on_mousedown.as_ref().unchecked_ref())
        .ok();
    on_mousedown.forget();
    web_sys::console::log_1(&"fd-host-web: delegated mousedown listener installed".into());
}

/// Bind click / drag / resize events to a node DOM element (per-node mode; not used under event delegation).
/// Read the left/top position from the DOM element's style.
pub(crate) fn read_node_position(el: &web_sys::Element) -> (f32, f32) {
    // P-6: prefer the data-fd-x/y cached attributes, falling back to style-string parsing (compatible with old nodes / cache-less render paths).
    let x = read_attr_f32(el, "data-fd-x").unwrap_or_else(|| {
        extract_css_px(&el.get_attribute("style").unwrap_or_default(), "left").unwrap_or(0.0)
    });
    let y = read_attr_f32(el, "data-fd-y").unwrap_or_else(|| {
        extract_css_px(&el.get_attribute("style").unwrap_or_default(), "top").unwrap_or(0.0)
    });
    (x, y)
}

/// Read the width/height from the DOM element's style.
pub(crate) fn read_node_size(el: &web_sys::Element) -> (f32, f32) {
    // P-6: prefer the data-fd-w/h cached attributes, falling back to style-string parsing.
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

/// Extract the px value of a given property from a CSS style string.
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

/// Update a DOM element's left/top styles (live drag preview).
pub(crate) fn update_node_position(el: &web_sys::Element, x: f32, y: f32) {
    let style = el.get_attribute("style").unwrap_or_default();
    let new_style = replace_css_prop(&style, "left", &format!("{}px", x));
    let new_style = replace_css_prop(&new_style, "top", &format!("{}px", y));
    el.set_attribute("style", &new_style).ok();
    // P-6: refresh the geometry cache in sync, so snap collection reads the cache without style-string parsing.
    el.set_attribute("data-fd-x", &format!("{}", x)).ok();
    el.set_attribute("data-fd-y", &format!("{}", y)).ok();
}

/// Replace a property value in a CSS style string, appending it if absent.
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

// L3: remove a given property from a CSS style string (used to drop display when restoring visibility so CSS defaults apply).
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

// L3: read the raw value of a given property from a CSS style string (also for non-numeric values, e.g. display:flex).
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

/// Set up the click event listener for the canvas blank area.
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

/// Set up the canvas wheel zoom event listener.
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

/// Set up the canvas middle-button / Space+drag pan event listener.
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
        // R-1 backfill: reclaim the previous round's leftover on_up/on_move before starting a drag (missed mouseup case).
        cleanup_pending_drag();
        let mouse = match event.dyn_ref::<web_sys::MouseEvent>() {
            Some(m) => m,
            None => return,
        };
        // Middle button (button=1) or Space+left button (button=0 + shiftKey)
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
            send_bridge_event(BridgeEvent::CanvasPan { dx, dy });
        }) as Box<dyn FnMut(web_sys::Event)>);
        win.add_event_listener_with_callback("mouseup", on_up.as_ref().unchecked_ref())
            .ok();
        // R-1 backfill: on_up is stashed in PENDING_DRAG_UP (cleanup_pending_drag reclaims it if mouseup is missed).
        PENDING_DRAG_UP.with(|c| *c.borrow_mut() = Some(on_up));
        // R-1: on_move is stashed in SHELL (take-reclaimed on mouseup); no more forget.

        event.prevent_default();
        event.stop_propagation();
    }) as Box<dyn FnMut(web_sys::Event)>);
    container
        .add_event_listener_with_callback("mousedown", on_mousedown.as_ref().unchecked_ref())
        .ok();
    on_mousedown.forget();
    web_sys::console::log_1(&"fd-host-web: canvas pan listener installed".into());
}

/// Marquee selection listener: press and drag on the canvas blank area → draw a translucent rectangle →
/// collect the nodes inside the rectangle on release.
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
        // R-1 backfill: reclaim the previous round's leftover on_up/on_move before starting a drag (missed mouseup case).
        cleanup_pending_drag();
        let mouse = match event.dyn_ref::<web_sys::MouseEvent>() {
            Some(m) => m,
            None => return,
        };
        // Only respond to left button without modifier keys (Shift = pan, already taken)
        if mouse.button() != 0 || mouse.shift_key() {
            return;
        }
        // If the click landed on a node element, skip marquee selection (node drag already handles it)
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

        // Create the marquee DOM element
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(doc) = window.document() else {
            return;
        };
        let marquee_el = match doc.create_element("div") {
            Ok(el) => el,
            Err(_) => {
                web_sys::console::warn_1(
                    &"fd-host-web: marquee failed to create marquee element".into(),
                );
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

            // Remove the marquee DOM
            if let Some(doc) = w.document() {
                if let Some(m) = doc.get_element_by_id("fd-marquee") {
                    m.remove();
                }
            }

            // Collect nodes inside the marquee rectangle
            let mm = event.dyn_ref::<web_sys::MouseEvent>();
            let (end_x, end_y) = match mm {
                Some(m) => (m.client_x() as f32, m.client_y() as f32),
                None => (start_x, start_y),
            };
            let rect_left = start_x.min(end_x);
            let rect_top = start_y.min(end_y);
            let rect_right = start_x.max(end_x);
            let rect_bottom = start_y.max(end_y);

            // At least 5px for a valid marquee selection
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
        // R-1 backfill: on_up is stashed in PENDING_DRAG_UP (cleanup_pending_drag reclaims it if mouseup is missed).
        PENDING_DRAG_UP.with(|c| *c.borrow_mut() = Some(on_up));
        // R-1: on_move is stashed in SHELL (take-reclaimed on mouseup); no more forget.
    }) as Box<dyn FnMut(web_sys::Event)>);

    container
        .add_event_listener_with_callback("mousedown", on_mousedown.as_ref().unchecked_ref())
        .ok();
    on_mousedown.forget();
    web_sys::console::log_1(&"fd-host-web: marquee listener installed".into());
}

/// Collect all data-node-id elements within the rectangle.
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
    // L2: recurse through the subtree so marquee selection can hit nested nodes (previously only one level was checked).
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
                    // Rectangle overlap check
                    if el_left < right && el_right > left && el_top < bottom && el_bottom > top {
                        out.push(node_id);
                    }
                }
                // Recurse into the subtree
                let child_node: &web_sys::Node = el.unchecked_ref();
                collect_nodes_in_rect_inner(child_node, left, top, right, bottom, out);
            }
        }
    }
}

/// Canvas zoom visual feedback: update CSS transform scale + translate.
/// Performance optimization: rAF-throttled DOM updates + viewport-culled incremental rendering.
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

    // rAF throttling: avoid multiple DOM updates per frame
    schedule_raf(move || {
        // Re-look-up the container (references cannot be held across the rAF callback)
        let window = match web_sys::window() {
            Some(w) => w,
            None => return,
        };
        let document = match window.document() {
            Some(d) => d,
            None => return,
        };
        if let Some(container) = document.get_element_by_id("fusion-dom-root") {
            // L-14: set only the transform-related properties, not a whole-style override (preserves the container's other CSS).
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
        // R-16: viewport cull after zoom — call synchronously, do not nest schedule_raf.
        // The old implementation nested rAF, pushing the cull to the next frame while reusing the same
        // RAF_SCHEDULED flag, racing the transform frame against the cull frame. Within one frame the
        // transform is set first, then the cull runs — no nesting.
        viewport_cull_update();
    });
}

/// Canvas pan visual feedback: update CSS transform translate.
/// Performance optimization: rAF-throttled DOM updates + viewport-culled incremental rendering.
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

    // rAF throttling: avoid multiple DOM updates per frame
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
            // L-14: set only the transform-related properties, not a whole-style override.
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
        // R-16: viewport cull after pan — call synchronously, do not nest schedule_raf (same as above).
        viewport_cull_update();
    });
}

/// Multi-select a node: toggle the given node's selection state without affecting other selected nodes.
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
            // L4: on deselect only remove the box-shadow we injected; do not touch the user's outline.
            let style = el.get_attribute("style").unwrap_or_default();
            let clean = strip_css_prop(&style, "box-shadow");
            el.set_attribute("style", &clean).ok();
            el.remove_attribute("data-fd-selected").ok();
            // R-17: remove from the multi-select set in sync; selected_id consistency handled below.
            if let Some(inner) = shell_lock().as_mut() {
                inner.selected_ids.remove(node_id);
                if inner.selected_id.as_deref() == Some(node_id) {
                    inner.selected_id = inner.selected_ids.iter().next().cloned();
                }
            }
        } else {
            el.set_attribute("data-fd-selected", "true").ok();
            // L4: use box-shadow for the selection highlight, keeping the node's own outline from being overridden.
            let style = el.get_attribute("style").unwrap_or_default();
            el.set_attribute("style", &format!("{};box-shadow:0 0 0 2px #007AFF;", style))
                .ok();
            // R-17: add to the multi-select set. When no primary selection exists, set it as the primary one (handles anchor).
            if let Some(inner) = shell_lock().as_mut() {
                inner.selected_ids.insert(node_id.to_string());
                if inner.selected_id.is_none() {
                    inner.selected_id = Some(node_id.to_string());
                }
            }
        }
    }
}
