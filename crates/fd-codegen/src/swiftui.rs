// ARCH-10 r4：SwiftUI 渲染从 lib.rs 拆出。render_page_swiftui/node_to_swiftui/
// swift_ui_color/escape_swift_string/TokenExt/collect_token_extensions/
// render_token_extensions。调 util（MAX_CODEGEN_DEPTH）+ fd_canvas_core::parse_hex_color。
// 零行为变更。

use std::collections::HashMap;

use fd_canvas_core::{
    parse_hex_color, FlexDirection, LayoutMode, NodeKind, Page, PenDocument, PenNode,
};

use crate::util::MAX_CODEGEN_DEPTH;

pub(super) fn render_page_swiftui(page: &Page, indent: usize) -> String {
    let pad = "    ".repeat(indent);
    let mut out = format!("{}VStack(spacing: 0) {{\n", pad);
    for node in &page.nodes {
        out.push_str(&node_to_swiftui(node, indent + 1));
    }
    out.push_str(&format!("{}}}\n", pad));
    out
}

pub(super) fn node_to_swiftui(node: &PenNode, indent: usize) -> String {
    node_to_swiftui_inner(node, indent, 0)
}

fn node_to_swiftui_inner(node: &PenNode, indent: usize, depth: usize) -> String {
    if depth > MAX_CODEGEN_DEPTH {
        return format!(
            "{}/* codegen depth limit reached */\n",
            "    ".repeat(indent)
        );
    }
    let pad = "    ".repeat(indent);
    let mut mods = vec![];

    if node.x != 0.0 || node.y != 0.0 {
        mods.push(format!(
            ".offset(x: {}, y: {})",
            node.x as i32, node.y as i32
        ));
    }
    if let Some(fill) = &node.style.fill {
        // R-6：fill 入 swift_ui_color("...") 的 Swift 字面量，须转义防注入。
        mods.push(format!(
            ".background(swift_ui_color(\"{}\"))",
            escape_swift_string(fill)
        ));
    }
    if let Some(stroke) = &node.style.stroke {
        // E-8：stroke_width 而非硬编码 lineWidth:1（对齐 HTML/Tailwind 路径）。
        // R-6：stroke 同入 Swift 字面量，转义。
        let sw = node.style.stroke_width.unwrap_or(1.0);
        mods.push(format!(".overlay(RoundedRectangle(cornerRadius: {}).stroke(swift_ui_color(\"{}\"), lineWidth: {}))",
            node.style.radius.unwrap_or(0.0) as i32, escape_swift_string(stroke), sw as i32));
    }
    if let Some(r) = node.style.radius {
        if r > 0.0 {
            mods.push(format!(".cornerRadius({})", r as i32));
        }
    }
    if let Some(op) = node.style.opacity {
        if (op - 1.0).abs() > f64::EPSILON {
            mods.push(format!(".opacity({})", op));
        }
    }
    // E-8：SwiftUI 路径补 rotation/z_index/font_family（此前仅 font_size，
    // 对齐 HTML/Tailwind/React 路径 5 维完整渲染）。
    if node.rotation != 0.0 {
        mods.push(format!(
            ".rotationEffect(.degrees({}))",
            node.rotation as i32
        ));
    }
    if node.z_index != 0 {
        mods.push(format!(".zIndex({})", node.z_index));
    }
    if let Some(fs) = node.style.font_size {
        if let Some(ff) = &node.style.font_family {
            // R-6：font_family 入字面量，转义防注入
            mods.push(format!(
                ".font(.custom(\"{}\", size: {}))",
                escape_swift_string(ff),
                fs as i32
            ));
        } else {
            mods.push(format!(".font(.system(size: {}))", fs as i32));
        }
    } else if let Some(ff) = &node.style.font_family {
        mods.push(format!(
            ".font(.custom(\"{}\", size: 16))",
            escape_swift_string(ff)
        ));
    }

    let mod_str = if mods.is_empty() {
        String::new()
    } else {
        mods.join("")
    };

    let rendered = match node.kind {
        NodeKind::Rect => {
            let w = if node.w > 0.0 {
                format!("width: {}", node.w as i32)
            } else {
                String::new()
            };
            let h = if node.h > 0.0 {
                format!("height: {}", node.h as i32)
            } else {
                String::new()
            };
            let dims = match (w.is_empty(), h.is_empty()) {
                (true, true) => String::new(),
                (true, false) => format!(".frame({})", h),
                (false, true) => format!(".frame({})", w),
                (false, false) => format!(".frame({}, {})", w, h),
            };
            format!("{}Color.clear{}{}\n", pad, dims, mod_str)
        }
        NodeKind::Circle => {
            let size = if node.w > 0.0 {
                node.w as i32
            } else {
                node.h as i32
            };
            format!(
                "{}Circle(){}.frame(width: {}, height: {})\n",
                pad, mod_str, size, size
            )
        }
        NodeKind::Text => {
            let txt = node.text.as_deref().unwrap_or("");
            let escaped = txt.replace('\\', "\\\\").replace('"', "\\\"");
            format!("{}Text(\"{}\"){}\n", pad, escaped, mod_str)
        }
        NodeKind::Image => {
            format!("{}Image(systemName: \"photo\"){}\n", pad, mod_str)
        }
        NodeKind::Group => {
            let (container, spacing) = match &node.style.layout {
                LayoutMode::Flex(flex) => {
                    let sp = if flex.gap > 0.0 {
                        format!("spacing: {}", flex.gap as i32)
                    } else {
                        "spacing: 0".to_string()
                    };
                    match flex.direction {
                        FlexDirection::Row | FlexDirection::RowReverse => ("HStack", sp),
                        FlexDirection::Column | FlexDirection::ColumnReverse => ("VStack", sp),
                    }
                }
                LayoutMode::Grid(_) => ("LazyVGrid", "spacing: 8".to_string()),
                LayoutMode::Free => ("ZStack", String::new()),
            };
            let mut out = if spacing.is_empty() {
                format!("{}{} {{\n", pad, container)
            } else {
                format!("{}{}({}) {{\n", pad, container, spacing)
            };
            for c in &node.children {
                out.push_str(&node_to_swiftui_inner(c, indent + 1, depth + 1));
            }
            out.push_str(&format!("{}}}{}\n", pad, mod_str));
            out
        }
    };

    // 非 Group 叶子节点带子节点：用 ZStack 承载叶子 + 子节点，避免丢嵌套（#7）
    if !matches!(node.kind, NodeKind::Group) && !node.children.is_empty() {
        let mut out = format!("{}ZStack {{\n", pad);
        out.push_str(&rendered);
        for c in &node.children {
            out.push_str(&node_to_swiftui_inner(c, indent + 1, depth + 1));
        }
        out.push_str(&pad);
        out.push('}');
        out.push('\n');
        out
    } else {
        rendered
    }
}

pub(crate) fn swift_ui_color(color: &str) -> String {
    if let Some(rest) = color.strip_prefix("token:") {
        return format!("DesignTokens.{}", rest.replace('.', "_"));
    }
    // C-2/A-2：经共享 parse_hex_color（ASCII 门控，CJK/emoji 返 None 不 panic）。
    if let Some([r, g, b]) = parse_hex_color(color) {
        format!("Color(red: {}/255, green: {}/255, blue: {}/255)", r, g, b)
    } else {
        // R-6：非 hex/token 的颜色名（如 CSS 命名色）入 Color("...") 字符串字面量，
        // 须转义引号/反斜杠/换行，防 `red"); evil(); /*` 注入可执行 Swift。
        format!("Color(\"{}\")", escape_swift_string(color))
    }
}

/// R-6：Swift 字符串字面量转义。注入面：fill/stroke/font_family/color 经 format!
/// 塞入 `"...""` 字面量，未转义的 `"`/`\`/换行可闭合字面量注入可执行代码。
/// 转义顺序：先 `\`（避免二次转义后续插入的反斜杠），再 `"`，再换行符。
pub(crate) fn escape_swift_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

#[derive(Debug, Clone)]
pub(super) struct TokenExt {
    pub swift_name: String,
    pub color_expr: String,
}

pub(super) fn collect_token_extensions(doc: &PenDocument) -> Vec<TokenExt> {
    let mut seen = HashMap::new();
    for page in &doc.pages {
        collect_token_exts_from_nodes(&page.nodes, &mut seen);
    }
    let mut exts: Vec<TokenExt> = seen.into_values().collect();
    exts.sort_by(|a, b| a.swift_name.cmp(&b.swift_name));
    exts
}

fn collect_token_exts_from_nodes(nodes: &[PenNode], seen: &mut HashMap<String, TokenExt>) {
    for node in nodes {
        for (key, token_ref) in &node.style.design_token_refs {
            let swift_name = token_ref.replace('.', "_");
            if seen.contains_key(&swift_name) {
                continue;
            }
            let color_expr = match key.as_str() {
                "fill" => node.style.fill.as_deref().map(swift_ui_color),
                "stroke" => node.style.stroke.as_deref().map(swift_ui_color),
                _ => None,
            };
            if let Some(expr) = color_expr {
                seen.insert(
                    swift_name.clone(),
                    TokenExt {
                        swift_name,
                        color_expr: expr,
                    },
                );
            }
        }
        collect_token_exts_from_nodes(&node.children, seen);
    }
}

pub(super) fn render_token_extensions(exts: &[TokenExt]) -> String {
    let mut out = String::from("extension Color {\n");
    out.push_str("    enum DesignTokens {\n");
    for ext in exts {
        out.push_str(&format!(
            "        static let {} = {}\n",
            ext.swift_name, ext.color_expr
        ));
    }
    out.push_str("    }\n");
    out.push_str("}\n\n");
    out
}
