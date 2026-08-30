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

use fd_canvas_core::parse_hex_color;
use fd_design_system::DesignSystemRegistry;
use serde::{Deserialize, Serialize};

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

/// 解析单个颜色值中的 Token 引用为实际色值。
/// 兼容 `var(--color.accent)`（dot）、`var(--color-accent)`（dash）与 `token:color.accent` 三种形式。
/// 未配置设计规范或 token 未定义时保留原值并告警（usvg 会回退黑色，但不影响其他格式导出）。
fn resolve_color_var(value: &Option<String>, reg: &DesignSystemRegistry) -> Option<String> {
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
fn resolve_page_token_vars(page: &mut CanvasPage, reg: &DesignSystemRegistry) {
    for el in &mut page.elements {
        el.fill = resolve_color_var(&el.fill, reg);
        el.stroke = resolve_color_var(&el.stroke, reg);
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
        reg: &DesignSystemRegistry,
    ) -> Result<Vec<PathBuf>, ExportError> {
        // PERF-4：逐页转换+token 解析+导出+释放，避免一次性 collect 全部 CanvasPage 占峰值内存。
        let mut files = Vec::with_capacity(doc.pages.len());
        let mut errors: Vec<String> = Vec::new();
        for page in &doc.pages {
            let mut canvas_page = CanvasPage::from_page(page);
            resolve_page_token_vars(&mut canvas_page, reg);
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

fn sanitize_filename(name: &str) -> String {
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

fn render_html(page: &CanvasPage) -> String {
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

fn xml_escape(s: &str) -> String {
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
fn sanitize_image_url(raw: &str) -> String {
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

fn render_png(page: &CanvasPage, file: &Path) -> Result<(), ExportError> {
    let svg_str = render_svg(page);
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_str(&svg_str, &opt)
        .map_err(|e| ExportError::RenderFailed(format!("SVG 解析失败: {e}")))?;
    let pixmap_size = tree.size();
    let width = pixmap_size.width() as u32;
    let height = pixmap_size.height() as u32;
    if width == 0 || height == 0 {
        return Err(ExportError::RenderFailed(
            "页面尺寸为零，无法渲染 PNG".into(),
        ));
    }
    if width > MAX_CANVAS_DIM || height > MAX_CANVAS_DIM {
        tracing::warn!(
            width,
            height,
            limit = MAX_CANVAS_DIM,
            "画布尺寸超出光栅化上限，拒绝渲染 PNG"
        );
        return Err(ExportError::RenderFailed(format!(
            "画布尺寸 {width}x{height} 超出光栅化上限 {MAX_CANVAS_DIM}x{MAX_CANVAS_DIM}，拒绝渲染 PNG 防止 OOM"
        )));
    }
    // R-A16：单边限制不够，16384²×4=1GB 仍 OOM。总像素门控。
    let total_pixels = width as u64 * height as u64;
    if total_pixels > MAX_CANVAS_PIXELS {
        tracing::warn!(
            width,
            height,
            total_pixels,
            limit = MAX_CANVAS_PIXELS,
            "画布总像素超出上限，拒绝渲染 PNG"
        );
        return Err(ExportError::RenderFailed(format!(
            "画布总像素 {total_pixels} ({width}x{height}) 超出上限 {MAX_CANVAS_PIXELS}，拒绝渲染 PNG 防止 OOM"
        )));
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| ExportError::RenderFailed(format!("无法创建 pixmap ({width}x{height})")))?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    let png_data = pixmap
        .encode_png()
        .map_err(|e| ExportError::RenderFailed(format!("PNG 编码失败: {e}")))?;
    std::fs::write(file, &png_data)?;
    tracing::info!(?file, "PNG 已导出");
    Ok(())
}

/// 把 #rrggbb / #rgb 十六进制颜色字符串解析为 printpdf Color::Rgb（0..1 浮点）。
/// 无法解析返回 None（调用方按默认色处理）。支持大写/小写/3 位/6 位。
/// C-3：经 fd_canvas_core::parse_hex_color ASCII 门控，CJK 字节切片 panic 根治。
fn hex_to_pdf_color(hex: &str) -> Option<printpdf::Color> {
    let [r, g, b] = parse_hex_color(hex)?;
    Some(printpdf::Color::Rgb(printpdf::Rgb::new(
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        None,
    )))
}

/// 判定字符串是否含 CJK 字符（中日韩统一表意文字 + 兼容表意文字）。
/// BuiltinFont::Helvetica 是 WinAnsi 编码，CJK 字符会丢成 .notdef（R-10）。
/// 含 CJK 时需切换到内嵌 TTF 字体（PingFang）才能正确出字。
fn text_has_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        let cp = c as u32;
        (0x4E00..=0x9FFF).contains(&cp)      // CJK 统一表意文字
            || (0x3400..=0x4DBF).contains(&cp) // CJK 扩展 A
            || (0xF900..=0xFAFF).contains(&cp) // CJK 兼容表意文字
            || (0x3000..=0x303F).contains(&cp) // CJK 标点
            || (0xFF00..=0xFFEF).contains(&cp) // 全角字符
    })
}

/// 页面是否含 CJK 文本元素（决定是否需要内嵌中文字体）。
fn page_has_cjk_text(page: &CanvasPage) -> bool {
    page.elements
        .iter()
        .any(|el| el.kind == "text" && el.text.as_deref().is_some_and(text_has_cjk))
}

/// macOS 系统中文字体候选路径。PingFang.ttc 是 macOS 14+ 默认中文字体。
/// 离线约束下不打包字体（版权 + 体积），运行时检测系统字体。
/// R-10：PDF 导出 CJK 文本需内嵌 TTF/OTF/TTC，Helvetica(WinAnsi) 不支持中文。
fn cjk_font_paths() -> &'static [&'static str] {
    &[
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
    ]
}

/// 加载系统中文字体字节。返回 (bytes, font_index)——TTC 需指定 face index。
/// 找不到系统字体返回 None（调用方降级 Helvetica + loud warn）。
fn load_cjk_font() -> Option<(Vec<u8>, usize)> {
    for path in cjk_font_paths() {
        match std::fs::read(path) {
            Ok(bytes) => {
                tracing::info!(font = %path, bytes = bytes.len(), "PDF CJK 字体已加载");
                return Some((bytes, 0));
            }
            Err(e) => {
                tracing::debug!(font = %path, err = %e, "CJK 字体候选不存在，尝试下一个");
            }
        }
    }
    None
}

fn render_pdf(page: &CanvasPage, file: &Path) -> Result<(), ExportError> {
    let width_mm = page.width * 0.264583;
    let height_mm = page.height * 0.264583;
    // 像素→mm 转换因子（1 px = 0.264583 mm @ 96 DPI）。
    let px_to_mm = 0.264583_f64;

    let mut doc = printpdf::PdfDocument::new(&page.name);

    // R-10：含 CJK 文本时内嵌系统中文字体（PingFang），否则 CJK 字符在
    // Helvetica(WinAnsi) 下丢成 .notdef 不可见。无系统字体则 loud warn 降级。
    let cjk_font_id: Option<printpdf::FontId> = if page_has_cjk_text(page) {
        match load_cjk_font() {
            Some((bytes, idx)) => {
                let mut warns = Vec::new();
                match printpdf::font::ParsedFont::from_bytes(&bytes, idx, &mut warns) {
                    Some(parsed) => Some(doc.add_font(&parsed)),
                    None => {
                        tracing::warn!(
                            "PDF CJK 字体解析失败，CJK 文本将无法显示（降级 Helvetica）"
                        );
                        None
                    }
                }
            }
            None => {
                tracing::warn!(
                    "未找到系统中文字体（PingFang.ttc 等），PDF CJK 文本将无法显示。\
                     请在 macOS 安装中文字体后重试。"
                );
                None
            }
        }
    } else {
        None
    };

    let mut ops = Vec::new();
    // R-11：聚合跳过的元素类型，供调用方 fail visibly 打印跳过清单。
    let mut skipped_kinds: Vec<String> = Vec::new();
    // 先画形状（rect/circle），再画文字，保证文字在形状之上不被遮挡。
    for el in &page.elements {
        match el.kind.as_str() {
            "rect" => {
                let fill = el.fill.as_deref().and_then(hex_to_pdf_color);
                let stroke = el.stroke.as_deref().and_then(hex_to_pdf_color);
                if fill.is_none() && stroke.is_none() {
                    continue;
                }
                ops.push(printpdf::ops::Op::SaveGraphicsState);
                // FUNC-9：非 0 旋转 shape 在 PDF 用 CTM 旋转，否则丢旋转。
                // CTM 累加，须在 SaveGraphicsState 域内（本 arm 末 RestoreGraphicsState 已复位）。
                // pivot 取 shape 中心（与 SVG transform="rotate(r cx cy)" 对齐保视觉一致）。
                if let Some(angle) = el.rotation {
                    if angle != 0.0 {
                        let cx = (el.x + el.w / 2.0) * px_to_mm;
                        let cy = height_mm - (el.y + el.h / 2.0) * px_to_mm;
                        ops.push(printpdf::ops::Op::SetTransformationMatrix {
                            matrix: printpdf::CurTransMat::TranslateRotate(
                                printpdf::Pt(cx as f32),
                                printpdf::Pt(cy as f32),
                                angle as f32,
                            ),
                        });
                    }
                }
                let mode = match (&fill, &stroke) {
                    (Some(_), Some(_)) => printpdf::graphics::PaintMode::FillStroke,
                    (Some(_), None) => printpdf::graphics::PaintMode::Fill,
                    (None, Some(_)) => printpdf::graphics::PaintMode::Stroke,
                    (None, None) => printpdf::graphics::PaintMode::Fill,
                };
                if let Some(c) = &fill {
                    ops.push(printpdf::ops::Op::SetFillColor { col: c.clone() });
                }
                if let Some(c) = &stroke {
                    ops.push(printpdf::ops::Op::SetOutlineColor { col: c.clone() });
                    if let Some(sw) = el.stroke_width {
                        ops.push(printpdf::ops::Op::SetOutlineThickness {
                            pt: printpdf::Pt(sw as f32),
                        });
                    }
                }
                // PDF 原点左下、y 向上；画布左上、y 向下。
                let pdf_y = height_mm - el.y * px_to_mm - el.h * px_to_mm;
                let mut rect = printpdf::graphics::Rect::from_xywh(
                    printpdf::Pt((el.x * px_to_mm) as f32),
                    printpdf::Pt(pdf_y as f32),
                    printpdf::Pt((el.w * px_to_mm) as f32),
                    printpdf::Pt((el.h * px_to_mm) as f32),
                );
                rect.mode = Some(mode);
                ops.push(printpdf::ops::Op::DrawRectangle { rectangle: rect });
                ops.push(printpdf::ops::Op::RestoreGraphicsState);
            }
            "circle" => {
                let fill = el.fill.as_deref().and_then(hex_to_pdf_color);
                let stroke = el.stroke.as_deref().and_then(hex_to_pdf_color);
                if fill.is_none() && stroke.is_none() {
                    continue;
                }
                let cx = el.x + el.w / 2.0;
                let cy = el.y + el.h / 2.0;
                let r = el.w.min(el.h) / 2.0;
                ops.push(printpdf::ops::Op::SaveGraphicsState);
                // FUNC-9：非 0 旋转 shape 在 PDF 用 CTM 旋转（circle 旋转视觉无变化但保一致）。
                if let Some(angle) = el.rotation {
                    if angle != 0.0 {
                        let cx = (el.x + el.w / 2.0) * px_to_mm;
                        let cy = height_mm - (el.y + el.h / 2.0) * px_to_mm;
                        ops.push(printpdf::ops::Op::SetTransformationMatrix {
                            matrix: printpdf::CurTransMat::TranslateRotate(
                                printpdf::Pt(cx as f32),
                                printpdf::Pt(cy as f32),
                                angle as f32,
                            ),
                        });
                    }
                }
                let mode = match (&fill, &stroke) {
                    (Some(_), Some(_)) => printpdf::graphics::PaintMode::FillStroke,
                    (Some(_), None) => printpdf::graphics::PaintMode::Fill,
                    (None, Some(_)) => printpdf::graphics::PaintMode::Stroke,
                    (None, None) => printpdf::graphics::PaintMode::Fill,
                };
                if let Some(c) = &fill {
                    ops.push(printpdf::ops::Op::SetFillColor { col: c.clone() });
                }
                if let Some(c) = &stroke {
                    ops.push(printpdf::ops::Op::SetOutlineColor { col: c.clone() });
                    if let Some(sw) = el.stroke_width {
                        ops.push(printpdf::ops::Op::SetOutlineThickness {
                            pt: printpdf::Pt(sw as f32),
                        });
                    }
                }
                // 圆用 4 段三次贝塞尔近似（magic number 0.5523）。PDF 坐标系 y 翻转。
                let pdf_cy = height_mm - cy * px_to_mm;
                let cx_pt = printpdf::Pt((cx * px_to_mm) as f32);
                let r_pt = (r * px_to_mm) as f32;
                let k = 0.5523_f32 * r_pt;
                let poly = circle_polygon(
                    printpdf::Pt(pdf_cy as f32),
                    cx_pt,
                    printpdf::Pt(r_pt),
                    k,
                    mode,
                );
                ops.push(printpdf::ops::Op::DrawPolygon { polygon: poly });
                ops.push(printpdf::ops::Op::RestoreGraphicsState);
            }
            "text" => {
                let text = el.text.as_deref().unwrap_or("");
                let fs = el.font_size.unwrap_or(12.0);
                ops.push(printpdf::ops::Op::StartTextSection);
                // R-10：CJK 文本用内嵌中文字体；纯 ASCII/Latin 仍用 Helvetica（无嵌入开销）。
                let font_handle = match (&cjk_font_id, text_has_cjk(text)) {
                    (Some(fid), true) => printpdf::ops::PdfFontHandle::External(fid.clone()),
                    _ => printpdf::ops::PdfFontHandle::Builtin(printpdf::BuiltinFont::Helvetica),
                };
                ops.push(printpdf::ops::Op::SetFont {
                    font: font_handle,
                    size: printpdf::Pt(fs as f32),
                });
                // FUNC-9：text 非 0 旋转——SetTextMatrix(Tm) 绕文本基点旋转。
                // Tm 重置文本矩阵（含定位），故旋转路径不再用 SetTextCursor（会被 Tm 覆盖）。
                // 非旋转路径沿用 SetTextCursor 定位（保持原行为）。基点对齐 rect/circle 用 shape 中心。
                let rotate_angle = el.rotation.filter(|r| *r != 0.0);
                if let Some(angle) = rotate_angle {
                    let tx = (el.x + el.w / 2.0) * px_to_mm;
                    let ty = height_mm - (el.y + el.h / 2.0) * px_to_mm;
                    ops.push(printpdf::ops::Op::SetTextMatrix {
                        matrix: printpdf::TextMatrix::TranslateRotate(
                            printpdf::Pt(tx as f32),
                            printpdf::Pt(ty as f32),
                            angle as f32,
                        ),
                    });
                } else {
                    ops.push(printpdf::ops::Op::SetTextCursor {
                        pos: printpdf::graphics::Point::new(
                            printpdf::Mm((el.x * px_to_mm) as f32),
                            printpdf::Mm((height_mm - el.y * px_to_mm - fs * px_to_mm) as f32),
                        ),
                    });
                }
                ops.push(printpdf::ops::Op::ShowText {
                    items: vec![printpdf::text::TextItem::Text(text.to_string())],
                });
                ops.push(printpdf::ops::Op::EndTextSection);
            }
            // L-12/R-11：不支持此元素类型不再静默丢弃——聚合到 skipped_kinds 供调用方打印。
            // FUNC-9：rect/circle 的非 0 旋转已在上文各 arm 用 SetTransformationMatrix
            // (CurTransMat::TranslateRotate) 处理，绕 shape 中心旋转，RestoreGraphicsState 自动复位 CTM。
            other => {
                if !skipped_kinds.iter().any(|k| k == other) {
                    skipped_kinds.push(other.to_string());
                }
                tracing::warn!(kind = %other, "PDF 导出不支持此元素类型，已跳过");
            }
        }
    }

    let pdf_page = printpdf::ops::PdfPage::new(
        printpdf::Mm(width_mm as f32),
        printpdf::Mm(height_mm as f32),
        ops,
    );
    doc.with_pages(vec![pdf_page]);

    let mut warnings = Vec::new();
    let opts = printpdf::serialize::PdfSaveOptions::default();
    let pdf_data = doc.save(&opts, &mut warnings);
    std::fs::write(file, &pdf_data)?;
    // R-11：跳过的元素类型 fail visibly——已导出但不完整，聚合打印供调用方/用户感知。
    if !skipped_kinds.is_empty() {
        tracing::warn!(
            skipped = ?skipped_kinds,
            "PDF 导出跳过部分不支持元素类型（输出可能不完整）"
        );
    }
    tracing::info!(?file, warnings = warnings.len(), "PDF 已导出");
    Ok(())
}

/// 用 4 段三次贝塞尔近似圆，构造 printpdf Polygon（含控制点）。
/// 中心 (cx, cy_pt)，半径 r_pt，k 为控制点偏移（0.5523*r）。
fn circle_polygon(
    cy_pt: printpdf::Pt,
    cx: printpdf::Pt,
    r_pt: printpdf::Pt,
    k: f32,
    mode: printpdf::graphics::PaintMode,
) -> printpdf::graphics::Polygon {
    use printpdf::graphics::{LinePoint, Point};
    // 4 个锚点：右、上、左、下（PDF y 向上）。
    let p_right = Point {
        x: printpdf::Pt(cx.0 + r_pt.0),
        y: cy_pt,
    };
    let p_top = Point {
        x: cx,
        y: printpdf::Pt(cy_pt.0 + r_pt.0),
    };
    let p_left = Point {
        x: printpdf::Pt(cx.0 - r_pt.0),
        y: cy_pt,
    };
    let p_bottom = Point {
        x: cx,
        y: printpdf::Pt(cy_pt.0 - r_pt.0),
    };
    // 每段两个控制点（bezier=true）。
    let c1a = Point {
        x: printpdf::Pt(cx.0 + r_pt.0),
        y: printpdf::Pt(cy_pt.0 + k),
    };
    let c1b = Point {
        x: printpdf::Pt(cx.0 + k),
        y: printpdf::Pt(cy_pt.0 + r_pt.0),
    };
    let c2a = Point {
        x: printpdf::Pt(cx.0 - k),
        y: printpdf::Pt(cy_pt.0 + r_pt.0),
    };
    let c2b = Point {
        x: printpdf::Pt(cx.0 - r_pt.0),
        y: printpdf::Pt(cy_pt.0 + k),
    };
    let c3a = Point {
        x: printpdf::Pt(cx.0 - r_pt.0),
        y: printpdf::Pt(cy_pt.0 - k),
    };
    let c3b = Point {
        x: printpdf::Pt(cx.0 - k),
        y: printpdf::Pt(cy_pt.0 - r_pt.0),
    };
    let c4a = Point {
        x: printpdf::Pt(cx.0 + k),
        y: printpdf::Pt(cy_pt.0 - r_pt.0),
    };
    let c4b = Point {
        x: printpdf::Pt(cx.0 + r_pt.0),
        y: printpdf::Pt(cy_pt.0 - k),
    };
    let points = vec![
        LinePoint {
            p: p_right,
            bezier: false,
        },
        LinePoint {
            p: c1a,
            bezier: true,
        },
        LinePoint {
            p: c1b,
            bezier: true,
        },
        LinePoint {
            p: p_top,
            bezier: false,
        },
        LinePoint {
            p: c2a,
            bezier: true,
        },
        LinePoint {
            p: c2b,
            bezier: true,
        },
        LinePoint {
            p: p_left,
            bezier: false,
        },
        LinePoint {
            p: c3a,
            bezier: true,
        },
        LinePoint {
            p: c3b,
            bezier: true,
        },
        LinePoint {
            p: p_bottom,
            bezier: false,
        },
        LinePoint {
            p: c4a,
            bezier: true,
        },
        LinePoint {
            p: c4b,
            bezier: true,
        },
        // 闭合回起点。
        LinePoint {
            p: p_right,
            bezier: false,
        },
    ];
    printpdf::graphics::Polygon {
        rings: vec![printpdf::graphics::PolygonRing { points }],
        mode,
        winding_order: printpdf::graphics::WindingOrder::default(),
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
        assert!(text_has_cjk("登录页面"));
        assert!(text_has_cjk("hello 世界"));
        assert!(text_has_cjk("全角　空格"));
        assert!(!text_has_cjk("plain ascii"));
        assert!(!text_has_cjk(""));
        assert!(!text_has_cjk("café résumé"));
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
        assert!(page_has_cjk_text(&cjk_page));

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
        assert!(!page_has_cjk_text(&latin_only));
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
    fn from_pen_document_multi_page_streams() {
        // PERF-4：多页逐页流式导出，每页产出独立文件，无全量 collect 峰值。
        // 回归锁定：多页 from_pen_document 须逐页导出全部页（不漏页）。
        let tmp = tempdir().unwrap();
        let mut doc = fd_canvas_core::PenDocument::new();
        for i in 0..5 {
            let mut page =
                fd_canvas_core::Page::new(&format!("p{i}"), &format!("Page{i}"), 100.0, 100.0);
            page.add(fd_canvas_core::PenNode::text(
                "n",
                10.0,
                10.0,
                &format!("mark{i}"),
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
        assert_eq!(sanitize_filename("Hello/World"), "Hello_World");
        assert_eq!(sanitize_filename("Page 1"), "Page_1");
        assert_eq!(sanitize_filename("ok-name_123"), "ok-name_123");
    }

    #[test]
    fn sanitize_filename_empty_defaults_to_page() {
        // E-20/P3：空名/全非法字符须回退 "page"，不得产出 ".svg" 隐藏文件。
        assert_eq!(sanitize_filename(""), "page");
        assert_eq!(sanitize_filename("   "), "___"); // 3 空格→3 下划线非空
        assert_eq!(sanitize_filename("///"), "___"); // 3 斜杠→3 下划线非空
        assert_eq!(sanitize_filename("!!!"), "___"); // 3 感叹号→3 下划线非空
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
        assert_eq!(xml_escape("<b>bold</b>"), "&lt;b&gt;bold&lt;/b&gt;");
        assert_eq!(xml_escape("a&b"), "a&amp;b");
        assert_eq!(xml_escape("he said \"hi\""), "he said &quot;hi&quot;");
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
        let svg = render_element_svg(&el);
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
        let svg = render_element_svg(&el);
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
        let svg = render_element_svg(&el);
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
        let svg = render_element_svg(&el);
        assert!(
            !svg.contains("transform"),
            "rotation=0 不应输出 transform: {svg}"
        );
        el.rotation = None;
        let svg = render_element_svg(&el);
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

    fn apple_hig_registry() -> DesignSystemRegistry {
        let mut reg = DesignSystemRegistry::new();
        reg.register_builtin();
        reg.activate("apple-hig").unwrap();
        reg
    }

    #[test]
    fn resolve_color_var_dot_form() {
        // to_css_value() 产出 dot 形式 var(--color.accent)
        let reg = apple_hig_registry();
        let v = resolve_color_var(&Some("var(--color.accent)".into()), &reg);
        assert_eq!(v.as_deref(), Some("#007AFF"));
    }

    #[test]
    fn resolve_color_var_dash_form() {
        // parse-html 产出 dash 形式 var(--color-accent)
        let reg = apple_hig_registry();
        let v = resolve_color_var(&Some("var(--color-accent)".into()), &reg);
        assert_eq!(v.as_deref(), Some("#007AFF"));
    }

    #[test]
    fn resolve_color_var_token_prefix() {
        let reg = apple_hig_registry();
        let v = resolve_color_var(&Some("token:color.accent".into()), &reg);
        assert_eq!(v.as_deref(), Some("#007AFF"));
    }

    #[test]
    fn resolve_color_var_passthrough_plain() {
        let reg = apple_hig_registry();
        assert_eq!(
            resolve_color_var(&Some("#FF8800".into()), &reg).as_deref(),
            Some("#FF8800")
        );
        assert_eq!(resolve_color_var(&None, &reg), None);
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
            sanitize_image_url("data:image/png;base64,xxx"),
            "data:image/png;base64,xxx"
        );
        assert_eq!(sanitize_image_url("assets/logo.svg"), "assets/logo.svg");
        assert_eq!(sanitize_image_url("photo.jpg"), "photo.jpg");
        assert_eq!(sanitize_image_url(""), "");
    }

    #[test]
    fn sanitize_image_url_rejects_executable_and_remote() {
        // R-5：javascript/data:text/html/http(s)/file 一律拒（返空串）
        assert_eq!(sanitize_image_url("javascript:alert(1)"), "");
        assert_eq!(
            sanitize_image_url("data:text/html,<script>alert(1)</script>"),
            ""
        );
        assert_eq!(sanitize_image_url("https://evil.com/x.png"), "");
        assert_eq!(sanitize_image_url("http://10.0.0.1/exfil.png"), "");
        assert_eq!(sanitize_image_url("file:///etc/passwd"), "");
    }

    #[test]
    fn sanitize_image_url_rejects_dotdot_traversal() {
        assert_eq!(sanitize_image_url("../secret"), "");
        assert_eq!(sanitize_image_url("a/../b"), "");
        assert_eq!(sanitize_image_url("../../etc/passwd"), "");
    }
    #[test]
    fn sanitize_image_url_allows_normal_relative() {
        assert_eq!(sanitize_image_url("logo.png"), "logo.png");
        assert_eq!(sanitize_image_url("assets/icon.svg"), "assets/icon.svg");
        assert_eq!(
            sanitize_image_url("data:image/png;base64,xxx"),
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
        let svg = render_element_svg(&el);
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
