//! ARCH-10：host-web DOM 渲染管线。从 lib.rs 335-902 迁出，零逻辑改。

// ── DOM 渲染管线（性能优化版）──

use wasm_bindgen::JsCast;

use crate::{
    fd_log_error, mark_listeners_installed, sanitize_css_value, setup_canvas_click_listener,
    setup_canvas_pan_listener, setup_canvas_zoom_listener, setup_delegated_click_listener,
    setup_delegated_mousedown_listener, setup_marquee_listener, shell_lock, PenDocument,
    MAX_RENDER_DEPTH,
};

/// 视口剔除边距（px），略大于屏幕确保边缘节点可见。
pub(crate) const VIEWPORT_MARGIN: f64 = 200.0;

// E-38/P2-5：节点尺寸/视口坐标硬上限，防恶意 .fusiondesign 用 f32::MAX
// 触发算术溢出/NaN/Inf 致渲染崩溃或 OOM。画布坐标单边 ≤ 100k px 足够任何合法设计稿。
pub(crate) const MAX_NODE_DIM_PX: f64 = 100_000.0;
const MAX_VIEWPORT_DIM_PX: f64 = 100_000.0;
pub(crate) const MAX_ZOOM: f64 = 1000.0;
// E-38/P2-5：单节点子节点数渲染上限。深度有 MAX_RENDER_DEPTH=64 上限，但
// 单层 children 数量无界——10 万扁平子节点深度=1 过深度检查，render_dom 串行
// 创建 10 万 DOM 致 WKWebView OOM。canvas-core 已有 MAX_NODE_TOTAL=100k 总数
// 护栏，此为渲染侧每节点扇出补充护栏：合法设计稿单层兄弟 ≤ 数百，2000 足够且
// 与 64 深度乘积远低于 WKWebView jetsam 阈值。超限只渲染前 N 个 + warn，fail visibly。
pub(crate) const MAX_CHILDREN_PER_NODE: usize = 2_000;

/// 将 PenDocument 渲染为 DOM 元素（而非 Canvas 2D）。
/// 性能优化：
/// - DocumentFragment 批量插入，避免逐节点 layout thrashing
/// - 事件委托：容器级别单处理器，替代逐节点绑定
/// - 视口剔除：仅渲染视口内节点（zoom/pan 变化时增量更新）
pub(crate) fn render_dom(doc_json: &str) {
    let doc = match PenDocument::from_json(doc_json) {
        Ok(d) => d,
        Err(e) => {
            fd_log_error(&format!("fd-host-web: DOM 渲染 PenDocument 解析失败: {e}"));
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
            // PERF-2：同步缓存已解析的 PenDocument，viewport_cull_update 免每帧重解析。
            inner.cached_doc = Some(doc.clone());
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

    // PERF-2：优先读已缓存的 PenDocument（render_dom/render_page 写入），免每帧重解析。
    // doc=None 而 json=Some 时回退解析一次（安全网，不应常态触发——写站同步设 doc）。
    let doc = {
        let guard = shell_lock();
        // 同一借用作用域内同时取 doc 缓存与 json 回退源，避免二次借用。
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
                        fd_log_error(&format!("fd-host-web: viewport_cull_update 解析失败: {e}"));
                        return;
                    }
                }
            }
        }
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
pub(crate) fn track_to_css(track: &fd_canvas_core::TrackSizing) -> String {
    match track {
        fd_canvas_core::TrackSizing::Fixed(v) => format!("{}px", v),
        fd_canvas_core::TrackSizing::Flex(v) => format!("{}fr", v),
        fd_canvas_core::TrackSizing::Auto => "auto".to_string(),
        fd_canvas_core::TrackSizing::Percent(v) => format!("{}%", v),
    }
}
