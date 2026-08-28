//! ARCH-10：host-web Canvas 渲染管线。从 lib.rs 2246-2599 迁出，零逻辑改。

use wasm_bindgen::JsCast;

use crate::{fd_log_error, shell_lock, MAX_CHILDREN_PER_NODE, PenDocument};

/// 重置画布视图（缩放=1.0，平移=0,0）。
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
        // L-14：仅重置 transform，不整体覆盖 style。
        if let Ok(el) = container.dyn_into::<web_sys::HtmlElement>() {
            let _ = el
                .style()
                .set_property("transform", "scale(1) translate(0px,0px)");
            let _ = el.style().set_property("transform-origin", "0 0");
        }
    }
    web_sys::console::log_1(&"fd-host-web: canvas view reset".into());
}

/// 清空画布（Canvas + DOM）。
pub(crate) fn clear_canvas() {
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
pub(crate) fn render_plan_preview(doc_json: &str) {
    let doc = match PenDocument::from_json(doc_json) {
        Ok(d) => d,
        Err(e) => {
            fd_log_error(&format!("fd-host-web: PlanPreview 解析失败: {e}"));
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
            web_sys::console::warn_1(&"fd-host-web: render_plan_preview 创建 overlay 失败".into());
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
        web_sys::console::warn_1(&"fd-host-web: render_plan_preview overlay set_attribute 失败".into());
    }

    for page in &doc.pages {
        for node in &page.nodes {
            let el = match document.create_element("div") {
                Ok(e) => e,
                Err(_) => {
                    web_sys::console::warn_1(
                        &"fd-host-web: render_plan_preview 创建节点失败".into(),
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
            // C12：set_attribute 失败不 panic，warn 后跳过该节点样式注入。
            if el.set_attribute("style", &style).is_err() {
                web_sys::console::warn_1(
                    &"fd-host-web: render_plan_preview 节点 set_attribute 失败".into(),
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

/// 移除 Plan 预览叠加层。
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

/// 渲染 PenDocument JSON 到 Canvas。
pub(crate) fn render_page(doc_json: &str) {
    let doc = match PenDocument::from_json(doc_json) {
        Ok(d) => d,
        Err(e) => {
            fd_log_error(&format!("fd-host-web: PenDocument 解析失败: {e}"));
            return;
        }
    };

    // L-18：canvas 渲染路径也写 cached_doc_json，
    // 使 viewport_cull_update 两路径（DOM/canvas）都有最新文档，避免陈旧/None。
    {
        let mut guard = shell_lock();
        if let Some(inner) = guard.as_mut() {
            inner.cached_doc_json = Some(doc_json.to_string());
            // PERF-2：canvas 路径同步缓存已解析 PenDocument，与 DOM 路径对齐。
            inner.cached_doc = Some(doc.clone());
            // P-7/R-17：canvas 重渲同 DOM 重渲，清空选中记录。
            inner.selected_id = None;
            inner.selected_ids.clear();
        }
    }

    // 仅持锁取 canvas_id 后立即释放，避免大文档重绘期间阻塞消息处理（P1-2）。
    let canvas_id = {
        let guard = shell_lock();
        match guard.as_ref() {
            Some(i) => i.canvas_id.clone(),
            None => {
                fd_log_error("fd-host-web: WebShell 未初始化");
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
            fd_log_error("fd-host-web: 无法获取 2D 上下文");
            return;
        }
    };

    // 清空画布
    ctx.clear_rect(0.0, 0.0, canvas.width() as f64, canvas.height() as f64);

    // A-11：canvas 路径清空 DOM 容器，消除 render_dom 残留的 DOM 重影。
    // 双轨不一致：若先 render_dom（DOM 节点）再 render_page（canvas），
    // 旧 DOM 节点仍叠在 canvas 上方，与 canvas 画面重影。与 render_dom 清空对齐。
    if let Some(container) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("fusion-dom-root"))
    {
        container.set_inner_html("");
    }

    // 渲染每个页面
    for page in &doc.pages {
        render_page_to_canvas(page, &ctx);
    }
}

/// 递归渲染深度上限，防止恶意/异常深层嵌套文档栈溢出（P2-1）。
pub(crate) const MAX_RENDER_DEPTH: u32 = 64;

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
                    &format!("render_node: 嵌套深度超限 {MAX_RENDER_DEPTH}，跳过子树").into(),
                );
                return;
            }
            if node.children.len() > MAX_CHILDREN_PER_NODE {
                web_sys::console::warn_1(
                    &format!(
                        "render_node: 子节点数 {} 超 {} 上限，仅渲染前 {} 个",
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
