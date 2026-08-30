// ARCH-10 r4：PNG 光栅化从 lib.rs 拆出。经 svg::render_svg 产 SVG 串再经
// resvg 光栅化。尺寸/总像素门控防 OOM。零行为变更。

use std::path::Path;

use crate::svg::render_svg;
use crate::{CanvasPage, ExportError, MAX_CANVAS_DIM, MAX_CANVAS_PIXELS};

pub(super) fn render_png(page: &CanvasPage, file: &Path) -> Result<(), ExportError> {
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
