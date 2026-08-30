// ARCH-10 r4：PDF 渲染从 lib.rs 拆出。含颜色解析、CJK 字体检测/加载、
// circle_polygon（PDF 独占近似圆）。零行为变更。

use std::path::Path;

use fd_canvas_core::parse_hex_color;

use crate::{CanvasPage, ExportError};

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
pub(super) fn text_has_cjk(s: &str) -> bool {
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
pub(super) fn page_has_cjk_text(page: &CanvasPage) -> bool {
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

pub(super) fn render_pdf(page: &CanvasPage, file: &Path) -> Result<(), ExportError> {
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
