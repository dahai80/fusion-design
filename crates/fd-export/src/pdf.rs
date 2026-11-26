// ARCH-10 r4: PDF rendering split out of lib.rs. Contains color parsing, CJK font detection/loading,
// and circle_polygon (PDF-only circle approximation). Zero behavior change.

use std::path::Path;

use fd_canvas_core::parse_hex_color;

use crate::{CanvasPage, ExportError};

/// Parse a #rrggbb / #rgb hex color string into printpdf Color::Rgb (0..1 floats).
/// Returns None when unparseable (caller falls back to the default color). Supports upper/lower case, 3-digit and 6-digit forms.
/// C-3: goes through fd_canvas_core::parse_hex_color with ASCII gating, eliminating the CJK byte-slice panic at the root.
fn hex_to_pdf_color(hex: &str) -> Option<printpdf::Color> {
    let [r, g, b] = parse_hex_color(hex)?;
    Some(printpdf::Color::Rgb(printpdf::Rgb::new(
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        None,
    )))
}

/// Detect whether a string contains CJK characters (CJK unified ideographs + compatibility ideographs).
/// BuiltinFont::Helvetica uses WinAnsi encoding, where CJK characters degrade to .notdef (R-10).
/// When CJK is present, an embedded TTF font (PingFang) is required to render the text correctly.
pub(super) fn text_has_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        let cp = c as u32;
        (0x4E00..=0x9FFF).contains(&cp)      // CJK unified ideographs
            || (0x3400..=0x4DBF).contains(&cp) // CJK extension A
            || (0xF900..=0xFAFF).contains(&cp) // CJK compatibility ideographs
            || (0x3000..=0x303F).contains(&cp) // CJK punctuation
            || (0xFF00..=0xFFEF).contains(&cp) // fullwidth characters
    })
}

/// Whether the page contains CJK text elements (decides whether a Chinese font must be embedded).
pub(super) fn page_has_cjk_text(page: &CanvasPage) -> bool {
    page.elements
        .iter()
        .any(|el| el.kind == "text" && el.text.as_deref().is_some_and(text_has_cjk))
}

/// Candidate paths for macOS system Chinese fonts. PingFang.ttc is the default Chinese font on macOS 14+.
/// Under the offline constraint no font is bundled (licensing + size); system fonts are detected at runtime.
/// R-10: PDF export of CJK text requires embedding TTF/OTF/TTC; Helvetica (WinAnsi) does not support Chinese.
fn cjk_font_paths() -> &'static [&'static str] {
    &[
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
    ]
}

/// Load system Chinese font bytes. Returns (bytes, font_index) — a TTC requires a face index.
/// Returns None when no system font is found (caller degrades to Helvetica + loud warn).
fn load_cjk_font() -> Option<(Vec<u8>, usize)> {
    for path in cjk_font_paths() {
        match std::fs::read(path) {
            Ok(bytes) => {
                tracing::info!(font = %path, bytes = bytes.len(), "PDF CJK font loaded");
                return Some((bytes, 0));
            }
            Err(e) => {
                tracing::debug!(font = %path, err = %e, "CJK font candidate not present, trying next");
            }
        }
    }
    None
}

pub(super) fn render_pdf(page: &CanvasPage, file: &Path) -> Result<(), ExportError> {
    let width_mm = page.width * 0.264583;
    let height_mm = page.height * 0.264583;
    // Pixel->mm conversion factor (1 px = 0.264583 mm @ 96 DPI).
    let px_to_mm = 0.264583_f64;

    let mut doc = printpdf::PdfDocument::new(&page.name);

    // R-10: when the page contains CJK text, embed a system Chinese font (PingFang); otherwise
    // CJK characters degrade to invisible .notdef under Helvetica (WinAnsi). Without a system
    // font, degrade with a loud warn.
    let cjk_font_id: Option<printpdf::FontId> = if page_has_cjk_text(page) {
        match load_cjk_font() {
            Some((bytes, idx)) => {
                let mut warns = Vec::new();
                match printpdf::font::ParsedFont::from_bytes(&bytes, idx, &mut warns) {
                    Some(parsed) => Some(doc.add_font(&parsed)),
                    None => {
                        tracing::warn!(
                            "PDF CJK font parsing failed, CJK text will not display (degrading to Helvetica)"
                        );
                        None
                    }
                }
            }
            None => {
                tracing::warn!(
                    "No system Chinese font found (PingFang.ttc etc.), PDF CJK text will not display. \
                     Install a Chinese font on macOS and retry."
                );
                None
            }
        }
    } else {
        None
    };

    let mut ops = Vec::new();
    // R-11: aggregate skipped element kinds so the caller can fail visibly with the skip list.
    let mut skipped_kinds: Vec<String> = Vec::new();
    // Draw shapes (rect/circle) first, then text, so text sits above shapes and is not occluded.
    for el in &page.elements {
        match el.kind.as_str() {
            "rect" => {
                let fill = el.fill.as_deref().and_then(hex_to_pdf_color);
                let stroke = el.stroke.as_deref().and_then(hex_to_pdf_color);
                if fill.is_none() && stroke.is_none() {
                    continue;
                }
                ops.push(printpdf::ops::Op::SaveGraphicsState);
                // FUNC-9: non-zero rotated shapes use a CTM rotation in PDF, otherwise the rotation is lost.
                // CTM accumulates, so it must be inside the SaveGraphicsState scope (RestoreGraphicsState at the end of this arm resets it).
                // The pivot is the shape center (aligned with the SVG transform="rotate(r cx cy)" for visual consistency).
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
                // PDF origin is bottom-left with y up; canvas is top-left with y down.
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
                // FUNC-9: non-zero rotated shapes use a CTM rotation in PDF (circle rotation is visually a no-op but kept for consistency).
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
                // Circle approximated with 4 cubic bezier segments (magic number 0.5523). PDF coordinate system flips y.
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
                // R-10: CJK text uses the embedded Chinese font; pure ASCII/Latin keeps Helvetica (no embedding overhead).
                let font_handle = match (&cjk_font_id, text_has_cjk(text)) {
                    (Some(fid), true) => printpdf::ops::PdfFontHandle::External(fid.clone()),
                    _ => printpdf::ops::PdfFontHandle::Builtin(printpdf::BuiltinFont::Helvetica),
                };
                ops.push(printpdf::ops::Op::SetFont {
                    font: font_handle,
                    size: printpdf::Pt(fs as f32),
                });
                // FUNC-9: text non-zero rotation — SetTextMatrix (Tm) rotates around the text baseline origin.
                // Tm resets the text matrix (including positioning), so the rotated path no longer uses SetTextCursor (it would be overridden by Tm).
                // The non-rotated path keeps SetTextCursor positioning (preserving original behavior). Pivot aligned with rect/circle using the shape center.
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
            // L-12/R-11: unsupported element kinds are no longer silently dropped — aggregated into skipped_kinds for the caller to print.
            // FUNC-9: non-zero rotation of rect/circle is already handled above in each arm via SetTransformationMatrix
            // (CurTransMat::TranslateRotate), rotating around the shape center; RestoreGraphicsState resets the CTM automatically.
            other => {
                if !skipped_kinds.iter().any(|k| k == other) {
                    skipped_kinds.push(other.to_string());
                }
                tracing::warn!(kind = %other, "PDF export: unsupported element kind, skipped");
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
    // R-11: skipped element kinds fail visibly — the export completed but is incomplete; print the aggregated list for caller/user awareness.
    if !skipped_kinds.is_empty() {
        tracing::warn!(
            skipped = ?skipped_kinds,
            "PDF export skipped some unsupported element kinds (output may be incomplete)"
        );
    }
    tracing::info!(?file, warnings = warnings.len(), "PDF exported");
    Ok(())
}

/// Approximate a circle with 4 cubic bezier segments, constructing a printpdf Polygon (with control points).
/// Center (cx, cy_pt), radius r_pt, k is the control point offset (0.5523*r).
fn circle_polygon(
    cy_pt: printpdf::Pt,
    cx: printpdf::Pt,
    r_pt: printpdf::Pt,
    k: f32,
    mode: printpdf::graphics::PaintMode,
) -> printpdf::graphics::Polygon {
    use printpdf::graphics::{LinePoint, Point};
    // 4 anchor points: right, top, left, bottom (PDF y is up).
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
    // Two control points per segment (bezier=true).
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
        // Close back to the start point.
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
