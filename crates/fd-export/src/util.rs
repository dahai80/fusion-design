// ARCH-10 r4: shared utilities split out of lib.rs — filename sanitizing, image href whitelist,
// token color resolution, element collection, XML escaping. Shared across rendering modules, zero behavior change.

use fd_design_system::DesignSystemRegistry;

use crate::{CanvasElement, CanvasPage};

pub(super) fn sanitize_filename(name: &str) -> String {
    // E-20/P3: empty name / all-illegal chars -> "" -> filename ".svg" (hidden file, same-named pages overwrite each other).
    // An empty result falls back to "page", keeping it non-hidden and distinguishable.
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

/// R-5: protocol whitelist for SVG `<image href>`. SVG can be embedded in HTML or opened directly, and
/// `<image href>` is an XSS/SSRF injection surface — `javascript:`/`data:text/html` can execute scripts,
/// and `http(s)://` triggers outbound requests (violating the offline constraint). Only allow:
///   - `data:image/*` (legitimate embedded bitmap/SVG, offline self-contained)
///   - relative/protocol-less paths (local resources, no outbound traffic)
///
/// Anything not whitelisted returns an empty string (omit href, rendering an empty image frame instead of executing the injection).
pub(super) fn sanitize_image_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Protocol check: take the part before the colon (lowercased). No colon is treated as a relative path and allowed.
    let lower = trimmed.to_lowercase();
    if lower.starts_with("data:") {
        // Only allow data:image/*, reject data:text/html etc.
        if lower.starts_with("data:image/") {
            return trimmed.to_string();
        }
        tracing::warn!(url = %trimmed, "SVG image href: rejecting non-image data URI");
        return String::new();
    }
    if lower.starts_with("javascript:")
        || lower.starts_with("vbscript:")
        || lower.starts_with("http:")
        || lower.starts_with("https:")
        || lower.starts_with("ftp:")
        || lower.starts_with("file:")
    {
        tracing::warn!(url = %trimmed, "SVG image href: rejecting outbound/executable protocol");
        return String::new();
    }
    // SEC-N3: before allowing protocol-less paths, check for .. segments to prevent path traversal.
    // Offline SVG has no outbound traffic, but a relative path could resolve to a sensitive filesystem location (theoretical risk).
    if trimmed.contains("/../") || trimmed.starts_with("../") || trimmed == ".." {
        tracing::warn!(url = %trimmed, "SVG image href: rejecting path traversal attempt containing ..");
        return String::new();
    }
    // No protocol prefix: relative path or bare filename, allow (local resource, no outbound traffic)
    trimmed.to_string()
}

/// Resolve Token references in a single color value to the actual color.
/// Supports three forms: `var(--color.accent)` (dot), `var(--color-accent)` (dash), and `token:color.accent`.
/// When no design system is configured or the token is undefined, keeps the original value and logs a warning (usvg would fall back to black, but other export formats are unaffected).
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
        // L-13: resolve token chains — the old impl only recognized TokenValue::Color and missed chain references (String -> token:xxx).
        // Resolve recursively via DesignSystem::resolve_reference (with cycle detection), then take the color.
        let normalized = name.replace('-', ".");
        for candidate in [name, normalized.as_str()] {
            if let Some(tv) = reg.lookup(candidate) {
                if let Some(system) = reg.active() {
                    let mut visited = std::collections::HashSet::new();
                    let resolved = system.resolve_reference(tv, &mut visited);
                    // resolve_reference returns the css value for non-Color tokens; use colors directly,
                    // and for non-colors (e.g. Number/Shadow) fall back to keeping the original var to avoid polluting fill.
                    if let fd_design_system::TokenValue::Color(c) = tv {
                        return Some(c.clone());
                    }
                    if !resolved.is_empty() && !resolved.starts_with("var(") {
                        return Some(resolved);
                    }
                }
            }
        }
        tracing::warn!(var = %name, "Token color variable could not be resolved in the active design system, keeping original value");
    }
    Some(s.to_string())
}

/// Resolve Token references for the fill/stroke of all elements on a page (in-place replacement).
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
