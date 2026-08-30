// ARCH-10 r4：React/Tailwind 渲染从 lib.rs 拆出。render_page_react/node_to_react/
// node_to_tailwind。调 util（escape_attr/escape_html/sanitize_css_color/MAX_CODEGEN_DEPTH）。
// 零行为变更：纯位置迁移，不改任何渲染逻辑。

use fd_canvas_core::{FlexDirection, LayoutMode, NodeKind, Page, PenNode};

use crate::util::{escape_attr, escape_html, sanitize_css_color, MAX_CODEGEN_DEPTH};

pub(super) fn render_page_react(page: &Page) -> String {
    let mut out = format!("      {{/* page: {} */}}\n", page.id);
    for node in &page.nodes {
        out.push_str(&node_to_react(node));
    }
    out
}

pub(super) fn node_to_react(node: &PenNode) -> String {
    node_to_react_inner(node, 0)
}

fn node_to_react_inner(node: &PenNode, depth: usize) -> String {
    if depth > MAX_CODEGEN_DEPTH {
        return "{/* codegen depth limit reached */}\n".to_string();
    }
    let cls = node_to_tailwind(node);
    // 同 node_to_html：统一收集子节点并注入各分支，修复叶子节点丢嵌套（#7）。
    let children: String = node
        .children
        .iter()
        .map(|c| node_to_react_inner(c, depth + 1))
        .collect();
    match node.kind {
        NodeKind::Rect | NodeKind::Circle => {
            let extra = if matches!(node.kind, NodeKind::Circle) {
                " rounded-full"
            } else {
                ""
            };
            if children.is_empty() {
                format!(
                    "      <div className=\"{}{}\" data-id=\"{}\"/>\n",
                    cls,
                    extra,
                    escape_attr(&node.id)
                )
            } else {
                format!(
                    "      <div className=\"{}{}\" data-id=\"{}\">\n{}</div>\n",
                    cls,
                    extra,
                    escape_attr(&node.id),
                    children
                )
            }
        }
        NodeKind::Text => format!(
            "      <div className=\"{}\" data-id=\"{}\">{}{}</div>\n",
            cls,
            escape_attr(&node.id),
            escape_html(node.text.as_deref().unwrap_or("")),
            children
        ),
        NodeKind::Image => {
            if children.is_empty() {
                format!(
                    "      <img className=\"{}\" data-id=\"{}\" alt=\"\"/>\n",
                    cls,
                    escape_attr(&node.id)
                )
            } else {
                // <img> 自闭合；有子节点时外裹 div 承载
                format!(
                    "      <div className=\"{}\" data-id=\"{}\"><img alt=\"\"/>{}</div>\n",
                    cls,
                    escape_attr(&node.id),
                    children
                )
            }
        }
        NodeKind::Group => {
            let mut s = format!(
                "      <div className=\"{}\" data-id=\"{}\">\n",
                cls,
                escape_attr(&node.id)
            );
            s.push_str(&children);
            s.push_str("      </div>\n");
            s
        }
    }
}

pub(super) fn node_to_tailwind(node: &PenNode) -> String {
    let mut cls: Vec<String> = Vec::new();
    // E-9：仅 Free 布局输出 absolute + left/top；Flex/Grid 走文档流。
    match &node.style.layout {
        LayoutMode::Free => {
            cls.push("absolute".into());
            cls.push(format!("left-[{}px]", node.x as i32));
            cls.push(format!("top-[{}px]", node.y as i32));
        }
        LayoutMode::Flex(p) => {
            cls.push("relative".into());
            cls.push("flex".into());
            cls.push(
                match p.direction {
                    FlexDirection::Row => "flex-row",
                    FlexDirection::RowReverse => "flex-row-reverse",
                    FlexDirection::Column => "flex-col",
                    FlexDirection::ColumnReverse => "flex-col-reverse",
                }
                .into(),
            );
            if p.gap > 0.0 {
                cls.push(format!("gap-[{}px]", p.gap as i32));
            }
            cls.push(
                match p.align_items {
                    fd_canvas_core::AlignItems::Start => "items-start",
                    fd_canvas_core::AlignItems::Center => "items-center",
                    fd_canvas_core::AlignItems::End => "items-end",
                    fd_canvas_core::AlignItems::Stretch => "items-stretch",
                }
                .into(),
            );
            cls.push(
                match p.justify_content {
                    fd_canvas_core::JustifyContent::Start => "justify-start",
                    fd_canvas_core::JustifyContent::Center => "justify-center",
                    fd_canvas_core::JustifyContent::End => "justify-end",
                    fd_canvas_core::JustifyContent::SpaceBetween => "justify-between",
                    fd_canvas_core::JustifyContent::SpaceAround => "justify-around",
                    fd_canvas_core::JustifyContent::SpaceEvenly => "justify-evenly",
                }
                .into(),
            );
            cls.push(
                match p.wrap {
                    fd_canvas_core::FlexWrap::NoWrap => "flex-nowrap",
                    fd_canvas_core::FlexWrap::Wrap => "flex-wrap",
                }
                .into(),
            );
        }
        LayoutMode::Grid(_) => {
            // Tailwind Grid 精细类需模板生成，此处用 arbitrary value 兜底；
            // 真实 grid 布局建议走 format_style_inline（grid-template-columns）。
            cls.push("relative".into());
            cls.push("grid".into());
        }
    }
    if node.w > 0.0 {
        cls.push(format!("w-[{}px]", node.w as i32));
    }
    if node.h > 0.0 {
        cls.push(format!("h-[{}px]", node.h as i32));
    }
    if let Some(fill) = &node.style.fill {
        cls.push(format!("bg-[{}]", sanitize_css_color(fill)));
    }
    if let Some(stroke) = &node.style.stroke {
        // E-8：输出 stroke_width 而非固定 border-2。
        let sw = node.style.stroke_width.unwrap_or(1.0);
        cls.push(format!("border-[{}px]", sw as i32));
        cls.push(format!("border-[{}]", sanitize_css_color(stroke)));
    }
    if let Some(r) = node.style.radius {
        cls.push(format!("rounded-[{}px]", r as i32));
    }
    if let Some(op) = node.style.opacity {
        cls.push(format!("opacity-[{}]", op));
    }
    // E-8：rotation / z_index / font。
    if node.rotation != 0.0 {
        cls.push(format!("rotate-[{}deg]", node.rotation));
    }
    if node.z_index != 0 {
        cls.push(format!("z-[{}]", node.z_index));
    }
    if let Some(fs) = node.style.font_size {
        cls.push(format!("text-[{}px]", fs as i32));
    }
    if let Some(ff) = &node.style.font_family {
        cls.push(format!("font-[{}]", sanitize_css_color(ff)));
    }
    cls.join(" ")
}
