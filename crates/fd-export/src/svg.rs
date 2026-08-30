// ARCH-10 r4：SVG 渲染从 lib.rs 拆出。render_html 经 render_svg 跨模块调；
// render_png 也调 render_svg。render_element_svg 调 xml_escape/sanitize_image_url
// （util 模块）。circle_polygon 留 lib.rs（PDF 独占，非 SVG）。零行为变更。

use crate::{util::sanitize_image_url, util::xml_escape, CanvasElement, CanvasPage};

pub(super) fn render_svg(page: &CanvasPage) -> String {
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\">\n",
        page.width, page.height, page.width, page.height
    );
    for el in &page.elements {
        svg.push_str(&render_element_svg(el));
    }
    svg.push_str("</svg>");
    svg
}

pub(super) fn render_element_svg(el: &CanvasElement) -> String {
    let fill = xml_escape(el.fill.as_deref().unwrap_or("none"));
    let stroke = xml_escape(el.stroke.as_deref().unwrap_or("none"));
    let sw = el
        .stroke_width
        .map(|w| format!("stroke-width=\"{w}\""))
        .unwrap_or_default();
    let rx = el
        .radius
        .map(|r| format!("rx=\"{r}\" ry=\"{r}\""))
        .unwrap_or_default();
    let opacity = el
        .opacity
        .map(|o| format!("opacity=\"{o}\""))
        .unwrap_or_default();
    let transform = el
        .rotation
        .map(|r| {
            format!(
                "transform=\"rotate({r} {} {})\"",
                el.x + el.w / 2.0,
                el.y + el.h / 2.0
            )
        })
        .unwrap_or_default();

    let attrs = format!("fill=\"{fill}\" stroke=\"{stroke}\" {sw} {rx} {opacity} {transform}");

    match el.kind.as_str() {
        "rect" => format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" {attrs}/>\n",
            el.x, el.y, el.w, el.h
        ),
        "circle" => format!(
            "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" {attrs}/>\n",
            el.x + el.w / 2.0,
            el.y + el.h / 2.0,
            el.w.min(el.h) / 2.0,
        ),
        "text" => {
            let fs = el
                .font_size
                .map(|s| format!("font-size=\"{s}px\""))
                .unwrap_or_default();
            let ff = el
                .font_family
                .as_deref()
                .map(|f| format!("font-family=\"{}\"", xml_escape(f)))
                .unwrap_or_default();
            let text = xml_escape(el.text.as_deref().unwrap_or(""));
            // FUNC-9：text 旋转——基点对齐 rect/circle（el.x+w/2, el.y+h/2），
            // 非用共用 attrs（text 的 fill/stroke/opacity 语义独立，arm 不消费 attrs）。
            let transform = el
                .rotation
                .filter(|r| *r != 0.0)
                .map(|r| {
                    format!(
                        "transform=\"rotate({r} {} {})\"",
                        el.x + el.w / 2.0,
                        el.y + el.h / 2.0
                    )
                })
                .unwrap_or_default();
            format!(
                "<text x=\"{}\" y=\"{}\" fill=\"{fill}\" {fs} {ff} {transform}>{text}</text>\n",
                el.x, el.y
            )
        }
        "image" => format!(
            "<image x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" href=\"{}\" {attrs}/>\n",
            el.x,
            el.y,
            el.w,
            el.h,
            xml_escape(&sanitize_image_url(el.text.as_deref().unwrap_or("")))
        ),
        "group" => "<!-- group -->\n".to_string(),
        other => format!("<!-- 未知元素类型 {} -->\n", xml_escape(other)),
    }
}
