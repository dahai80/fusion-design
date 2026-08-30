// ARCH-10 r4：通用工具从 lib.rs 拆出——文件名清洗、image href 白名单、
// Token 颜色解析、元素收集、XML 转义。跨渲染模块共享，零行为变更。

use fd_design_system::DesignSystemRegistry;

use crate::{CanvasElement, CanvasPage};

pub(super) fn sanitize_filename(name: &str) -> String {
    // E-20/P3：空名/全非法字符 → "" → 文件名 ".svg"（隐藏文件，同名页面互相覆盖）。
    // 空结果回退 "page"，保非隐藏、可区分。
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "page".to_string()
    } else {
        sanitized
    }
}

pub(super) fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// R-5：SVG `<image href>` 协议白名单。SVG 可被嵌入 HTML/直接打开，`<image href>`
/// 是 XSS/SSRF 注入面——`javascript:`/`data:text/html` 可执行脚本，`http(s)://`
/// 触发出站请求（违反离线约束）。仅放行：
///   - `data:image/*`（合法内嵌位图/SVG，离线自包含）
///   - 相对/无协议路径（本地资源，不出网）
///
/// 非白名单一律返空串（omit href，渲染空图框而非执行注入）。
pub(super) fn sanitize_image_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // 协议判断：取冒号前部分（小写）。无冒号视为相对路径，放行。
    let lower = trimmed.to_lowercase();
    if lower.starts_with("data:") {
        // 仅放行 data:image/*，拒 data:text/html 等
        if lower.starts_with("data:image/") {
            return trimmed.to_string();
        }
        tracing::warn!(url = %trimmed, "SVG image href 拒绝非 image data URI");
        return String::new();
    }
    if lower.starts_with("javascript:")
        || lower.starts_with("vbscript:")
        || lower.starts_with("http:")
        || lower.starts_with("https:")
        || lower.starts_with("ftp:")
        || lower.starts_with("file:")
    {
        tracing::warn!(url = %trimmed, "SVG image href 拒绝出网/可执行协议");
        return String::new();
    }
    // SEC-N3：无协议前缀路径放行前检查 .. 段防路径穿越。
    // 离线 SVG 无出网，但相对路径可解析到文件系统敏感位置（理论风险）。
    if trimmed.contains("/../") || trimmed.starts_with("../") || trimmed == ".." {
        tracing::warn!(url = %trimmed, "SVG image href 拒绝含 .. 的路径穿越意图");
        return String::new();
    }
    // 无协议前缀：相对路径或纯文件名，放行（本地资源，不出网）
    trimmed.to_string()
}

/// 解析单个颜色值中的 Token 引用为实际色值。
/// 兼容 `var(--color.accent)`（dot）、`var(--color-accent)`（dash）与 `token:color.accent` 三种形式。
/// 未配置设计规范或 token 未定义时保留原值并告警（usvg 会回退黑色，但不影响其他格式导出）。
pub(super) fn resolve_color_var(
    value: &Option<String>,
    reg: &DesignSystemRegistry,
) -> Option<String> {
    let s = value.as_deref()?;
    let trimmed = s.trim();
    let token_name = trimmed
        .strip_prefix("var(--")
        .and_then(|r| r.strip_suffix(')'))
        .map(|rest| rest.trim())
        .or_else(|| trimmed.strip_prefix("token:").map(|rest| rest.trim()));
    if let Some(name) = token_name {
        // L-13：解析 token 链——旧实现只认 TokenValue::Color，链引用（String→token:xxx）漏。
        // 经 DesignSystem::resolve_reference 递归解析（带环检测），再取颜色。
        let normalized = name.replace('-', ".");
        for candidate in [name, normalized.as_str()] {
            if let Some(tv) = reg.lookup(candidate) {
                if let Some(system) = reg.active() {
                    let mut visited = std::collections::HashSet::new();
                    let resolved = system.resolve_reference(tv, &mut visited);
                    // resolve_reference 对非 Color 返回 css 值；颜色直接用，
                    // 非颜色（如 Number/Shadow）回退保留原 var 以免污染 fill。
                    if let fd_design_system::TokenValue::Color(c) = tv {
                        return Some(c.clone());
                    }
                    if !resolved.is_empty() && !resolved.starts_with("var(") {
                        return Some(resolved);
                    }
                }
            }
        }
        tracing::warn!(var = %name, "Token 颜色变量未能在当前设计规范中解析，保留原值");
    }
    Some(s.to_string())
}

/// 对整页元素的 fill/stroke 解析 Token 引用（就地替换）。
pub(super) fn resolve_page_token_vars(page: &mut CanvasPage, reg: &DesignSystemRegistry) {
    for el in &mut page.elements {
        el.fill = resolve_color_var(&el.fill, reg);
        el.stroke = resolve_color_var(&el.stroke, reg);
    }
}

pub(super) fn collect_elements(node: &fd_canvas_core::PenNode, out: &mut Vec<CanvasElement>) {
    let kind = match node.kind {
        fd_canvas_core::NodeKind::Rect => "rect",
        fd_canvas_core::NodeKind::Circle => "circle",
        fd_canvas_core::NodeKind::Text => "text",
        fd_canvas_core::NodeKind::Image => "image",
        fd_canvas_core::NodeKind::Group => "group",
    };
    out.push(CanvasElement {
        kind: kind.into(),
        x: node.x,
        y: node.y,
        w: node.w,
        h: node.h,
        text: node.text.clone(),
        fill: node.style.fill.clone(),
        stroke: node.style.stroke.clone(),
        stroke_width: node.style.stroke_width,
        radius: node.style.radius,
        opacity: node.style.opacity,
        font_size: node.style.font_size,
        font_family: node.style.font_family.clone(),
        rotation: if node.rotation != 0.0 {
            Some(node.rotation)
        } else {
            None
        },
    });
    for child in &node.children {
        collect_elements(child, out);
    }
}
