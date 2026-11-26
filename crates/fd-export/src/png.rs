// ARCH-10 r4: PNG rasterization split out of lib.rs. Produces an SVG string via svg::render_svg,
// then rasterizes with resvg. Size/total-pixel gates prevent OOM. Zero behavior change.

use std::path::Path;

use crate::svg::render_svg;
use crate::{CanvasPage, ExportError, MAX_CANVAS_DIM, MAX_CANVAS_PIXELS};

pub(super) fn render_png(page: &CanvasPage, file: &Path) -> Result<(), ExportError> {
    let svg_str = render_svg(page);
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_str(&svg_str, &opt)
        .map_err(|e| ExportError::RenderFailed(format!("SVG parse failed: {e}")))?;
    let pixmap_size = tree.size();
    let width = pixmap_size.width() as u32;
    let height = pixmap_size.height() as u32;
    if width == 0 || height == 0 {
        return Err(ExportError::RenderFailed(
            "page size is zero, cannot render PNG".into(),
        ));
    }
    if width > MAX_CANVAS_DIM || height > MAX_CANVAS_DIM {
        tracing::warn!(
            width,
            height,
            limit = MAX_CANVAS_DIM,
            "canvas size exceeds rasterization limit, refusing to render PNG"
        );
        return Err(ExportError::RenderFailed(format!(
            "canvas size {width}x{height} exceeds rasterization limit {MAX_CANVAS_DIM}x{MAX_CANVAS_DIM}, refusing to render PNG to prevent OOM"
        )));
    }
    // R-A16: an edge-only limit is not enough, 16384^2 x 4 = 1GB would still OOM. Total-pixel gate.
    let total_pixels = width as u64 * height as u64;
    if total_pixels > MAX_CANVAS_PIXELS {
        tracing::warn!(
            width,
            height,
            total_pixels,
            limit = MAX_CANVAS_PIXELS,
            "canvas total pixels exceed the limit, refusing to render PNG"
        );
        return Err(ExportError::RenderFailed(format!(
            "canvas total pixels {total_pixels} ({width}x{height}) exceed limit {MAX_CANVAS_PIXELS}, refusing to render PNG to prevent OOM"
        )));
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).ok_or_else(|| {
        ExportError::RenderFailed(format!("failed to create pixmap ({width}x{height})"))
    })?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    let png_data = pixmap
        .encode_png()
        .map_err(|e| ExportError::RenderFailed(format!("PNG encoding failed: {e}")))?;
    std::fs::write(file, &png_data)?;
    tracing::info!(?file, "PNG exported");
    Ok(())
}
