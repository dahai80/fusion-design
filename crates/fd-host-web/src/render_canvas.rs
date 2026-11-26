//! ARCH-10: host-web Canvas rendering pipeline. Moved out of lib.rs lines 2246-2599 with zero logic changes.

use wasm_bindgen::JsCast;

use crate::{fd_log_error, shell_lock, PenDocument, MAX_CHILDREN_PER_NODE};

/// Reset the canvas view (zoom=1.0, pan=0,0).
pub(crate) fn reset_canvas_view() {
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
        // L-14: reset only the transform, not a whole-style override.
        if let Ok(el) = container.dyn_into::<web_sys::HtmlElement>() {
            let _ = el
                .style()
                .set_property("transform", "scale(1) translate(0px,0px)");
            let _ = el.style().set_property("transform-origin", "0 0");
        }
    }
    web_sys::console::log_1(&"fd-host-web: canvas view reset".into());
}

/// Clear the canvas (Canvas + DOM).
pub(crate) fn clear_canvas() {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };

    // Clear the Canvas
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

    // Clear the DOM container
    if let Some(container) = document.get_element_by_id("fusion-dom-root") {
        container.set_inner_html("");
    }
}

/// Plan preview: render PenDocument nodes as a dashed overlay layer.
pub(crate) fn render_plan_preview(doc_json: &str) {
    let doc = match PenDocument::from_json(doc_json) {
        Ok(d) => d,
        Err(e) => {
            fd_log_error(&format!("fd-host-web: PlanPreview parse failed: {e}"));
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
            web_sys::console::warn_1(
                &"fd-host-web: render_plan_preview failed to create overlay".into(),
            );
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
        web_sys::console::warn_1(&"fd-host-web: render_plan_preview overlay set_attribute failed".into());
    }

    for page in &doc.pages {
        for node in &page.nodes {
            let el = match document.create_element("div") {
                Ok(e) => e,
                Err(_) => {
                    web_sys::console::warn_1(
                        &"fd-host-web: render_plan_preview failed to create node".into(),
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
            // C12: set_attribute failure must not panic; warn and skip this node's style injection.
            if el.set_attribute("style", &style).is_err() {
                web_sys::console::warn_1(
                    &"fd-host-web: render_plan_preview node set_attribute failed".into(),
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

/// Remove the Plan preview overlay layer.
pub(crate) fn remove_plan_preview() {
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

/// Render PenDocument JSON onto the Canvas.
pub(crate) fn render_page(doc_json: &str) {
    let doc = match PenDocument::from_json(doc_json) {
        Ok(d) => d,
        Err(e) => {
            fd_log_error(&format!("fd-host-web: PenDocument parse failed: {e}"));
            return;
        }
    };

    // L-18: the canvas rendering path also writes cached_doc_json,
    // so viewport_cull_update has the latest document on both paths (DOM/canvas), avoiding stale/None.
    {
        let mut guard = shell_lock();
        if let Some(inner) = guard.as_mut() {
            inner.cached_doc_json = Some(doc_json.to_string());
            // PERF-2: the canvas path caches the parsed PenDocument in sync, aligned with the DOM path.
            inner.cached_doc = Some(doc.clone());
            // P-7/R-17: a canvas re-render behaves like a DOM re-render; clear the selection records.
            inner.selected_id = None;
            inner.selected_ids.clear();
        }
    }

    // Hold the lock only to fetch canvas_id, releasing immediately, avoiding blocking message
    // processing during large-document redraws (P1-2).
    let canvas_id = {
        let guard = shell_lock();
        match guard.as_ref() {
            Some(i) => i.canvas_id.clone(),
            None => {
                fd_log_error("fd-host-web: WebShell not initialized");
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
            fd_log_error("fd-host-web: failed to acquire 2D context");
            return;
        }
    };

    // Clear the canvas
    ctx.clear_rect(0.0, 0.0, canvas.width() as f64, canvas.height() as f64);

    // A-11: the canvas path also clears the DOM container, removing DOM ghosts left by render_dom.
    // Dual-track inconsistency: if render_dom (DOM nodes) ran first and then render_page (canvas),
    // the old DOM nodes would still sit above the canvas, ghosting against the canvas picture. Aligned with render_dom's clearing.
    if let Some(container) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("fusion-dom-root"))
    {
        container.set_inner_html("");
    }

    // Render each page
    for page in &doc.pages {
        render_page_to_canvas(page, &ctx);
    }
}

/// Recursion depth cap for rendering, preventing stack overflow on malicious/abnormally deeply nested documents (P2-1).
pub(crate) const MAX_RENDER_DEPTH: u32 = 64;

/// Render a single page onto Canvas 2D.
fn render_page_to_canvas(page: &fd_canvas_core::Page, ctx: &web_sys::CanvasRenderingContext2d) {
    for node in &page.nodes {
        render_node(node, ctx, 0);
    }
}

/// Render a node recursively. depth limits nesting depth; beyond MAX_RENDER_DEPTH the subtree is skipped to prevent stack overflow (P2-1).
fn render_node(
    node: &fd_canvas_core::PenNode,
    ctx: &web_sys::CanvasRenderingContext2d,
    depth: u32,
) {
    // Set the fill color
    if let Some(fill) = &node.style.fill {
        ctx.set_fill_style_str(fill);
    }

    // Set the stroke
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
                    &format!(
                        "render_node: nesting depth beyond {MAX_RENDER_DEPTH}, skipping subtree"
                    )
                    .into(),
                );
                return;
            }
            if node.children.len() > MAX_CHILDREN_PER_NODE {
                web_sys::console::warn_1(
                    &format!(
                        "render_node: {} children exceeds the {} cap, rendering only the first {}",
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

/// Draw a rounded rectangle.
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

/// Get the canvas element.
fn get_canvas(canvas_id: &str) -> Option<web_sys::HtmlCanvasElement> {
    let window = web_sys::window()?;
    let document = window.document()?;
    document
        .get_element_by_id(canvas_id)?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .ok()
}
