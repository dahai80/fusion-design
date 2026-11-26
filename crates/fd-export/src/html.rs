// ARCH-10 r4: HTML rendering split out of lib.rs. render_html wraps an SVG layer + escaped page name.
// Calls svg::render_svg + util::xml_escape. Zero behavior change.

use crate::svg::render_svg;
use crate::{util::xml_escape, CanvasPage};

pub(super) fn render_html(page: &CanvasPage) -> String {
    let svg = render_svg(page);
    // E-21: page.name may contain </title><script>... via IPC/AI; escape it to prevent HTML injection (WKWebView native bridge = XSS = native execution).
    let safe_name = xml_escape(&page.name);
    format!(
        "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>{name}</title>\
         <style>body{{margin:0;display:flex;justify-content:center;align-items:center;min-height:100vh;background:#f5f5f5;}}</style>\
         </head>\n<body>{svg}</body></html>",
        name = safe_name
    )
}
