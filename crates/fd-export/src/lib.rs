// Callers: fd-cli (export/export-batch), DesignBridge (Swift Process)
// Affected API: Exporter::from_pen_document(), CanvasPage::from_page(), CanvasElement new fields
// Data schemas: PenDocument→CanvasPage bridge, enhanced SVG with NodeStyle (stroke_width/radius/opacity/font/rotation)
// User instruction: "start implementing now" — Task #16 P3-5 fd-export PNG/SVG/HTML batch export
//
// ARCH-10 r4: god-file split. lib.rs keeps only public types (ExportError/ExportFormat/CanvasPage/
// CanvasElement) + Exporter impl + constants. Rendering logic split into svg/html/png/pdf modules,
// shared utilities (filename sanitizing/xml escaping/token parsing/element collection) into util.
// Pure positional move, zero behavior change.

//! Fusion-Design export — PNG/SVG/PDF/HTML batch export.
//!
//! Covers the export capability of PRD module 5 "Prototype Interaction & Delivery".
//! Supported formats: HTML (static), SVG (vector), PNG (tiny-skia rasterization), PDF (structured), JSON (project file).
//!
//! V0.2: supports exporting PenDocument directly (no manual CanvasPage conversion),
//! enhanced rendering: NodeStyle (fill/stroke/radius/opacity/font) -> SVG attributes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod html;
mod pdf;
mod png;
mod svg;
mod util;

// A-4: explicit error enum for the library crate, replacing anyhow bail. Downstream
// (fd-cli report_error) can downcast and match on variants for differentiated messages
// (fixes E-9). io/serde are direct dependency types, so use #[from];
// usvg/png/printpdf errors are funneled through RenderFailed(String) to avoid exposing
// those error types in the public API (png/printpdf are not direct deps of this crate;
// cross-crate exposure would increase downstream coupling).
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("render failed: {0}")]
    RenderFailed(String),
    #[error("unsupported export format: {0:?}")]
    UnsupportedFormat(ExportFormat),
    #[error("batch export had {count} failure(s):\n{detail}")]
    BatchPartial { count: usize, detail: String },
}

// Max canvas edge in pixels when rasterizing (PNG), to prevent malicious .fusiondesign files from triggering OOM.
const MAX_CANVAS_DIM: u32 = 16384;
// Max total canvas pixels when rasterizing (PNG) (RGBA4 = 64M px x 4 B = 256 MB).
// An edge-only limit is not enough: 16384^2 x 4 = 1 GB would still OOM. R-A16.
const MAX_CANVAS_PIXELS: u64 = 64_000_000;

/// Export format.
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

/// Canvas page data (minimal subset, used for export rendering).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanvasPage {
    pub id: String,
    pub name: String,
    pub width: f64,
    pub height: f64,
    pub elements: Vec<CanvasElement>,
}

/// Vector element (V0.2 extension: full NodeStyle fields).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanvasElement {
    pub kind: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub fill: Option<String>,
    #[serde(default)]
    pub stroke: Option<String>,
    #[serde(default)]
    pub stroke_width: Option<f64>,
    #[serde(default)]
    pub radius: Option<f64>,
    #[serde(default)]
    pub opacity: Option<f64>,
    #[serde(default)]
    pub font_size: Option<f64>,
    #[serde(default)]
    pub font_family: Option<String>,
    #[serde(default)]
    pub rotation: Option<f64>,
}

impl CanvasPage {
    /// Convert from an fd-canvas-core Page.
    pub fn from_page(page: &fd_canvas_core::Page) -> Self {
        let mut elements = Vec::new();
        for node in &page.nodes {
            util::collect_elements(node, &mut elements);
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

/// Exporter.
pub struct Exporter;

impl Exporter {
    /// Export all pages directly from a PenDocument.
    pub fn from_pen_document(
        doc: &fd_canvas_core::PenDocument,
        format: ExportFormat,
        out_dir: &Path,
    ) -> Result<Vec<PathBuf>, ExportError> {
        // PERF-4: per-page convert+export+drop, to avoid collecting all CanvasPages at once
        // and inflating peak memory. For large multi-page documents (validate_limits caps
        // at 100k nodes), the old collect kept every page resident simultaneously.
        let mut files = Vec::with_capacity(doc.pages.len());
        let mut errors: Vec<String> = Vec::new();
        for page in &doc.pages {
            let canvas_page = CanvasPage::from_page(page);
            match Self::export_page(&canvas_page, format, out_dir) {
                Ok(f) => files.push(f),
                Err(e) => errors.push(format!("page '{}' export failed: {e}", canvas_page.name)),
            }
            // canvas_page is dropped at the end of each iteration, so the next page does not stack on the previous page's memory.
        }
        if !errors.is_empty() {
            tracing::error!(
                count = errors.len(),
                "from_pen_document: some pages failed to export"
            );
            return Err(ExportError::BatchPartial {
                count: errors.len(),
                detail: errors.join("\n"),
            });
        }
        Ok(files)
    }

    /// Export from a PenDocument, resolving `var(--token)` / `token:` color references against
    /// the currently active design system first.
    /// usvg does not support CSS Custom Properties; unresolved vars fall back to black at rasterization (#8).
    pub fn from_pen_document_with_tokens(
        doc: &fd_canvas_core::PenDocument,
        format: ExportFormat,
        out_dir: &Path,
        reg: &fd_design_system::DesignSystemRegistry,
    ) -> Result<Vec<PathBuf>, ExportError> {
        // PERF-4: per-page convert+token-resolve+export+drop, to avoid collecting all CanvasPages at once and inflating peak memory.
        let mut files = Vec::with_capacity(doc.pages.len());
        let mut errors: Vec<String> = Vec::new();
        for page in &doc.pages {
            let mut canvas_page = CanvasPage::from_page(page);
            util::resolve_page_token_vars(&mut canvas_page, reg);
            match Self::export_page(&canvas_page, format, out_dir) {
                Ok(f) => files.push(f),
                Err(e) => errors.push(format!("page '{}' export failed: {e}", canvas_page.name)),
            }
        }
        if !errors.is_empty() {
            tracing::error!(
                count = errors.len(),
                "from_pen_document_with_tokens: some pages failed to export"
            );
            return Err(ExportError::BatchPartial {
                count: errors.len(),
                detail: errors.join("\n"),
            });
        }
        Ok(files)
    }

    /// Export a single page in the given format.
    pub fn export_page(
        page: &CanvasPage,
        format: ExportFormat,
        out_dir: &Path,
    ) -> Result<PathBuf, ExportError> {
        std::fs::create_dir_all(out_dir)?;
        let filename = format!(
            "{}.{}",
            util::sanitize_filename(&page.name),
            format.extension()
        );
        let file = out_dir.join(&filename);
        match format {
            ExportFormat::Png => png::render_png(page, &file)?,
            ExportFormat::Pdf => pdf::render_pdf(page, &file)?,
            _ => {
                let content = match format {
                    ExportFormat::Html => html::render_html(page),
                    ExportFormat::Svg => svg::render_svg(page),
                    ExportFormat::Json => serde_json::to_string_pretty(page)?,
                    other => {
                        tracing::error!(format = ?other, "export_page: unsupported export format");
                        return Err(ExportError::UnsupportedFormat(other));
                    }
                };
                std::fs::write(&file, content)?;
            }
        }
        tracing::info!(?file, format = format.extension(), "page exported");
        Ok(file)
    }

    /// Batch export multiple pages.
    pub fn export_batch(
        pages: &[CanvasPage],
        format: ExportFormat,
        out_dir: &Path,
    ) -> Result<Vec<PathBuf>, ExportError> {
        // L-12: batch export is not atomic — the old impl used per-page `?`, so a mid-way
        // failure left some pages exported with no summary. Now attempt all pages, collect
        // errors, and fail visibly with a summary if any fail (already-exported files stay;
        // the caller decides retry/cleanup based on errors).
        let mut files = Vec::with_capacity(pages.len());
        let mut errors: Vec<String> = Vec::new();
        for page in pages {
            match Self::export_page(page, format, out_dir) {
                Ok(f) => files.push(f),
                Err(e) => errors.push(format!("page '{}' export failed: {e}", page.name)),
            }
        }
        if !errors.is_empty() {
            tracing::error!(
                count = errors.len(),
                "export_batch: some pages failed to export"
            );
            return Err(ExportError::BatchPartial {
                count: errors.len(),
                detail: errors.join("\n"),
            });
        }
        Ok(files)
    }

    /// Async batch export (for task queue callers).
    pub async fn export_batch_async(
        pages: Vec<CanvasPage>,
        format: ExportFormat,
        out_dir: PathBuf,
    ) -> Result<Vec<PathBuf>, ExportError> {
        tokio::task::spawn_blocking(move || Exporter::export_batch(&pages, format, &out_dir))
            .await
            .map_err(|e| ExportError::RenderFailed(format!("blocking task failed: {e}")))?
    }

    /// Async export from a PenDocument.
    pub async fn from_pen_document_async(
        doc: fd_canvas_core::PenDocument,
        format: ExportFormat,
        out_dir: PathBuf,
    ) -> Result<Vec<PathBuf>, ExportError> {
        tokio::task::spawn_blocking(move || Self::from_pen_document(&doc, format, &out_dir))
            .await
            .map_err(|e| ExportError::RenderFailed(format!("blocking task failed: {e}")))?
    }
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
                    x: 0.0,
                    y: 0.0,
                    w: 50.0,
                    h: 50.0,
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
                    x: 10.0,
                    y: 20.0,
                    w: 0.0,
                    h: 0.0,
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

    // E-21 regression: a page.name containing </title><script> inside <title> must be escaped, not executed.
    #[test]
    fn export_html_escapes_title_xss() {
        let tmp = tempdir().unwrap();
        let page = CanvasPage {
            id: "xss".into(),
            name: "</title><script>alert(1)</script>".into(),
            width: 100.0,
            height: 100.0,
            elements: vec![],
        };
        let file = Exporter::export_page(&page, ExportFormat::Html, tmp.path()).unwrap();
        let content = std::fs::read_to_string(&file).unwrap();
        assert!(
            !content.contains("<script>alert(1)</script>"),
            "raw <script> must not survive"
        );
        assert!(
            content.contains("&lt;script&gt;"),
            "must be escaped to entities"
        );
        assert!(
            content.contains("&lt;/title&gt;"),
            "</title> must be escaped"
        );
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
        assert!(
            data.starts_with(&[0x89, 0x50, 0x4E, 0x47]),
            "PNG magic bytes"
        );
    }

    #[test]
    fn export_pdf_writes_file() {
        let tmp = tempdir().unwrap();
        let page = sample_page();
        let file = Exporter::export_page(&page, ExportFormat::Pdf, tmp.path()).unwrap();
        let data = std::fs::read(&file).unwrap();
        assert!(data.starts_with(b"%PDF"), "PDF magic bytes");
    }

    // R-A15 regression: rect/circle must actually be drawn into the PDF, not emit text only and produce a blank page.
    fn pdf_content(page: &CanvasPage, dir: &Path) -> String {
        let file = Exporter::export_page(page, ExportFormat::Pdf, dir).unwrap();
        let data = std::fs::read(&file).unwrap();
        String::from_utf8_lossy(&data).to_string()
    }

    #[test]
    fn render_pdf_rect_emits_rectangle_op() {
        let tmp = tempdir().unwrap();
        let rect_page = CanvasPage {
            id: "r".into(),
            name: "R".into(),
            width: 100.0,
            height: 100.0,
            elements: vec![CanvasElement {
                kind: "rect".into(),
                x: 10.0,
                y: 10.0,
                w: 40.0,
                h: 40.0,
                fill: Some("#ff0000".into()),
                stroke: None,
                stroke_width: None,
                text: None,
                radius: None,
                opacity: None,
                font_size: None,
                font_family: None,
                rotation: None,
            }],
        };
        let empty = CanvasPage {
            id: "e".into(),
            name: "E".into(),
            width: 100.0,
            height: 100.0,
            elements: vec![],
        };
        let with_rect = pdf_content(&rect_page, tmp.path());
        let empty_pdf = pdf_content(&empty, tmp.path());
        assert!(
            with_rect.contains(" re"),
            "rect must emit the PDF re rectangle operator"
        );
        assert!(
            with_rect.contains(" rg"),
            "fill #ff0000 must emit the rg fill color operator"
        );
        assert!(
            !empty_pdf.contains(" re"),
            "empty page must not contain the re rectangle operator (control baseline)"
        );
    }

    #[test]
    fn render_pdf_circle_emits_fill_color_op() {
        let tmp = tempdir().unwrap();
        let circle_page = CanvasPage {
            id: "c".into(),
            name: "C".into(),
            width: 100.0,
            height: 100.0,
            elements: vec![CanvasElement {
                kind: "circle".into(),
                x: 10.0,
                y: 10.0,
                w: 40.0,
                h: 40.0,
                fill: Some("#00ff00".into()),
                stroke: Some("#000000".into()),
                stroke_width: Some(1.0),
                text: None,
                radius: None,
                opacity: None,
                font_size: None,
                font_family: None,
                rotation: None,
            }],
        };
        let with_circle = pdf_content(&circle_page, tmp.path());
        assert!(
            with_circle.contains(" rg"),
            "circle fill must emit the rg fill color operator"
        );
        assert!(
            with_circle.contains(" RG"),
            "circle stroke must emit the RG stroke color operator"
        );
    }

    #[test]
    fn render_pdf_hex_color_invalid_skipped_not_panic() {
        let tmp = tempdir().unwrap();
        let page = CanvasPage {
            id: "x".into(),
            name: "X".into(),
            width: 100.0,
            height: 100.0,
            elements: vec![CanvasElement {
                kind: "rect".into(),
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
                fill: Some("not-a-color".into()),
                stroke: Some("#zzz".into()),
                stroke_width: Some(2.0),
                text: None,
                radius: None,
                opacity: None,
                font_size: None,
                font_family: None,
                rotation: None,
            }],
        };
        let content = pdf_content(&page, tmp.path());
        assert!(
            content.starts_with("%PDF"),
            "invalid color must not panic, still produce a valid PDF"
        );
    }

    // R-10: CJK text detection.
    #[test]
    fn text_has_cjk_detects_chinese() {
        assert!(pdf::text_has_cjk("登录页面"));
        assert!(pdf::text_has_cjk("hello 世界"));
        assert!(pdf::text_has_cjk("全角　空格"));
        assert!(!pdf::text_has_cjk("plain ascii"));
        assert!(!pdf::text_has_cjk(""));
        assert!(!pdf::text_has_cjk("café résumé"));
    }

    // R-10: pages containing CJK text need an embedded font; pure Latin skips embedding (avoiding needless overhead).
    #[test]
    fn page_has_cjk_text_detection() {
        let cjk_page = CanvasPage {
            id: "c".into(),
            name: "C".into(),
            width: 100.0,
            height: 100.0,
            elements: vec![CanvasElement {
                kind: "text".into(),
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 20.0,
                fill: None,
                stroke: None,
                stroke_width: None,
                text: Some("你好".into()),
                radius: None,
                opacity: None,
                font_size: Some(12.0),
                font_family: None,
                rotation: None,
            }],
        };
        assert!(pdf::page_has_cjk_text(&cjk_page));

        let latin_only = CanvasPage {
            id: "l".into(),
            name: "L".into(),
            width: 100.0,
            height: 100.0,
            elements: vec![CanvasElement {
                kind: "text".into(),
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 20.0,
                fill: None,
                stroke: None,
                stroke_width: None,
                text: Some("login".into()),
                radius: None,
                opacity: None,
                font_size: Some(12.0),
                font_family: None,
                rotation: None,
            }],
        };
        assert!(!pdf::page_has_cjk_text(&latin_only));
    }

    // R-10: PDF with CJK text stays valid (embeds PingFang when present on the machine, degrades without it but never panics).
    // Deliberately not asserting visible glyphs (depends on real machine fonts), only that it does not crash and yields a valid PDF.
    #[test]
    fn render_pdf_cjk_text_produces_valid_pdf() {
        let tmp = tempdir().unwrap();
        let page = CanvasPage {
            id: "cjk".into(),
            name: "中文页".into(),
            width: 200.0,
            height: 100.0,
            elements: vec![CanvasElement {
                kind: "text".into(),
                x: 10.0,
                y: 10.0,
                w: 180.0,
                h: 20.0,
                fill: None,
                stroke: None,
                stroke_width: None,
                text: Some("登录页面".into()),
                radius: None,
                opacity: None,
                font_size: Some(14.0),
                font_family: None,
                rotation: None,
            }],
        };
        let content = pdf_content(&page, tmp.path());
        assert!(
            content.starts_with("%PDF"),
            "CJK text must produce a valid PDF"
        );
    }

    // FUNC-9: rotating a shape must not lose the rotation on PDF export. The current impl
    // rotates with printpdf CTM (SetTransformationMatrix/TranslateRotate) inside the
    // SaveGraphicsState scope.
    // In printpdf 0.12 serialize.rs the cm operator is uncompressed (doc.compress() is
    // commented out), so we can check for the " cm" operator directly in the PDF bytes to
    // verify the rotation matrix was written.
    // The pivot is the shape center (aligned with the SVG path transform="rotate(r cx cy)" for visual consistency).
    #[test]
    fn pdf_export_rotated_shape_not_skipped() {
        let tmp = tempdir().unwrap();
        let page = CanvasPage {
            id: "rot".into(),
            name: "Rot".into(),
            width: 100.0,
            height: 100.0,
            elements: vec![CanvasElement {
                kind: "rect".into(),
                x: 10.0,
                y: 10.0,
                w: 40.0,
                h: 40.0,
                text: None,
                fill: Some("#ff0000".into()),
                stroke: None,
                stroke_width: None,
                radius: None,
                opacity: None,
                font_size: None,
                font_family: None,
                rotation: Some(45.0),
            }],
        };
        let file = Exporter::export_page(&page, ExportFormat::Pdf, tmp.path()).unwrap();
        assert!(file.exists(), "PDF file should have been generated");
        let data = std::fs::read(&file).unwrap();
        assert!(data.starts_with(b"%PDF"), "PDF magic bytes");
        assert!(
            data.len() > 1000,
            "rotated rect PDF should be non-trivial in size (>1000B), got {}B",
            data.len()
        );
        let content = String::from_utf8_lossy(&data);
        assert!(
            content.contains(" re"),
            "rotated rect must still emit the re rectangle operator (not skipped)"
        );
        assert!(
            content.contains(" cm"),
            "rotated shape must emit the cm CTM operator (FUNC-9 rotation matrix written)"
        );
        assert!(
            content.contains(" rg"),
            "fill #ff0000 must emit the rg fill color operator"
        );
    }

    // R-11: skipped unsupported element kinds are no longer silent — the PDF is still produced but the element is missing.
    // Verifies unknown kinds do not panic and yield a valid PDF (skip list is logged via tracing warn).
    #[test]
    fn render_pdf_skips_unsupported_kind_safely() {
        let tmp = tempdir().unwrap();
        let page = CanvasPage {
            id: "s".into(),
            name: "S".into(),
            width: 100.0,
            height: 100.0,
            elements: vec![
                CanvasElement {
                    kind: "image".into(),
                    x: 0.0,
                    y: 0.0,
                    w: 50.0,
                    h: 50.0,
                    fill: None,
                    stroke: None,
                    stroke_width: None,
                    text: None,
                    radius: None,
                    opacity: None,
                    font_size: None,
                    font_family: None,
                    rotation: None,
                },
                CanvasElement {
                    kind: "rect".into(),
                    x: 0.0,
                    y: 0.0,
                    w: 10.0,
                    h: 10.0,
                    fill: Some("#000000".into()),
                    stroke: None,
                    stroke_width: None,
                    text: None,
                    radius: None,
                    opacity: None,
                    font_size: None,
                    font_family: None,
                    rotation: None,
                },
            ],
        };
        let content = pdf_content(&page, tmp.path());
        assert!(
            content.starts_with("%PDF"),
            "page with unsupported elements must still produce a valid PDF"
        );
        assert!(
            content.contains(" re"),
            "supported rect must still export normally"
        );
    }

    // R-A16 regression: exceeding the total pixel limit (edge within limit) must refuse to render, preventing a 1GB OOM.
    // 9000x9000=81M px > 64M, but each edge < 16384, so only the total-pixel gate can block it.
    #[test]
    fn render_png_rejects_total_pixels_over_limit() {
        let tmp = tempdir().unwrap();
        let huge = CanvasPage {
            id: "h".into(),
            name: "H".into(),
            width: 9000.0,
            height: 9000.0,
            elements: vec![],
        };
        let res = Exporter::export_page(&huge, ExportFormat::Png, tmp.path());
        let err = res.expect_err("9000x9000=81M px > 64M must be rejected by the total-pixel gate without allocating 1GB");
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("total") || msg.contains("exceed"),
            "should report total pixels over limit, got: {msg}"
        );
    }

    #[test]
    fn export_batch_multiple_pages() {
        let tmp = tempdir().unwrap();
        let pages = vec![
            sample_page(),
            CanvasPage {
                id: "p2".into(),
                name: "Second".into(),
                width: 200.0,
                height: 200.0,
                elements: vec![],
            },
        ];
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
        let files =
            Exporter::export_batch_async(pages, ExportFormat::Json, tmp.path().to_path_buf())
                .await
                .unwrap();
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn render_element_svg_unknown_kind_commented() {
        let el = CanvasElement {
            kind: "weird".into(),
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
            text: None,
            fill: None,
            stroke: None,
            stroke_width: None,
            radius: None,
            opacity: None,
            font_size: None,
            font_family: None,
            rotation: None,
        };
        let svg = svg::render_element_svg(&el);
        assert!(svg.contains("unknown element kind"));
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
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
                text: None,
                fill: Some("#F00".into()),
                stroke: None,
                stroke_width: None,
                radius: None,
                opacity: None,
                font_size: None,
                font_family: None,
                rotation: None,
            }],
        };
        let svg = svg::render_svg(&page);
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
    fn from_pen_document_multi_page_streams() {
        // PERF-4: multi-page streaming export, each page producing an independent file, no full-collect peak.
        // Regression lock: multi-page from_pen_document must export every page one by one (no pages dropped).
        let tmp = tempdir().unwrap();
        let mut doc = fd_canvas_core::PenDocument::new();
        for i in 0..5 {
            let mut page =
                fd_canvas_core::Page::new(format!("p{i}"), format!("Page{i}"), 100.0, 100.0);
            page.add(fd_canvas_core::PenNode::text(
                "n",
                10.0,
                10.0,
                format!("mark{i}"),
            ));
            doc.add_page(page);
        }
        let files = Exporter::from_pen_document(&doc, ExportFormat::Svg, tmp.path()).unwrap();
        assert_eq!(files.len(), 5, "all 5 pages must be exported");
        for (i, f) in files.iter().enumerate() {
            let content = std::fs::read_to_string(f).unwrap();
            assert!(
                content.contains(&format!("mark{i}")),
                "page {i} content must match: {content}"
            );
        }
    }

    #[test]
    fn sanitize_filename_strips_special() {
        assert_eq!(util::sanitize_filename("Hello/World"), "Hello_World");
        assert_eq!(util::sanitize_filename("Page 1"), "Page_1");
        assert_eq!(util::sanitize_filename("ok-name_123"), "ok-name_123");
    }

    #[test]
    fn sanitize_filename_empty_defaults_to_page() {
        // E-20/P3: empty name / all-illegal chars must fall back to "page", never produce a ".svg" hidden file.
        assert_eq!(util::sanitize_filename(""), "page");
        assert_eq!(util::sanitize_filename("   "), "___"); // 3 spaces -> 3 underscores, non-empty
        assert_eq!(util::sanitize_filename("///"), "___"); // 3 slashes -> 3 underscores, non-empty
        assert_eq!(util::sanitize_filename("!!!"), "___"); // 3 exclamation marks -> 3 underscores, non-empty
    }

    #[test]
    fn export_empty_name_page_not_hidden_file() {
        // E-20/P3: empty-name page must export as "page.svg", not ".svg" (hidden file).
        let tmp = tempdir().unwrap();
        let page = CanvasPage {
            id: "p1".into(),
            name: "".into(),
            width: 100.0,
            height: 100.0,
            elements: vec![],
        };
        Exporter::export_batch(&[page], ExportFormat::Svg, tmp.path()).unwrap();
        let files: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            files.iter().any(|f| f == "page.svg"),
            "empty name must export page.svg, not .svg: {files:?}"
        );
        assert!(
            !files.iter().any(|f| f == ".svg"),
            "must not produce a hidden .svg: {files:?}"
        );
    }

    #[test]
    fn xml_escape_handles_special() {
        assert_eq!(util::xml_escape("<b>bold</b>"), "&lt;b&gt;bold&lt;/b&gt;");
        assert_eq!(util::xml_escape("a&b"), "a&amp;b");
        assert_eq!(util::xml_escape("he said \"hi\""), "he said &quot;hi&quot;");
    }

    #[test]
    fn svg_attribute_injection_escaped() {
        let el = CanvasElement {
            kind: "rect".into(),
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            fill: Some("red\" onclick=\"alert(1)".into()),
            stroke: Some("blue\" onload=\"evil()".into()),
            stroke_width: None,
            radius: None,
            opacity: None,
            rotation: None,
            text: None,
            font_size: None,
            font_family: None,
        };
        let svg = svg::render_element_svg(&el);
        assert!(
            !svg.contains("\" onclick=\""),
            "fill should not create new attribute: {svg}"
        );
        assert!(
            !svg.contains("\" onload=\""),
            "stroke should not create new attribute: {svg}"
        );
        assert!(
            svg.contains("fill=\"red&quot;"),
            "fill quotes should be escaped: {svg}"
        );
        assert!(
            svg.contains("stroke=\"blue&quot;"),
            "stroke quotes should be escaped: {svg}"
        );
    }

    #[test]
    fn rotation_in_svg() {
        let el = CanvasElement {
            kind: "rect".into(),
            x: 10.0,
            y: 20.0,
            w: 50.0,
            h: 30.0,
            text: None,
            fill: Some("#000".into()),
            stroke: None,
            stroke_width: None,
            radius: None,
            opacity: None,
            font_size: None,
            font_family: None,
            rotation: Some(45.0),
        };
        let svg = svg::render_element_svg(&el);
        assert!(svg.contains("rotate(45"));
    }

    #[test]
    fn rotation_in_svg_text() {
        // FUNC-9: the text arm was originally a separate branch that did not consume attrs, dropping rotation. After the fix it must carry transform.
        let el = CanvasElement {
            kind: "text".into(),
            x: 10.0,
            y: 20.0,
            w: 50.0,
            h: 30.0,
            text: Some("hello".into()),
            fill: Some("#000".into()),
            stroke: None,
            stroke_width: None,
            radius: None,
            opacity: None,
            font_size: Some(14.0),
            font_family: None,
            rotation: Some(30.0),
        };
        let svg = svg::render_element_svg(&el);
        assert!(
            svg.contains("rotate(30"),
            "rotated text must output transform: {svg}"
        );
        assert!(
            svg.contains("hello"),
            "text content must be preserved: {svg}"
        );
    }

    #[test]
    fn rotation_zero_svg_text_no_transform() {
        // rotation=0 or None must not output transform (avoiding a meaningless rotation attribute).
        let mut el = CanvasElement {
            kind: "text".into(),
            x: 10.0,
            y: 20.0,
            w: 50.0,
            h: 30.0,
            text: Some("hi".into()),
            fill: Some("#000".into()),
            stroke: None,
            stroke_width: None,
            radius: None,
            opacity: None,
            font_size: Some(12.0),
            font_family: None,
            rotation: Some(0.0),
        };
        let svg = svg::render_element_svg(&el);
        assert!(
            !svg.contains("transform"),
            "rotation=0 must not output transform: {svg}"
        );
        el.rotation = None;
        let svg = svg::render_element_svg(&el);
        assert!(
            !svg.contains("transform"),
            "rotation=None must not output transform: {svg}"
        );
    }

    #[test]
    fn nested_children_flattened() {
        let mut doc = fd_canvas_core::PenDocument::new();
        let mut page = fd_canvas_core::Page::new("p1", "Nested", 100.0, 100.0);
        let group = fd_canvas_core::PenNode::group(
            "g1",
            0.0,
            0.0,
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

    fn apple_hig_registry() -> fd_design_system::DesignSystemRegistry {
        let mut reg = fd_design_system::DesignSystemRegistry::new();
        reg.register_builtin();
        reg.activate("apple-hig").unwrap();
        reg
    }

    #[test]
    fn resolve_color_var_dot_form() {
        // to_css_value() produces the dot form var(--color.accent)
        let reg = apple_hig_registry();
        let v = util::resolve_color_var(&Some("var(--color.accent)".into()), &reg);
        assert_eq!(v.as_deref(), Some("#007AFF"));
    }

    #[test]
    fn resolve_color_var_dash_form() {
        // parse-html produces the dash form var(--color-accent)
        let reg = apple_hig_registry();
        let v = util::resolve_color_var(&Some("var(--color-accent)".into()), &reg);
        assert_eq!(v.as_deref(), Some("#007AFF"));
    }

    #[test]
    fn resolve_color_var_token_prefix() {
        let reg = apple_hig_registry();
        let v = util::resolve_color_var(&Some("token:color.accent".into()), &reg);
        assert_eq!(v.as_deref(), Some("#007AFF"));
    }

    #[test]
    fn resolve_color_var_passthrough_plain() {
        let reg = apple_hig_registry();
        assert_eq!(
            util::resolve_color_var(&Some("#FF8800".into()), &reg).as_deref(),
            Some("#FF8800")
        );
        assert_eq!(util::resolve_color_var(&None, &reg), None);
    }

    #[test]
    fn from_pen_document_with_tokens_resolves_svg() {
        // usvg cannot parse var(--); it must be replaced with the actual color value before export (#8)
        let mut doc = fd_canvas_core::PenDocument::new();
        let mut page = fd_canvas_core::Page::new("p1", "Token", 100.0, 100.0);
        let mut n = fd_canvas_core::PenNode::rect("n1", 0.0, 0.0, 50.0, 50.0);
        n.style.fill = Some("var(--color-accent)".into());
        page.add(n);
        doc.add_page(page);
        let reg = apple_hig_registry();
        let tmp = tempdir().unwrap();
        let files =
            Exporter::from_pen_document_with_tokens(&doc, ExportFormat::Svg, tmp.path(), &reg)
                .unwrap();
        let svg = std::fs::read_to_string(&files[0]).unwrap();
        assert!(
            svg.contains("fill=\"#007AFF\""),
            "SVG must contain the resolved actual color value: {svg}"
        );
        assert!(
            !svg.contains("var(--"),
            "SVG must not retain unresolved CSS variables: {svg}"
        );
    }

    #[test]
    fn from_pen_document_with_tokens_png_resolved() {
        // Token-driven design export to PNG: colors are resolved, no longer falling back to black
        let mut doc = fd_canvas_core::PenDocument::new();
        let mut page = fd_canvas_core::Page::new("p1", "Png", 40.0, 40.0);
        let mut n = fd_canvas_core::PenNode::rect("n1", 0.0, 0.0, 40.0, 40.0);
        n.style.fill = Some("var(--color-accent)".into());
        page.add(n);
        doc.add_page(page);
        let reg = apple_hig_registry();
        let tmp = tempdir().unwrap();
        let files =
            Exporter::from_pen_document_with_tokens(&doc, ExportFormat::Png, tmp.path(), &reg)
                .unwrap();
        let data = std::fs::read(&files[0]).unwrap();
        assert!(
            data.starts_with(&[0x89, 0x50, 0x4E, 0x47]),
            "PNG magic bytes"
        );
    }

    #[test]
    fn sanitize_image_url_whitelist() {
        // R-5: data:image/* allowed, relative paths allowed
        assert_eq!(
            util::sanitize_image_url("data:image/png;base64,xxx"),
            "data:image/png;base64,xxx"
        );
        assert_eq!(
            util::sanitize_image_url("assets/logo.svg"),
            "assets/logo.svg"
        );
        assert_eq!(util::sanitize_image_url("photo.jpg"), "photo.jpg");
        assert_eq!(util::sanitize_image_url(""), "");
    }

    #[test]
    fn sanitize_image_url_rejects_executable_and_remote() {
        // R-5: javascript/data:text/html/http(s)/file all rejected (returns empty string)
        assert_eq!(util::sanitize_image_url("javascript:alert(1)"), "");
        assert_eq!(
            util::sanitize_image_url("data:text/html,<script>alert(1)</script>"),
            ""
        );
        assert_eq!(util::sanitize_image_url("https://evil.com/x.png"), "");
        assert_eq!(util::sanitize_image_url("http://10.0.0.1/exfil.png"), "");
        assert_eq!(util::sanitize_image_url("file:///etc/passwd"), "");
    }

    #[test]
    fn sanitize_image_url_rejects_dotdot_traversal() {
        assert_eq!(util::sanitize_image_url("../secret"), "");
        assert_eq!(util::sanitize_image_url("a/../b"), "");
        assert_eq!(util::sanitize_image_url("../../etc/passwd"), "");
    }
    #[test]
    fn sanitize_image_url_allows_normal_relative() {
        assert_eq!(util::sanitize_image_url("logo.png"), "logo.png");
        assert_eq!(
            util::sanitize_image_url("assets/icon.svg"),
            "assets/icon.svg"
        );
        assert_eq!(
            util::sanitize_image_url("data:image/png;base64,xxx"),
            "data:image/png;base64,xxx"
        );
    }

    #[test]
    fn svg_image_href_injection_blocked() {
        // R-5 end-to-end: a malicious href on an image element is intercepted by sanitize_image_url
        let el = CanvasElement {
            kind: "image".into(),
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            text: Some("javascript:alert(1)".into()),
            fill: None,
            stroke: None,
            stroke_width: None,
            radius: None,
            opacity: None,
            font_size: None,
            font_family: None,
            rotation: None,
        };
        let svg = svg::render_element_svg(&el);
        assert!(
            !svg.contains("javascript:"),
            "javascript: href must be stripped: {svg}"
        );
        assert!(
            svg.contains("href=\"\""),
            "blocked href should be empty: {svg}"
        );
    }
}
