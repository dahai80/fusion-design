//! Fusion-Design HTML → PenDocument 解析器。
//!
//! A-2：从 fd-ai-adapter 拆出的叶子 crate。将 AI 响应中的 HTML artifact
//! 转换为 PenDocument 节点树。与 adapter 内部（FusionMlxClient/SkillRegistry）
//! 零耦合，独立成 crate 便于复用与测试。

use anyhow::Result;
use fd_canvas_core::{NodeKind, NodeStyle, Page, PenDocument, PenNode};
use scraper::{ElementRef, Html, Node, Selector};

// ── HTML → PenDocument 解析器 ──
//
// AI 响应中常包含 HTML artifact（如 <artifact type="html">... 或 ```html...``` 代码块）。
// 本解析器将 HTML 元素转换为 PenDocument 节点树，支持：
// - 基础元素映射：div/section/main→Rect, h1-h6/p/span/a→Text, img→Image, button→Rect
// - 样式提取：width/height/left/top/background/color/font-size/border-radius
// - 嵌套子元素递归解析
// - class→token 引用（如 class="bg-primary" → fill=var(--color-primary)）

pub fn html_to_pen_document(html: &str, page_name: &str) -> Result<PenDocument> {
    let extracted = extract_html_artifact(html);
    let document = Html::parse_document(&extracted);

    let body_sel = Selector::parse("body").unwrap();
    let body_el = document.select(&body_sel).next();

    let container = body_el
        .map(|el| el.inner_html())
        .unwrap_or_else(|| extracted.clone());
    let container_doc = Html::parse_fragment(&container);

    let mut doc = PenDocument::new();
    let mut page = Page::new("page_1", page_name, 1440.0, 900.0);

    let root_sel = Selector::parse(":root > *").unwrap_or_else(|_| Selector::parse("*").unwrap());
    let mut auto_y: f64 = 0.0;
    let mut node_counter: u32 = 0;
    for el_ref in container_doc.select(&root_sel) {
        if let Some(node) = html_element_to_node(&el_ref, 0.0, &mut auto_y, 0, &mut node_counter) {
            auto_y += node.h + 8.0;
            page.add(node);
        }
    }

    if page.nodes.is_empty() {
        let any_sel = Selector::parse("*").unwrap();
        let root_el = container_doc.select(&any_sel).next();
        if let Some(root) = root_el {
            for child_ref in root.child_elements() {
                if let Some(node) =
                    html_element_to_node(&child_ref, 0.0, &mut auto_y, 0, &mut node_counter)
                {
                    auto_y += node.h + 8.0;
                    page.add(node);
                }
            }
        }
    }

    doc.add_page(page);
    tracing::info!(
        "html_to_pen_document: 解析完成，{} 个节点",
        doc.pages.first().map(|p| p.nodes.len()).unwrap_or(0)
    );
    Ok(doc)
}

fn extract_html_artifact(raw: &str) -> String {
    if let Some(start) = raw.find(r#"<artifact"#) {
        if let Some(content_start) = raw[start..].find('>') {
            let content_start = start + content_start + 1;
            if let Some(end) = raw[content_start..].find("</artifact>") {
                return raw[content_start..content_start + end].trim().to_string();
            }
        }
    }

    if let Some(start_marker) = raw.find("```html") {
        let content_start = start_marker + 7;
        if let Some(end) = raw[content_start..].find("```") {
            return raw[content_start..content_start + end].trim().to_string();
        }
    }

    if raw.trim().starts_with("```") {
        let trimmed = raw.trim();
        let first_newline = trimmed.find('\n').unwrap_or(3);
        let content_start = first_newline + 1;
        if let Some(end) = trimmed[content_start..].rfind("```") {
            return trimmed[content_start..content_start + end]
                .trim()
                .to_string();
        }
    }

    if raw.contains('<') && raw.contains('>') {
        return raw.trim().to_string();
    }

    raw.trim().to_string()
}

fn html_element_to_node(
    el_ref: &ElementRef,
    base_x: f64,
    auto_y: &mut f64,
    depth: usize,
    counter: &mut u32,
) -> Option<PenNode> {
    const MAX_PARSE_HTML_DEPTH: usize = 64;
    if depth > MAX_PARSE_HTML_DEPTH {
        tracing::warn!(depth, "parse-html 嵌套深度超限，截断子树");
        return None;
    }
    let el = el_ref.value();
    let tag = el.name();

    let (kind, name) = match tag {
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => (NodeKind::Text, format!("heading_{tag}")),
        "p" | "span" | "a" | "label" | "li" => (NodeKind::Text, tag.to_string()),
        "img" | "svg" => (NodeKind::Image, tag.to_string()),
        "input" | "textarea" | "select" => (NodeKind::Rect, format!("input_{tag}")),
        "button" => (NodeKind::Rect, "button".to_string()),
        _ => (NodeKind::Rect, tag.to_string()),
    };

    let text = extract_text_content(el_ref);
    let mut style = NodeStyle::default();
    let (mut x, mut y, mut w, mut h) = (base_x, *auto_y, 300.0, 40.0);

    match tag {
        "h1" => {
            w = 1440.0;
            h = 60.0;
            style.fill = Some("#FFFFFF".into());
        }
        "h2" => {
            w = 600.0;
            h = 48.0;
            style.fill = Some("#FFFFFF".into());
        }
        "h3" => {
            w = 400.0;
            h = 36.0;
            style.fill = Some("#FFFFFF".into());
        }
        "p" | "span" | "a" | "label" => {
            w = 300.0;
            h = 24.0;
            style.fill = Some("#E0E0E0".into());
        }
        "button" => {
            w = 120.0;
            h = 40.0;
            style.radius = Some(8.0);
            style.fill = Some("#007AFF".into());
        }
        "input" => {
            w = 300.0;
            h = 36.0;
            style.radius = Some(6.0);
            style.fill = Some("#2C2C2E".into());
            style.stroke = Some("1px solid #555".into());
        }
        "img" => {
            w = 200.0;
            h = 150.0;
        }
        "div" | "section" | "main" | "header" | "footer" | "nav" | "article" | "form" => {
            if text.is_some() {
                h = 40.0;
            } else {
                h = 80.0;
            }
            w = 1440.0;
        }
        "ul" | "ol" => {
            w = 300.0;
            h = 120.0;
        }
        "li" => {
            w = 280.0;
            h = 28.0;
            style.fill = Some("#E0E0E0".into());
        }
        _ => {}
    }

    let parsed_style = el.attr("style").map(parse_inline_style);
    if let Some(ref parsed) = parsed_style {
        if let Some(v) = parsed.get("width") {
            w = parse_px(v).unwrap_or(w);
        }
        if let Some(v) = parsed.get("height") {
            h = parse_px(v).unwrap_or(h);
        }
        if let Some(v) = parsed.get("left") {
            x = base_x + parse_px(v).unwrap_or(0.0);
        }
        if let Some(v) = parsed.get("top") {
            y = parse_px(v).unwrap_or(0.0);
        }
        if let Some(v) = parsed.get("background") {
            style.fill = Some(v.clone());
        }
        if let Some(v) = parsed.get("background-color") {
            style.fill = Some(v.clone());
        }
        if let Some(v) = parsed.get("color") {
            if kind == NodeKind::Text {
                style.fill = Some(v.clone());
            }
        }
        if let Some(v) = parsed.get("border-radius") {
            style.radius = Some(parse_px(v).unwrap_or(0.0));
        }
        if let Some(v) = parsed.get("border") {
            style.stroke = Some(v.clone());
        }
    }

    if let Some(class) = el.attr("class") {
        if let Some(token_fill) = class_to_fill_hint(class) {
            let has_bg = parsed_style.as_ref().is_some_and(|p| {
                p.contains_key("background") || p.contains_key("background-color")
            });
            if !has_bg {
                style.fill = Some(token_fill);
            }
        }
    }

    *counter += 1;
    const MAX_PARSE_HTML_TOTAL: u32 = 100_000;
    if *counter > MAX_PARSE_HTML_TOTAL {
        tracing::warn!(total = *counter, "parse-html 节点总数超限，截断剩余子树");
        return None;
    }
    let id = format!("n_{}", counter);
    let node_text = if kind == NodeKind::Text { text } else { None };

    let children: Vec<PenNode> = el_ref
        .child_elements()
        .filter_map(|child_ref| {
            let mut child_auto_y = 0.0f64;
            html_element_to_node(&child_ref, x + 16.0, &mut child_auto_y, depth + 1, counter)
        })
        .collect();

    Some(PenNode {
        id,
        kind,
        name,
        x,
        y,
        w,
        h,
        style,
        text: node_text,
        children,
        rotation: 0.0,
        z_index: depth as i32,
    })
}

fn extract_text_content(el_ref: &ElementRef) -> Option<String> {
    let mut texts = Vec::new();
    for child in el_ref.children() {
        if let Node::Text(t) = child.value() {
            let trimmed = t.text.trim();
            if !trimmed.is_empty() {
                texts.push(trimmed.to_string());
            }
        }
    }
    if texts.is_empty() {
        None
    } else {
        Some(texts.join(" "))
    }
}

fn parse_inline_style(style_str: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for decl in style_str.split(';') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        if let Some((key, val)) = decl.split_once(':') {
            map.insert(key.trim().to_string(), val.trim().to_string());
        }
    }
    map
}

fn parse_px(val: &str) -> Option<f64> {
    let v = val.trim();
    if let Some(num) = v.strip_suffix("px") {
        num.trim().parse::<f64>().ok()
    } else if let Ok(n) = v.parse::<f64>() {
        Some(n)
    } else if let Some(pct) = v.strip_suffix('%') {
        pct.trim().parse::<f64>().ok()
    } else {
        None
    }
}

fn class_to_fill_hint(class: &str) -> Option<String> {
    for cls in class.split_whitespace() {
        match cls {
            "bg-primary" | "btn-primary" => return Some("var(--color-accent)".into()),
            "bg-secondary" | "btn-secondary" => return Some("var(--color-secondary)".into()),
            "bg-danger" | "btn-danger" => return Some("var(--color-error)".into()),
            "bg-success" | "btn-success" => return Some("var(--color-success)".into()),
            "bg-dark" => return Some("#1C1C1E".into()),
            "bg-light" | "bg-white" => return Some("#FFFFFF".into()),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_plain_html() {
        let html = r#"<div><h1>Hello</h1><p>World</p></div>"#;
        let result = extract_html_artifact(html);
        assert!(result.contains("<h1>"));
    }

    #[test]
    fn extract_code_fenced_html() {
        let raw = "```html\n<div><button>Click</button></div>\n```";
        let result = extract_html_artifact(raw);
        assert!(result.contains("<button>"));
        assert!(!result.contains("```"));
    }

    #[test]
    fn extract_artifact_tag() {
        let raw = r#"<artifact type="html"><div>Content</div></artifact>"#;
        let result = extract_html_artifact(raw);
        assert!(result.contains("<div>Content</div>"));
        assert!(!result.contains("<artifact"));
    }

    #[test]
    fn html_to_pen_document_basic() {
        let html = r#"<h1>Title</h1><p>Paragraph</p><button>Click</button>"#;
        let doc = html_to_pen_document(html, "TestPage").unwrap();
        assert_eq!(doc.pages.len(), 1);
        let page = &doc.pages[0];
        assert!(page.nodes.len() >= 2, "at least h1 and p");
        let h1 = page.nodes.iter().find(|n| n.name == "heading_h1");
        assert!(h1.is_some());
        assert_eq!(h1.unwrap().kind, NodeKind::Text);
        assert_eq!(h1.unwrap().text.as_deref(), Some("Title"));
    }

    #[test]
    fn html_to_pen_document_button() {
        let html = r#"<button>Submit</button>"#;
        let doc = html_to_pen_document(html, "BtnPage").unwrap();
        let btn = doc.pages[0].nodes.iter().find(|n| n.name == "button");
        assert!(btn.is_some());
        assert_eq!(btn.unwrap().kind, NodeKind::Rect);
        assert_eq!(btn.unwrap().style.radius, Some(8.0));
    }

    #[test]
    fn html_to_pen_document_inline_style() {
        let html = r#"<div style="background: #333; width: 200px; height: 100px; border-radius: 12px;">Box</div>"#;
        let doc = html_to_pen_document(html, "StylePage").unwrap();
        let div = doc.pages[0].nodes.first().unwrap();
        assert_eq!(div.w, 200.0);
        assert_eq!(div.h, 100.0);
        assert_eq!(div.style.fill.as_deref(), Some("#333"));
        assert_eq!(div.style.radius, Some(12.0));
    }

    #[test]
    fn html_to_pen_document_class_token_hint() {
        let html = r#"<button class="btn-primary">Go</button>"#;
        let doc = html_to_pen_document(html, "TokenPage").unwrap();
        let btn = doc.pages[0].nodes.first().unwrap();
        assert_eq!(btn.style.fill.as_deref(), Some("var(--color-accent)"));
    }

    #[test]
    fn html_to_pen_document_nested() {
        let html = r#"<div><h1>Title</h1><p>Sub</p></div>"#;
        let doc = html_to_pen_document(html, "NestedPage").unwrap();
        let div = doc.pages[0].nodes.first().unwrap();
        assert!(!div.children.is_empty(), "div should have child nodes");
    }

    #[test]
    fn html_to_pen_document_deeply_nested_no_overflow() {
        let mut html = String::new();
        for _ in 0..68 {
            html.push_str("<div>");
        }
        html.push_str("deep");
        for _ in 0..68 {
            html.push_str("</div>");
        }
        let result = html_to_pen_document(&html, "DeepPage");
        assert!(result.is_ok(), "深度嵌套 HTML 必须被深度守卫截断而非崩溃");
    }

    #[test]
    fn html_to_pen_document_img() {
        let html = r#"<img src="test.png" />"#;
        let doc = html_to_pen_document(html, "ImgPage").unwrap();
        let img = doc.pages[0]
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Image);
        assert!(img.is_some());
    }

    #[test]
    fn parse_inline_style_basic() {
        let map = parse_inline_style("width: 100px; height: 50px; color: #fff");
        assert_eq!(map.get("width").unwrap(), "100px");
        assert_eq!(map.get("height").unwrap(), "50px");
        assert_eq!(map.get("color").unwrap(), "#fff");
    }

    #[test]
    fn parse_px_values() {
        assert_eq!(parse_px("100px"), Some(100.0));
        assert_eq!(parse_px("50"), Some(50.0));
        assert_eq!(parse_px("75%"), Some(75.0));
        assert_eq!(parse_px("auto"), None);
    }

    #[test]
    fn class_to_fill_hint_mapping() {
        assert_eq!(
            class_to_fill_hint("bg-primary"),
            Some("var(--color-accent)".to_string())
        );
        assert_eq!(
            class_to_fill_hint("bg-danger"),
            Some("var(--color-error)".to_string())
        );
        assert_eq!(class_to_fill_hint("unknown"), None);
    }
}
