// ARCH-10 r4：HTML 渲染从 lib.rs 拆出。render_html 包一层 SVG + 页名转义。
// 调 svg::render_svg + util::xml_escape。零行为变更。

use crate::svg::render_svg;
use crate::{util::xml_escape, CanvasPage};

pub(super) fn render_html(page: &CanvasPage) -> String {
    let svg = render_svg(page);
    // E-21：page.name 经 IPC/AI 可含 </title><script>…，转义防 HTML 注入（WKWebView 原生桥接=XSS=原生执行）。
    let safe_name = xml_escape(&page.name);
    format!(
        "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>{name}</title>\
         <style>body{{margin:0;display:flex;justify-content:center;align-items:center;min-height:100vh;background:#f5f5f5;}}</style>\
         </head>\n<body>{svg}</body></html>",
        name = safe_name
    )
}
