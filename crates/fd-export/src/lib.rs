// Callers: fd-cli (export/export-batch), DesignBridge (Swift Process)
// Affected API: Exporter::from_pen_document(), CanvasPage::from_page(), CanvasElement new fields
// Data schemas: PenDocument→CanvasPage bridge, enhanced SVG with NodeStyle (stroke_width/radius/opacity/font/rotation)
// User instruction: "现在开始实施" — Task #16 P3-5 fd-export PNG/SVG/HTML 批量导出
//
// ARCH-10 r4：god-file 拆分。lib.rs 仅留公共类型（ExportError/ExportFormat/CanvasPage/
// CanvasElement）+ Exporter 实现 + 常量。渲染逻辑按格式拆 svg/html/png/pdf 模块，
// 共享工具（文件名清洗/xml 转义/token 解析/元素收集）入 util。纯位置迁移，零行为变更。

//! Fusion-Design 导出 — PNG/SVG/PDF/HTML 批量导出。
//!
//! 对应 PRD 模块 5「原型交互与交付」的导出能力。
//! 支持格式：HTML（静态）、SVG（矢量）、PNG（tiny-skia 光栅化）、PDF（结构化）、JSON（工程文件）。
//!
//! V0.2: 支持 PenDocument 直接导出（无需手动转 CanvasPage），
//! 增强渲染：NodeStyle（fill/stroke/radius/opacity/font）→ SVG 属性。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod html;
mod pdf;
mod png;
mod svg;
mod util;

// A-4：库 crate 显式错误枚举，替代 anyhow bail。下游（fd-cli report_error）
// 可 downcast 按变体 match 做差异化提示（修 E-9）。io/serde 是直接依赖类型用 #[from]；
// usvg/png/printpdf 错误经 RenderFailed(String) 收口，避免把这俩错误类型暴露到公共 API
// （png/printpdf 非本 crate 直接依赖，跨 crate 暴露会增加下游耦合）。
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("序列化错误: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("渲染失败: {0}")]
    RenderFailed(String),
    #[error("不支持的导出格式: {0:?}")]
    UnsupportedFormat(ExportFormat),
    #[error("批量导出存在 {count} 项失败:\n{detail}")]
    BatchPartial { count: usize, detail: String },
}

// 渲染光栅图（PNG）时画布单边像素上限，防止恶意 .fusiondesign 触发 OOM。
const MAX_CANVAS_DIM: u32 = 16384;
// 渲染光栅图（PNG）时画布总像素上限（RGBA4 = 64M px × 4 B = 256 MB）。
// 单边限制不够：16384²×4 = 1 GB 仍会 OOM。R-A16。
const MAX_CANVAS_PIXELS: u64 = 64_000_000;

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
    pub width: f64,
    pub height: f64,
    pub elements: Vec<CanvasElement>,
}

/// 矢量元素（V0.2 扩展：NodeStyle 全字段）。
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
    /// 从 fd-canvas-core Page 转换。
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

/// 导出器。
pub struct Exporter;

impl Exporter {
    /// 从 PenDocument 直接导出所有页面。
    pub fn from_pen_document(
        doc: &fd_canvas_core::PenDocument,
        format: ExportFormat,
        out_dir: &Path,
    ) -> Result<Vec<PathBuf>, ExportError> {
        // PERF-4：逐页转换+导出+释放，避免一次性 collect 全部 CanvasPage 占峰值内存。
        // 大文档（validate_limits 上限 100k 节点）多页时，旧 collect 把所有页同时驻留。
        let mut files = Vec::with_capacity(doc.pages.len());
        let mut errors: Vec<String> = Vec::new();
        for page in &doc.pages {
            let canvas_page = CanvasPage::from_page(page);
            match Self::export_page(&canvas_page, format, out_dir) {
                Ok(f) => files.push(f),
                Err(e) => errors.push(format!("页面 '{}' 导出失败: {e}", canvas_page.name)),
            }
            // canvas_page 循环末释放，下一页不叠加上一页内存。
        }
        if !errors.is_empty() {
            tracing::error!(count = errors.len(), "from_pen_document: 部分页面导出失败");
            return Err(ExportError::BatchPartial {
                count: errors.len(),
                detail: errors.join("\n"),
            });
        }
        Ok(files)
    }

    /// 从 PenDocument 导出，导出前用当前激活设计规范解析 `var(--token)` / `token:` 颜色引用。
    /// usvg 不支持 CSS Custom Property，未解析会在光栅化阶段回退黑色（#8）。
    pub fn from_pen_document_with_tokens(
        doc: &fd_canvas_core::PenDocument,
        format: ExportFormat,
        out_dir: &Path,
        reg: &fd_design_system::DesignSystemRegistry,
    ) -> Result<Vec<PathBuf>, ExportError> {
        // PERF-4：逐页转换+token 解析+导出+释放，避免一次性 collect 全部 CanvasPage 占峰值内存。
        let mut files = Vec::with_capacity(doc.pages.len());
        let mut errors: Vec<String> = Vec::new();
        for page in &doc.pages {
            let mut canvas_page = CanvasPage::from_page(page);
            util::resolve_page_token_vars(&mut canvas_page, reg);
            match Self::export_page(&canvas_page, format, out_dir) {
                Ok(f) => files.push(f),
                Err(e) => errors.push(format!("页面 '{}' 导出失败: {e}", canvas_page.name)),
            }
        }
        if !errors.is_empty() {
            tracing::error!(
                count = errors.len(),
                "from_pen_document_with_tokens: 部分页面导出失败"
            );
            return Err(ExportError::BatchPartial {
                count: errors.len(),
                detail: errors.join("\n"),
            });
        }
        Ok(files)
    }

    /// 导出单页到指定格式。
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
                        tracing::error!(format = ?other, "export_page 不支持的导出格式");
                        return Err(ExportError::UnsupportedFormat(other));
                    }
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
    ) -> Result<Vec<PathBuf>, ExportError> {
        // L-12：批导出非原子——旧实现逐页 `?`，中途失败已导出部分页且无汇总。
        // 改为全部尝试，收集错误，任一失败则 fail visibly 汇总（已导出文件保留，
        // 调用方据 errors 决定重试/清理）。
        let mut files = Vec::with_capacity(pages.len());
        let mut errors: Vec<String> = Vec::new();
        for page in pages {
            match Self::export_page(page, format, out_dir) {
                Ok(f) => files.push(f),
                Err(e) => errors.push(format!("页面 '{}' 导出失败: {e}", page.name)),
            }
        }
        if !errors.is_empty() {
            tracing::error!(count = errors.len(), "export_batch: 部分页面导出失败");
            return Err(ExportError::BatchPartial {
                count: errors.len(),
                detail: errors.join("\n"),
            });
        }
        Ok(files)
    }

    /// 异步批量导出（供任务队列调用）。
    pub async fn export_batch_async(
        pages: Vec<CanvasPage>,
        format: ExportFormat,
        out_dir: PathBuf,
    ) -> Result<Vec<PathBuf>, ExportError> {
        tokio::task::spawn_blocking(move || Exporter::export_batch(&pages, format, &out_dir))
            .await
            .map_err(|e| ExportError::RenderFailed(format!("阻塞任务失败: {e}")))?
    }

    /// 异步从 PenDocument 导出。
    pub async fn from_pen_document_async(
        doc: fd_canvas_core::PenDocument,
        format: ExportFormat,
        out_dir: PathBuf,
    ) -> Result<Vec<PathBuf>, ExportError> {
        tokio::task::spawn_blocking(move || Self::from_pen_document(&doc, format, &out_dir))
            .await
            .map_err(|e| ExportError::RenderFailed(format!("阻塞任务失败: {e}")))?
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

    // E-21 回归：<title> 中 page.name 含 </title><script> 应被转义，不得执行脚本。
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
            "原始 <script> 不得残留"
        );
        assert!(content.contains("&lt;script&gt;"), "应被转义为实体");
        assert!(content.contains("&lt;/title&gt;"), "</title> 应被转义");
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

    // R-A15 回归：rect/circle 必须真正画入 PDF，不能只输出文字导致白纸。
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
        assert!(with_rect.contains(" re"), "rect 应生成 PDF re 矩形算子");
        assert!(
            with_rect.contains(" rg"),
            "fill #ff0000 应生成 rg 填充色算子"
        );
        assert!(
            !empty_pdf.contains(" re"),
            "空页面不应含 re 矩形算子（对照基线）"
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
            "circle fill 应生成 rg 填充色算子"
        );
        assert!(
            with_circle.contains(" RG"),
            "circle stroke 应生成 RG 描边色算子"
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
            "非法颜色不应 panic，仍输出有效 PDF"
        );
    }

    // R-10：CJK 文本检测。
    #[test]
    fn text_has_cjk_detects_chinese() {
        assert!(pdf::text_has_cjk("登录页面"));
        assert!(pdf::text_has_cjk("hello 世界"));
        assert!(pdf::text_has_cjk("全角　空格"));
        assert!(!pdf::text_has_cjk("plain ascii"));
        assert!(!pdf::text_has_cjk(""));
        assert!(!pdf::text_has_cjk("café résumé"));
    }

    // R-10：含 CJK 文本页面需内嵌字体；纯 Latin 不嵌入（避免无谓开销）。
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

    // R-10：CJK 文本 PDF 仍有效（真机有 PingFang 时嵌入，无时降级但不 panic）。
    // 不强断言字形可见（依赖真机字体），只验证不崩且产出合法 PDF。
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
        assert!(content.starts_with("%PDF"), "CJK 文本应产出合法 PDF");
    }

    // FUNC-9：旋转 shape 导出 PDF 不应丢旋转。当前实现用 printpdf CTM
    // (SetTransformationMatrix/TranslateRotate) 在 SaveGraphicsState 域内旋转。
    // printpdf 0.12 serialize.rs 中 cm 算子未压缩（doc.compress() 被注释），
    // 故可直接在 PDF 字节中检 " cm" 算子验证旋转矩阵已写入。
    // pivot 取 shape 中心（与 SVG 路径 transform="rotate(r cx cy)" 对齐保视觉一致）。
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
        assert!(file.exists(), "PDF 文件应已生成");
        let data = std::fs::read(&file).unwrap();
        assert!(data.starts_with(b"%PDF"), "PDF magic bytes");
        assert!(
            data.len() > 1000,
            "旋转 rect PDF 应非平凡大小（>1000B），实际 {}B",
            data.len()
        );
        let content = String::from_utf8_lossy(&data);
        assert!(
            content.contains(" re"),
            "旋转 rect 仍应生成 re 矩形算子（未被跳过）"
        );
        assert!(
            content.contains(" cm"),
            "旋转 shape 应生成 cm CTM 算子（FUNC-9 旋转矩阵已写入）"
        );
        assert!(content.contains(" rg"), "fill #ff0000 应生成 rg 填充色算子");
    }

    // R-11：跳过的不支持元素类型不再静默——PDF 仍生成但该元素缺失。
    // 验证未知类型不 panic 且产出合法 PDF（跳过清单经 tracing warn 打印）。
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
            "含不支持元素的页面仍应产出合法 PDF"
        );
        assert!(content.contains(" re"), "支持的 rect 仍应正常导出");
    }

    // R-A16 回归：总像素超限（单边不超限）应拒绝渲染，防 1GB OOM。
    // 9000×9000=81M px > 64M，但单边 < 16384，必须由总像素门控拦下。
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
        let err = res.expect_err("9000×9000=81M px > 64M 应被总像素门控拒绝，不分配 1GB");
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("总像素") || msg.contains("超出"),
            "应报总像素超限，实际: {msg}"
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
        // PERF-4：多页逐页流式导出，每页产出独立文件，无全量 collect 峰值。
        // 回归锁定：多页 from_pen_document 须逐页导出全部页（不漏页）。
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
        assert_eq!(files.len(), 5, "5 页须全导出");
        for (i, f) in files.iter().enumerate() {
            let content = std::fs::read_to_string(f).unwrap();
            assert!(
                content.contains(&format!("mark{i}")),
                "第 {i} 页内容须对应: {content}"
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
        // E-20/P3：空名/全非法字符须回退 "page"，不得产出 ".svg" 隐藏文件。
        assert_eq!(util::sanitize_filename(""), "page");
        assert_eq!(util::sanitize_filename("   "), "___"); // 3 空格→3 下划线非空
        assert_eq!(util::sanitize_filename("///"), "___"); // 3 斜杠→3 下划线非空
        assert_eq!(util::sanitize_filename("!!!"), "___"); // 3 感叹号→3 下划线非空
    }

    #[test]
    fn export_empty_name_page_not_hidden_file() {
        // E-20/P3：空名页面导出文件名须为 "page.svg" 非 ".svg"（隐藏文件）。
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
            "空名导出 page.svg 非 .svg: {files:?}"
        );
        assert!(
            !files.iter().any(|f| f == ".svg"),
            "不得产出隐藏 .svg: {files:?}"
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
        // FUNC-9：text arm 原独立分支不消费 attrs，丢旋转。修后须带 transform。
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
            "text 旋转须输出 transform: {svg}"
        );
        assert!(svg.contains("hello"), "text 内容须保留: {svg}");
    }

    #[test]
    fn rotation_zero_svg_text_no_transform() {
        // rotation=0 或 None 不应输出 transform（避免无意义旋转属性）。
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
            "rotation=0 不应输出 transform: {svg}"
        );
        el.rotation = None;
        let svg = svg::render_element_svg(&el);
        assert!(
            !svg.contains("transform"),
            "rotation=None 不应输出 transform: {svg}"
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
        // to_css_value() 产出 dot 形式 var(--color.accent)
        let reg = apple_hig_registry();
        let v = util::resolve_color_var(&Some("var(--color.accent)".into()), &reg);
        assert_eq!(v.as_deref(), Some("#007AFF"));
    }

    #[test]
    fn resolve_color_var_dash_form() {
        // parse-html 产出 dash 形式 var(--color-accent)
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
        // usvg 无法解析 var(--)；导出前必须替换为实际色值（#8）
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
            "SVG 应含解析后的实际色值: {svg}"
        );
        assert!(
            !svg.contains("var(--"),
            "SVG 不应残留未解析 CSS 变量: {svg}"
        );
    }

    #[test]
    fn from_pen_document_with_tokens_png_resolved() {
        // Token 驱动的设计稿导出 PNG：颜色已解析，不再是回退黑色
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
        // R-5：data:image/* 放行，相对路径放行
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
        // R-5：javascript/data:text/html/http(s)/file 一律拒（返空串）
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
        // R-5 端到端：image 元素的恶意 href 经 sanitize_image_url 拦截
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
