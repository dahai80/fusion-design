// Callers: fd-cli (export/export-batch), DesignBridge (Swift Process)
// Affected API: Exporter::from_pen_document(), CanvasPage::from_page(), CanvasElement new fields
// Data schemas: PenDocument→CanvasPage bridge, enhanced SVG with NodeStyle (stroke_width/radius/opacity/font/rotation)
// User instruction: "现在开始实施" — Task #16 P3-5 fd-export PNG/SVG/HTML 批量导出

//! Fusion-Design 导出 — PNG/SVG/PDF/HTML 批量导出。
//!
//! 对应 PRD 模块 5「原型交互与交付」的导出能力。
//! 支持格式：HTML（静态）、SVG（矢量）、PNG（tiny-skia 光栅化）、PDF（结构化）、JSON（工程文件）。
//!
//! V0.2: 支持 PenDocument 直接导出（无需手动转 CanvasPage），
//! 增强渲染：NodeStyle（fill/stroke/radius/opacity/font）→ SVG 属性。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 导出格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    Html,
    Svg,
    Json,
    Png,
    Pdf,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Svg => "svg",
            Self::Json => "json",
            Self::Png => "png",
            Self::Pdf => "pdf",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Html => "text/html",
            Self::Svg => "image/svg+xml",
            Self::Json => "application/json",
            Self::Png => "image/png",
            Self::Pdf => "application/pdf",
        }
    }
}

/// 画布页面数据（最小子集，供导出渲染）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanvasPage {
    pub id: String,
    pub name: String,
    pub width: f32,
    pub height: f32,
    pub elements: Vec<CanvasElement>,
}

/// 矢量元素（V0.2 扩展：NodeStyle 全字段）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanvasElement {
    pub kind: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub fill: Option<String>,
    #[serde(default)]
    pub stroke: Option<String>,
    #[serde(default)]
    pub stroke_width: Option<f32>,
    #[serde(default)]
    pub radius: Option<f32>,
    #[serde(default)]
    pub opacity: Option<f32>,
    #[serde(default)]
    pub font_size: Option<f32>,
    #[serde(default)]
    pub font_family: Option<String>,
    #[serde(default)]
    pub rotation: Option<f32>,
}

impl CanvasPage {
    /// 从 fd-canvas-core Page 转换。
    pub fn from_page(page: &fd_canvas_core::Page) -> Self {
        let mut elements = Vec::new();
        for node in &page.nodes {
            collect_elements(node, &mut elements);
        }
        Self {
            id: page.id.clone(),
            name: page.name.clone(),
            width: page.width,
            height: page.height,
            elements,
        }
    }
}

fn collect_elements(node: &fd_canvas_core::PenNode, out: &mut Vec<CanvasElement>) {
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
        rotation: if node.rotation != 0.0 { Some(node.rotation) } else { None },
    });
    for child in &node.children {
        collect_elements(child, out);
    }
}

/// 导出器。
pub struct Exporter;

impl Exporter {
    /// 从 PenDocument 直接导出所有页面。
    pub fn from_pen_document(
        doc: &fd_canvas_core::PenDocument,
        format: ExportFormat,
        out_dir: &Path,
    ) -> anyhow::Result<Vec<PathBuf>> {
        let pages: Vec<CanvasPage> = doc.pages.iter().map(CanvasPage::from_page).collect();
        Self::export_batch(&pages, format, out_dir)
    }

    /// 导出单页到指定格式。
    pub fn export_page(
        page: &CanvasPage,
        format: ExportFormat,
        out_dir: &Path,
    ) -> anyhow::Result<PathBuf> {
        std::fs::create_dir_all(out_dir)?;
        let filename = format!("{}.{}", sanitize_filename(&page.name), format.extension());
        let file = out_dir.join(&filename);
        match format {
            ExportFormat::Png => render_png(page, &file)?,
            ExportFormat::Pdf => render_pdf(page, &file)?,
            _ => {
                let content = match format {
                    ExportFormat::Html => render_html(page),
                    ExportFormat::Svg => render_svg(page),
                    ExportFormat::Json => serde_json::to_string_pretty(page)?,
                    _ => unreachable!(),
                };
                std::fs::write(&file, content)?;
            }
        }
        tracing::info!(?file, format = format.extension(), "页面已导出");
        Ok(file)
    }

    /// 批量导出多页。
    pub fn export_batch(
        pages: &[CanvasPage],
        format: ExportFormat,
        out_dir: &Path,
    ) -> anyhow::Result<Vec<PathBuf>> {
        let mut files = Vec::with_capacity(pages.len());
        for page in pages {
            files.push(Self::export_page(page, format, out_dir)?);
        }
        Ok(files)
    }

    /// 异步批量导出（供任务队列调用）。
    pub async fn export_batch_async(
        pages: Vec<CanvasPage>,
        format: ExportFormat,
        out_dir: PathBuf,
    ) -> anyhow::Result<Vec<PathBuf>> {
        tokio::task::spawn_blocking(move || {
            Exporter::export_batch(&pages, format, &out_dir)
        })
        .await?
    }

    /// 异步从 PenDocument 导出。
    pub async fn from_pen_document_async(
        doc: fd_canvas_core::PenDocument,
        format: ExportFormat,
        out_dir: PathBuf,
    ) -> anyhow::Result<Vec<PathBuf>> {
        tokio::task::spawn_blocking(move || {
            Self::from_pen_document(&doc, format, &out_dir)
        })
        .await?
    }
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

fn render_html(page: &CanvasPage) -> String {
    let svg = render_svg(page);
    format!(
        "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>{name}</title>\
         <style>body{{margin:0;display:flex;justify-content:center;align-items:center;min-height:100vh;background:#f5f5f5;}}</style>\
         </head>\n<body>{svg}</body></html>",
        name = page.name
    )
}

fn render_svg(page: &CanvasPage) -> String {
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

fn render_element_svg(el: &CanvasElement) -> String {
    let fill = xml_escape(el.fill.as_deref().unwrap_or("none"));
    let stroke = xml_escape(el.stroke.as_deref().unwrap_or("none"));
    let sw = el.stroke_width.map(|w| format!("stroke-width=\"{w}\"")).unwrap_or_default();
    let rx = el.radius.map(|r| format!("rx=\"{r}\" ry=\"{r}\"")).unwrap_or_default();
    let opacity = el.opacity.map(|o| format!("opacity=\"{o}\"")).unwrap_or_default();
    let transform = el.rotation.map(|r| {
        format!("transform=\"rotate({r} {} {})\"", el.x + el.w / 2.0, el.y + el.h / 2.0)
    }).unwrap_or_default();

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
            let fs = el.font_size.map(|s| format!("font-size=\"{s}px\"")).unwrap_or_default();
            let ff = el.font_family.as_deref().map(|f| format!("font-family=\"{}\"", xml_escape(f))).unwrap_or_default();
            let text = xml_escape(el.text.as_deref().unwrap_or(""));
            format!(
                "<text x=\"{}\" y=\"{}\" fill=\"{fill}\" {fs} {ff}>{text}</text>\n",
                el.x, el.y
            )
        }
        "image" => format!(
            "<image x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" href=\"{}\" {attrs}/>\n",
            el.x, el.y, el.w, el.h,
            xml_escape(el.text.as_deref().unwrap_or(""))
        ),
        "group" => "<!-- group -->\n".to_string(),
        other => format!("<!-- 未知元素类型 {other} -->\n"),
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace('"', "&quot;")
}

fn render_png(page: &CanvasPage, file: &Path) -> anyhow::Result<()> {
    let svg_str = render_svg(page);
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_str(&svg_str, &opt)?;
    let pixmap_size = tree.size();
    let width = pixmap_size.width() as u32;
    let height = pixmap_size.height() as u32;
    if width == 0 || height == 0 {
        anyhow::bail!("页面尺寸为零，无法渲染 PNG");
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| anyhow::anyhow!("无法创建 pixmap ({}x{})", width, height))?;
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    let png_data = pixmap.encode_png()?;
    std::fs::write(file, &png_data)?;
    tracing::info!(?file, "PNG 已导出");
    Ok(())
}

fn render_pdf(page: &CanvasPage, file: &Path) -> anyhow::Result<()> {
    let width_mm = page.width * 0.264583;
    let height_mm = page.height * 0.264583;

    let mut ops = Vec::new();
    for el in &page.elements {
        if el.kind == "text" {
            let text = el.text.as_deref().unwrap_or("");
            let fs = el.font_size.unwrap_or(12.0);
            ops.push(printpdf::ops::Op::StartTextSection);
            ops.push(printpdf::ops::Op::SetFontSizeBuiltinFont {
                size: printpdf::Pt(fs),
                font: printpdf::BuiltinFont::Helvetica,
            });
            ops.push(printpdf::ops::Op::SetTextCursor {
                pos: printpdf::graphics::Point::new(
                    printpdf::Mm(el.x * 0.264583),
                    printpdf::Mm(height_mm - el.y * 0.264583 - fs * 0.264583),
                ),
            });
            ops.push(printpdf::ops::Op::WriteTextBuiltinFont {
                items: vec![printpdf::text::TextItem::Text(text.to_string())],
                font: printpdf::BuiltinFont::Helvetica,
            });
            ops.push(printpdf::ops::Op::EndTextSection);
        }
    }

    let pdf_page = printpdf::ops::PdfPage::new(
        printpdf::Mm(width_mm),
        printpdf::Mm(height_mm),
        ops,
    );

    let mut doc = printpdf::PdfDocument::new(&page.name);
    doc.with_pages(vec![pdf_page]);

    let mut warnings = Vec::new();
    let opts = printpdf::serialize::PdfSaveOptions::default();
    let pdf_data = doc.save(&opts, &mut warnings);
    std::fs::write(file, &pdf_data)?;
    tracing::info!(?file, warnings = warnings.len(), "PDF 已导出");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_page() -> CanvasPage {
        CanvasPage {
            id: "p1".into(),
            name: "Test Page".into(),
            width: 100.0,
            height: 100.0,
            elements: vec![
                CanvasElement {
                    kind: "rect".into(),
                    x: 0.0, y: 0.0, w: 50.0, h: 50.0,
                    text: None,
                    fill: Some("#FFF".into()),
                    stroke: Some("#000".into()),
                    stroke_width: Some(1.0),
                    radius: Some(4.0),
                    opacity: None,
                    font_size: None,
                    font_family: None,
                    rotation: None,
                },
                CanvasElement {
                    kind: "text".into(),
                    x: 10.0, y: 20.0, w: 0.0, h: 0.0,
                    text: Some("hello".into()),
                    fill: Some("#000".into()),
                    stroke: None,
                    stroke_width: None,
                    radius: None,
                    opacity: None,
                    font_size: Some(14.0),
                    font_family: Some("Helvetica".into()),
                    rotation: None,
                },
            ],
        }
    }

    #[test]
    fn export_html_writes_file() {
        let tmp = tempdir().unwrap();
        let page = sample_page();
        let file = Exporter::export_page(&page, ExportFormat::Html, tmp.path()).unwrap();
        let content = std::fs::read_to_string(&file).unwrap();
        assert!(content.contains("<svg"));
        assert!(content.contains("hello"));
        assert!(content.contains("font-size"));
    }

    #[test]
    fn export_svg_writes_file() {
        let tmp = tempdir().unwrap();
        let page = sample_page();
        let file = Exporter::export_page(&page, ExportFormat::Svg, tmp.path()).unwrap();
        let content = std::fs::read_to_string(&file).unwrap();
        assert!(content.starts_with("<svg"));
        assert!(content.contains("rx=\"4\""));
        assert!(content.contains("stroke-width"));
    }

    #[test]
    fn export_json_writes_file() {
        let tmp = tempdir().unwrap();
        let page = sample_page();
        let file = Exporter::export_page(&page, ExportFormat::Json, tmp.path()).unwrap();
        let content = std::fs::read_to_string(&file).unwrap();
        assert!(content.contains("\"id\""));
        assert!(content.contains("p1"));
    }

    #[test]
    fn export_png_writes_file() {
        let tmp = tempdir().unwrap();
        let page = sample_page();
        let file = Exporter::export_page(&page, ExportFormat::Png, tmp.path()).unwrap();
        let data = std::fs::read(&file).unwrap();
        assert!(data.starts_with(&[0x89, 0x50, 0x4E, 0x47]), "PNG magic bytes");
    }

    #[test]
    fn export_pdf_writes_file() {
        let tmp = tempdir().unwrap();
        let page = sample_page();
        let file = Exporter::export_page(&page, ExportFormat::Pdf, tmp.path()).unwrap();
        let data = std::fs::read(&file).unwrap();
        assert!(data.starts_with(b"%PDF"), "PDF magic bytes");
    }

    #[test]
    fn export_batch_multiple_pages() {
        let tmp = tempdir().unwrap();
        let pages = vec![sample_page(), CanvasPage {
            id: "p2".into(),
            name: "Second".into(),
            width: 200.0,
            height: 200.0,
            elements: vec![],
        }];
        let files = Exporter::export_batch(&pages, ExportFormat::Svg, tmp.path()).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn export_creates_out_dir() {
        let tmp = tempdir().unwrap();
        let out = tmp.path().join("nested").join("out");
        let page = sample_page();
        Exporter::export_page(&page, ExportFormat::Html, &out).unwrap();
        assert!(out.exists());
    }

    #[test]
    fn format_extension_and_mime() {
        assert_eq!(ExportFormat::Html.extension(), "html");
        assert_eq!(ExportFormat::Svg.mime(), "image/svg+xml");
    }

    #[tokio::test]
    async fn export_batch_async_works() {
        let tmp = tempdir().unwrap();
        let pages = vec![sample_page()];
        let files = Exporter::export_batch_async(pages, ExportFormat::Json, tmp.path().to_path_buf())
            .await
            .unwrap();
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn render_element_svg_unknown_kind_commented() {
        let el = CanvasElement {
            kind: "weird".into(),
            x: 0.0, y: 0.0, w: 0.0, h: 0.0,
            text: None, fill: None, stroke: None,
            stroke_width: None, radius: None, opacity: None,
            font_size: None, font_family: None, rotation: None,
        };
        let svg = render_element_svg(&el);
        assert!(svg.contains("未知元素"));
    }

    #[test]
    fn render_svg_includes_circle() {
        let page = CanvasPage {
            id: "p".into(),
            name: "n".into(),
            width: 10.0,
            height: 10.0,
            elements: vec![CanvasElement {
                kind: "circle".into(),
                x: 0.0, y: 0.0, w: 10.0, h: 10.0,
                text: None,
                fill: Some("#F00".into()),
                stroke: None,
                stroke_width: None, radius: None, opacity: None,
                font_size: None, font_family: None, rotation: None,
            }],
        };
        let svg = render_svg(&page);
        assert!(svg.contains("<circle"));
    }

    #[test]
    fn from_pen_document_converts() {
        let mut doc = fd_canvas_core::PenDocument::new();
        let mut page = fd_canvas_core::Page::new("p1", "Home", 100.0, 200.0);
        let mut node = fd_canvas_core::PenNode::rect("n1", 10.0, 20.0, 50.0, 30.0);
        node.style.fill = Some("#FF0000".into());
        node.style.radius = Some(8.0);
        page.add(node);
        doc.add_page(page);

        let cp = CanvasPage::from_page(&doc.pages[0]);
        assert_eq!(cp.id, "p1");
        assert_eq!(cp.name, "Home");
        assert_eq!(cp.width, 100.0);
        assert_eq!(cp.elements.len(), 1);
        assert_eq!(cp.elements[0].fill.as_deref(), Some("#FF0000"));
        assert_eq!(cp.elements[0].radius, Some(8.0));
    }

    #[test]
    fn from_pen_document_export_html() {
        let tmp = tempdir().unwrap();
        let mut doc = fd_canvas_core::PenDocument::new();
        let mut page = fd_canvas_core::Page::new("p1", "Demo", 100.0, 100.0);
        page.add(fd_canvas_core::PenNode::rect("n1", 0.0, 0.0, 50.0, 50.0));
        page.add(fd_canvas_core::PenNode::text("n2", 10.0, 20.0, "world"));
        doc.add_page(page);

        let files = Exporter::from_pen_document(&doc, ExportFormat::Html, tmp.path()).unwrap();
        assert_eq!(files.len(), 1);
        let content = std::fs::read_to_string(&files[0]).unwrap();
        assert!(content.contains("world"));
    }

    #[test]
    fn sanitize_filename_strips_special() {
        assert_eq!(sanitize_filename("Hello/World"), "Hello_World");
        assert_eq!(sanitize_filename("Page 1"), "Page_1");
        assert_eq!(sanitize_filename("ok-name_123"), "ok-name_123");
    }

    #[test]
    fn xml_escape_handles_special() {
        assert_eq!(xml_escape("<b>bold</b>"), "&lt;b&gt;bold&lt;/b&gt;");
        assert_eq!(xml_escape("a&b"), "a&amp;b");
        assert_eq!(xml_escape("he said \"hi\""), "he said &quot;hi&quot;");
    }

    #[test]
    fn svg_attribute_injection_escaped() {
        let el = CanvasElement {
            kind: "rect".into(),
            x: 0.0, y: 0.0, w: 10.0, h: 10.0,
            fill: Some("red\" onclick=\"alert(1)".into()),
            stroke: Some("blue\" onload=\"evil()".into()),
            stroke_width: None, radius: None, opacity: None, rotation: None,
            text: None, font_size: None, font_family: None,
        };
        let svg = render_element_svg(&el);
        assert!(!svg.contains("\" onclick=\""), "fill should not create new attribute: {svg}");
        assert!(!svg.contains("\" onload=\""), "stroke should not create new attribute: {svg}");
        assert!(svg.contains("fill=\"red&quot;"), "fill quotes should be escaped: {svg}");
        assert!(svg.contains("stroke=\"blue&quot;"), "stroke quotes should be escaped: {svg}");
    }

    #[test]
    fn rotation_in_svg() {
        let el = CanvasElement {
            kind: "rect".into(),
            x: 10.0, y: 20.0, w: 50.0, h: 30.0,
            text: None, fill: Some("#000".into()), stroke: None,
            stroke_width: None, radius: None, opacity: None,
            font_size: None, font_family: None, rotation: Some(45.0),
        };
        let svg = render_element_svg(&el);
        assert!(svg.contains("rotate(45"));
    }

    #[test]
    fn nested_children_flattened() {
        let mut doc = fd_canvas_core::PenDocument::new();
        let mut page = fd_canvas_core::Page::new("p1", "Nested", 100.0, 100.0);
        let group = fd_canvas_core::PenNode::group(
            "g1", 0.0, 0.0,
            vec![fd_canvas_core::PenNode::rect("c1", 5.0, 5.0, 10.0, 10.0)],
        );
        page.add(group);
        doc.add_page(page);

        let cp = CanvasPage::from_page(&doc.pages[0]);
        assert_eq!(cp.elements.len(), 2);
        assert_eq!(cp.elements[0].kind, "group");
        assert_eq!(cp.elements[1].kind, "rect");
    }

    #[test]
    fn backward_compat_old_canvas_page_json() {
        let old_json = r##"{"id":"p1","name":"Test","width":100,"height":100,"elements":[{"kind":"rect","x":0,"y":0,"w":50,"h":50,"text":null,"fill":"#FFF","stroke":"#000"}]}"##;
        let page: CanvasPage = serde_json::from_str(old_json).unwrap();
        assert_eq!(page.elements[0].stroke_width, None);
        assert_eq!(page.elements[0].radius, None);
    }
}
