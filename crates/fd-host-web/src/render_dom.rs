//! ARCH-10: host-web DOM rendering pipeline. Moved out of lib.rs lines 335-902 with zero logic changes.

// ── DOM rendering pipeline (performance-optimized) ──

use wasm_bindgen::JsCast;

use crate::{
    fd_log_error, mark_listeners_installed, sanitize_css_value, setup_canvas_click_listener,
    setup_canvas_pan_listener, setup_canvas_zoom_listener, setup_delegated_click_listener,
    setup_delegated_mousedown_listener, setup_marquee_listener, shell_lock, PenDocument,
    MAX_RENDER_DEPTH,
};

/// Viewport culling margin (px), slightly larger than the screen so edge nodes stay visible.
pub(crate) const VIEWPORT_MARGIN: f64 = 200.0;

// E-38/P2-5: hard caps on node dimensions / viewport coordinates, preventing a malicious .fusiondesign
// from using f32::MAX to trigger arithmetic overflow/NaN/Inf causing render crashes or OOM. Canvas
// coordinates up to 100k px per side are enough for any legitimate design.
pub(crate) const MAX_NODE_DIM_PX: f64 = 100_000.0;
const MAX_VIEWPORT_DIM_PX: f64 = 100_000.0;
pub(crate) const MAX_ZOOM: f64 = 1000.0;
// E-38/P2-5: render cap on the number of children per node. Depth has a MAX_RENDER_DEPTH=64 cap, but
// the per-layer children count is unbounded — 100k flat children at depth=1 pass the depth check, and
// render_dom creating 100k DOM nodes serially would OOM the WKWebView. canvas-core already guards the
// total with MAX_NODE_TOTAL=100k; this is a render-side per-node fan-out guard on top: legitimate
// designs have ≤ hundreds of siblings per layer, 2000 is plenty, and multiplied by depth 64 it stays
// far below the WKWebView jetsam threshold. Over the limit only the first N render + warn — fail visibly.
pub(crate) const MAX_CHILDREN_PER_NODE: usize = 2_000;

/// Render a PenDocument into DOM elements (rather than Canvas 2D).
/// Performance optimizations:
/// - DocumentFragment batch insertion, avoiding per-node layout thrashing
/// - Event delegation: a single container-level handler instead of per-node binding
/// - Viewport culling: only nodes inside the viewport render (incremental update on zoom/pan changes)
pub(crate) fn render_dom(doc_json: &str) {
    let doc = match PenDocument::from_json(doc_json) {
        Ok(d) => d,
        Err(e) => {
            fd_log_error(&format!("fd-host-web: DOM render PenDocument parse failed: {e}"));
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

    // Get or create the DOM container
    let container = match document.get_element_by_id("fusion-dom-root") {
        Some(c) => c,
        None => {
            // C12: create_element / body failure must not panic; warn and return early.
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
                    web_sys::console::warn_1(&"fd-host-web: render_dom failed to create container div".into());
                    return;
                }
            }
        }
    };

    // Clear the existing DOM nodes
    container.set_inner_html("");

    // Initialize zoom/pan state
    container.set_attribute("data-fd-zoom", "1.0").ok();
    container.set_attribute("data-fd-pan-x", "0.0").ok();
    container.set_attribute("data-fd-pan-y", "0.0").ok();

    // C11: no longer write the whole PenDocument JSON into a DOM attribute (unbounded, re-parsed every frame).
    // Instead cache it in SHELL global memory; viewport_cull_update reads it directly.
    {
        let mut guard = shell_lock();
        if let Some(inner) = guard.as_mut() {
            inner.cached_doc_json = Some(doc_json.to_string());
            // PERF-2: also cache the parsed PenDocument in sync, so viewport_cull_update skips re-parsing every frame.
            inner.cached_doc = Some(doc.clone());
            // P-7/R-17: a full re-render replaces the DOM, the previously selected element is gone; clear the
            // selection records to avoid dangling references.
            inner.selected_id = None;
            inner.selected_ids.clear();
        }
    }
    web_sys::console::log_1(&"fd-host-web: data-fd-doc attribute cache removed, switched to in-memory cache (C11)".into());

    // Batch-insert via a DocumentFragment
    let fragment = document.create_document_fragment();
    let mut node_count: u32 = 0;
    for page in &doc.pages {
        for node in &page.nodes {
            // Viewport culling: skip nodes entirely outside the viewport
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
        &format!("fd-host-web: DOM render done, {node_count} nodes (after viewport culling)").into(),
    );

    // Event delegation: container-level listeners install once per container (L-15).
    if !mark_listeners_installed("fusion-dom-root") {
        setup_delegated_click_listener("fusion-dom-root");
        setup_delegated_mousedown_listener("fusion-dom-root");
        setup_canvas_click_listener("fusion-dom-root");
        setup_canvas_zoom_listener("fusion-dom-root");
        setup_canvas_pan_listener("fusion-dom-root");
        setup_marquee_listener("fusion-dom-root");
        web_sys::console::log_1(&"fd-host-web: container-level listeners installed (per-container)".into());
    }
}

/// E-38/P2-5: node geometry guard (pure function, no DOM dependency, unit-testable).
/// Non-finite dimensions or dimensions beyond MAX_NODE_DIM_PX are invalid, preventing a malicious
/// .fusiondesign from using f32::MAX/NaN to trigger arithmetic overflow/Inf render crashes. Zoom is
/// likewise bounded to [>0, MAX_ZOOM].
pub(crate) fn is_node_geom_valid(node: &fd_canvas_core::PenNode, zoom: f64) -> bool {
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

/// Determine whether a node is inside the current viewport (accounting for zoom/pan).
fn is_node_in_viewport(node: &fd_canvas_core::PenNode, container: &web_sys::Element) -> bool {
    // E-38/P2-5: node dimension/coordinate guard. Non-finite nodes or nodes beyond MAX_NODE_DIM_PX are
    // treated as invisible, preventing a malicious .fusiondesign from using f32::MAX/NaN to trigger
    // arithmetic overflow/Inf render crashes.
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
    // E-38/P2-5: viewport dimension guard against abnormal inner_width/height (some embedded WebViews return 0/oversized values).
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

    // Node bounds in canvas coordinates
    let node_left = node.x * zoom + pan_x;
    let node_top = node.y * zoom + pan_y;
    let node_right = (node.x + node.w) * zoom + pan_x;
    let node_bottom = (node.y + node.h) * zoom + pan_y;
    // After the guard the coordinates must be finite; one more defensive check against unforeseen overflow.
    if !node_left.is_finite()
        || !node_top.is_finite()
        || !node_right.is_finite()
        || !node_bottom.is_finite()
    {
        return false;
    }

    // Viewport expanded by the margin
    let vp_left = -VIEWPORT_MARGIN;
    let vp_top = -VIEWPORT_MARGIN;
    let vp_right = vp_w + VIEWPORT_MARGIN;
    let vp_bottom = vp_h + VIEWPORT_MARGIN;

    // AABB overlap test
    node_right > vp_left && node_left < vp_right && node_bottom > vp_top && node_top < vp_bottom
}

/// Viewport-culled incremental rendering: called on zoom/pan changes; adds nodes newly entering the
/// viewport and removes nodes that left it.
pub(crate) fn viewport_cull_update() {
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

    // PERF-2: prefer the already-cached PenDocument (written by render_dom/render_page), avoiding re-parsing every frame.
    // If doc=None while json=Some, fall back to parsing once (safety net, should not trigger regularly — write sites set doc in sync).
    let doc = {
        let guard = shell_lock();
        // Take the doc cache and the json fallback within the same borrow scope to avoid a second borrow.
        let (cached, json_fallback) = match guard.as_ref() {
            Some(i) => (i.cached_doc.clone(), i.cached_doc_json.clone()),
            None => (None, None),
        };
        match cached {
            Some(d) => d,
            None => {
                let doc_json = match json_fallback {
                    Some(j) => j,
                    None => return,
                };
                drop(guard);
                match PenDocument::from_json(&doc_json) {
                    Ok(d) => d,
                    Err(e) => {
                        fd_log_error(&format!("fd-host-web: viewport_cull_update parse failed: {e}"));
                        return;
                    }
                }
            }
        }
    };

    // E-37/P2-5: collect the existing DOM node ids into a HashSet in one pass, replacing the old
    // implementation's one query_selector (O(N) global query) per visible node × N nodes = O(N²). Now O(N) overall.
    let existing_dom_ids = collect_existing_dom_node_ids(&container);

    // Collect all node ids in the document and their viewport state
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

    // Remove the DOM of nodes that left the viewport
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

    // Add nodes newly entering the viewport
    if !ids_to_add.is_empty() {
        // E-37/P2-5: ids_to_add → HashSet, dropping add_nodes_by_ids' contains from O(M) to O(1).
        let add_set: std::collections::HashSet<String> = ids_to_add.iter().cloned().collect();
        let fragment = document.create_document_fragment();
        for page in &doc.pages {
            add_nodes_by_ids(&page.nodes, &document, &add_set, &fragment);
        }
        container.append_child(&fragment).ok();
    }

    web_sys::console::log_1(
        &format!("fd-host-web: viewport cull update, added {} nodes", ids_to_add.len()).into(),
    );
}

/// E-37/P2-5: walk the container's direct children in one pass, collecting data-node-id → HashSet.
/// Replaces the old per-node query_selector O(N²) inside collect_visible_node_ids_inner.
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

/// Recursively collect node ids inside the viewport, diffing against the existing DOM id set to find additions.
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
            // E-37/P2-5: HashSet lookup is O(1), replacing the old query_selector O(N) global search.
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

/// Recursively find and render the nodes with the given ids.
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

/// Render a PenNode into a DOM element.
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

    // Base positioning
    let mut style = format!(
        "position:absolute;left:{}px;top:{}px;width:{}px;height:{}px;",
        node.x, node.y, node.w, node.h
    );

    // z-index and rotation
    if node.z_index != 0 {
        style.push_str(&format!("z-index:{};", node.z_index));
    }
    if node.rotation != 0.0 {
        style.push_str(&format!("transform:rotate({}deg);", node.rotation));
    }

    // Fill color
    if let Some(fill) = &node.style.fill {
        let fill = sanitize_css_value(fill, "transparent");
        style.push_str(&format!("background-color:{};", fill));
    }

    // Stroke
    if let Some(stroke) = &node.style.stroke {
        let width = node.style.stroke_width.unwrap_or(1.0);
        let stroke = sanitize_css_value(stroke, "transparent");
        style.push_str(&format!("border:{}px solid {};", width, stroke));
    }

    // Corner radius
    if let Some(radius) = node.style.radius {
        style.push_str(&format!("border-radius:{}px;", radius));
    }

    // Text
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

    // Flex layout
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

    // Design token CSS variable references
    for (key, value) in &node.style.design_token_refs {
        let css_var = key.replace('.', "-");
        let value = sanitize_css_value(value, "transparent");
        style.push_str(&format!("--{}:{};", css_var, value));
    }

    el.set_attribute("style", &style).ok()?;
    el.set_attribute("data-node-id", &node.id).ok()?;
    // P-6: cache geometry into data-* attributes so snap collection skips the querySelectorAll full
    // scan plus per-element style-string parsing.
    // Drag refreshes data-fd-x/y via update_node_position; mutate_node refreshes w/h when resizing.
    el.set_attribute("data-fd-x", &format!("{}", node.x)).ok();
    el.set_attribute("data-fd-y", &format!("{}", node.y)).ok();
    el.set_attribute("data-fd-w", &format!("{}", node.w)).ok();
    el.set_attribute("data-fd-h", &format!("{}", node.h)).ok();

    // Render children (children also use the event delegation mode). Depth cap prevents stack
    // overflow (P2-1); fan-out cap prevents OOM (E-38/P2-5).
    if depth < MAX_RENDER_DEPTH {
        let children = &node.children;
        if children.len() > MAX_CHILDREN_PER_NODE {
            web_sys::console::warn_1(
                &format!(
                    "render_node_to_dom: {} children exceeds the {} cap, rendering only the first {}",
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
            &format!("render_node_to_dom: nesting depth beyond {MAX_RENDER_DEPTH}, skipping subtree").into(),
        );
    }

    Some(el)
}

/// Convert a TrackSizing to a CSS grid track value.
pub(crate) fn track_to_css(track: &fd_canvas_core::TrackSizing) -> String {
    match track {
        fd_canvas_core::TrackSizing::Fixed(v) => format!("{}px", v),
        fd_canvas_core::TrackSizing::Flex(v) => format!("{}fr", v),
        fd_canvas_core::TrackSizing::Auto => "auto".to_string(),
        fd_canvas_core::TrackSizing::Percent(v) => format!("{}%", v),
    }
}
