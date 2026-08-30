// ARCH-10 r4：HTML 渲染从 lib.rs 拆出。render_page_html/node_to_html/format_style_inline/
// flex_css/grid_css。调 util（escape_attr/sanitize_css_color/MAX_CODEGEN_DEPTH）。零行为变更。

use fd_canvas_core::{LayoutMode, NodeKind, Page, PenNode};

use crate::util::{escape_attr, escape_html, MAX_CODEGEN_DEPTH};

pub(super) fn render_page_html(page: &Page) -> String {
    let mut out = format!(
        "<section data-page=\"{}\" style=\"width:{}px;height:{}px;position:relative;\">\n",
        page.id, page.width as i32, page.height as i32
    );
    for node in &page.nodes {
        out.push_str(&node_to_html(node));
    }
    out.push_str("</section>\n");
    out
}

pub(super) fn node_to_html(node: &PenNode) -> String {
    node_to_html_inner(node, 0)
}

fn node_to_html_inner(node: &PenNode, depth: usize) -> String {
    if depth > MAX_CODEGEN_DEPTH {
        return "<!-- codegen depth limit reached -->\n".to_string();
    }
    let style = format_style_inline(node);
    // 子节点对任意 kind 均可能存在（parse-html 把带子节点的 <div> 映射为 Rect），
    // 统一收集后在各分支注入，避免叶子节点静默丢弃嵌套结构（#7）。
    let children: String = node
        .children
        .iter()
        .map(|c| node_to_html_inner(c, depth + 1))
        .collect();
    match node.kind {
        NodeKind::Rect | NodeKind::Circle => {
            let tag = "div";
            let extra = if matches!(node.kind, NodeKind::Circle) {
                "border-radius:50%;"
            } else {
                ""
            };
            if children.is_empty() {
                format!(
                    "<{tag} data-id=\"{}\" style=\"{}{}\"></{tag}>\n",
                    escape_attr(&node.id),
                    style,
                    extra
                )
            } else {
                format!(
                    "<{tag} data-id=\"{}\" style=\"{}{}\">{}</{tag}>\n",
                    escape_attr(&node.id),
                    style,
                    extra,
                    children
                )
            }
        }
        NodeKind::Text => format!(
            "<div data-id=\"{}\" style=\"{}\">{}{}</div>\n",
            escape_attr(&node.id),
            style,
            escape_html(node.text.as_deref().unwrap_or("")),
            children
        ),
        NodeKind::Image => {
            if children.is_empty() {
                format!(
                    "<img data-id=\"{}\" style=\"{}\" alt=\"\"/>\n",
                    escape_attr(&node.id),
                    style
                )
            } else {
                // <img> 为 void 元素；有子节点时外裹 div 承载嵌套结构
                format!(
                    "<div data-id=\"{}\" style=\"{}\"><img style=\"{}\" alt=\"\"/>{}</div>\n",
                    escape_attr(&node.id),
                    style,
                    style,
                    children
                )
            }
        }
        NodeKind::Group => {
            let mut s = format!(
                "<div data-id=\"{}\" style=\"{}\">\n",
                escape_attr(&node.id),
                style
            );
            s.push_str(&children);
            s.push_str("</div>\n");
            s
        }
    }
}

pub(super) fn format_style_inline(node: &PenNode) -> String {
    let mut parts: Vec<String> = Vec::new();
    // E-9：仅 Free 布局输出 absolute + left/top；Flex/Grid 容器走文档流，
    // 由 display:flex/grid 驱动子元素排列，不强制 absolute。
    match &node.style.layout {
        LayoutMode::Free => {
            parts.push(format!(
                "position:absolute;left:{}px;top:{}px;",
                node.x as i32, node.y as i32
            ));
        }
        LayoutMode::Flex(p) => {
            parts.push("position:relative;".to_string());
            parts.extend(flex_css(p));
        }
        LayoutMode::Grid(g) => {
            parts.push("position:relative;".to_string());
            parts.extend(grid_css(g));
        }
    }
    if node.w > 0.0 {
        parts.push(format!("width:{}px;", node.w as i32));
    }
    if node.h > 0.0 {
        parts.push(format!("height:{}px;", node.h as i32));
    }
    if let Some(fill) = &node.style.fill {
        parts.push(format!(
            "background:{};",
            crate::util::sanitize_css_color(fill)
        ));
    }
    if let Some(stroke) = &node.style.stroke {
        // E-8：输出 stroke_width 而非固定 2px。
        let sw = node.style.stroke_width.unwrap_or(1.0);
        parts.push(format!(
            "border:{}px solid {};",
            sw as i32,
            crate::util::sanitize_css_color(stroke)
        ));
    }
    if let Some(r) = node.style.radius {
        parts.push(format!("border-radius:{}px;", r as i32));
    }
    if let Some(op) = node.style.opacity {
        parts.push(format!("opacity:{};", op));
    }
    // E-8：rotation / z_index。
    if node.rotation != 0.0 {
        parts.push(format!("transform:rotate({}deg);", node.rotation));
    }
    if node.z_index != 0 {
        parts.push(format!("z-index:{};", node.z_index));
    }
    // E-8：font_family / font_size（Text 节点字号字体，非 Text 节点亦输出供容器继承）。
    if let Some(fs) = node.style.font_size {
        parts.push(format!("font-size:{}px;", fs as i32));
    }
    if let Some(ff) = &node.style.font_family {
        parts.push(format!(
            "font-family:{};",
            crate::util::sanitize_css_color(ff)
        ));
    }
    parts.join("")
}

/// E-9：FlexParams → CSS 声明片段（对齐 host-web 渲染约定）。
fn flex_css(p: &fd_canvas_core::FlexParams) -> Vec<String> {
    use fd_canvas_core::{AlignItems, FlexDirection, FlexWrap, JustifyContent};
    let mut v = vec!["display:flex;".to_string()];
    v.push(
        match p.direction {
            FlexDirection::Row => "flex-direction:row;",
            FlexDirection::RowReverse => "flex-direction:row-reverse;",
            FlexDirection::Column => "flex-direction:column;",
            FlexDirection::ColumnReverse => "flex-direction:column-reverse;",
        }
        .into(),
    );
    if p.gap > 0.0 {
        v.push(format!("gap:{}px;", p.gap as i32));
    }
    v.push(
        match p.align_items {
            AlignItems::Start => "align-items:flex-start;",
            AlignItems::Center => "align-items:center;",
            AlignItems::End => "align-items:flex-end;",
            AlignItems::Stretch => "align-items:stretch;",
        }
        .into(),
    );
    v.push(
        match p.justify_content {
            JustifyContent::Start => "justify-content:flex-start;",
            JustifyContent::Center => "justify-content:center;",
            JustifyContent::End => "justify-content:flex-end;",
            JustifyContent::SpaceBetween => "justify-content:space-between;",
            JustifyContent::SpaceAround => "justify-content:space-around;",
            JustifyContent::SpaceEvenly => "justify-content:space-evenly;",
        }
        .into(),
    );
    v.push(
        match p.wrap {
            FlexWrap::NoWrap => "flex-wrap:nowrap;",
            FlexWrap::Wrap => "flex-wrap:wrap;",
        }
        .into(),
    );
    if p.padding.top > 0.0
        || p.padding.right > 0.0
        || p.padding.bottom > 0.0
        || p.padding.left > 0.0
    {
        v.push(format!(
            "padding:{}px {}px {}px {}px;",
            p.padding.top as i32,
            p.padding.right as i32,
            p.padding.bottom as i32,
            p.padding.left as i32
        ));
    }
    v
}

/// E-9：GridParams → CSS 声明片段。
fn grid_css(g: &fd_canvas_core::GridParams) -> Vec<String> {
    use fd_canvas_core::TrackSizing;
    let mut v = vec!["display:grid;".to_string()];
    let cols: Vec<String> = g
        .columns
        .iter()
        .map(|t| match t {
            TrackSizing::Fixed(px) => format!("{}px", *px as i32),
            TrackSizing::Auto => "auto".into(),
            TrackSizing::Flex(f) => format!("{f}fr"),
            TrackSizing::Percent(pct) => format!("{pct}%"),
        })
        .collect();
    if !cols.is_empty() {
        v.push(format!("grid-template-columns:{};", cols.join(" ")));
    }
    let rows: Vec<String> = g
        .rows
        .iter()
        .map(|t| match t {
            TrackSizing::Fixed(px) => format!("{}px", *px as i32),
            TrackSizing::Auto => "auto".into(),
            TrackSizing::Flex(f) => format!("{f}fr"),
            TrackSizing::Percent(pct) => format!("{pct}%"),
        })
        .collect();
    if !rows.is_empty() {
        v.push(format!("grid-template-rows:{};", rows.join(" ")));
    }
    if g.gap.0 > 0.0 || g.gap.1 > 0.0 {
        v.push(format!("gap:{}px {}px;", g.gap.1 as i32, g.gap.0 as i32));
    }
    v
}
