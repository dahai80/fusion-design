// ARCH-10 r4：颜色/对比度纯函数从 lib.rs 拆出。check_contrast 消费，无状态。
// E-14：luminance 旧实现仅 parse_hex_color，遇 rgb()/rgba()/命名色即返 -1.0 跳过，
// check_contrast 静默漏检（假阴性）。扩解析器：hex（含 alpha 忽略，对齐 parse_hex_color）、
// rgb(r,g,b)、rgba(r,g,b,a)、10 个常见命名色。返 (r,g,b) 三元组，alpha 在 check_contrast 单独处理。

use fd_canvas_core::parse_hex_color;

pub(super) fn luminance(color: &str) -> f64 {
    let (r, g, b) = match parse_color_any(color) {
        Some(c) => c,
        None => return -1.0,
    };

    let r = srgb_to_linear(r as f64 / 255.0);
    let g = srgb_to_linear(g as f64 / 255.0);
    let b = srgb_to_linear(b as f64 / 255.0);

    0.2126 * r + 0.7152 * g + 0.0722 * b
}

pub(super) fn parse_color_any(color: &str) -> Option<(u8, u8, u8)> {
    let trimmed = color.trim();
    if let Some(hex) = parse_hex_color(trimmed) {
        return Some((hex[0], hex[1], hex[2]));
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(rest) = lower
        .strip_prefix("rgba(")
        .and_then(|s| s.strip_suffix(')'))
    {
        return parse_rgb_components(rest).map(|(r, g, b, _)| (r, g, b));
    }
    if let Some(rest) = lower.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')) {
        return parse_rgb_components(rest).map(|(r, g, b, _)| (r, g, b));
    }
    named_color(&lower)
}

// rgb/rgba 内部解析：逗号分隔，前 3 通道 0-255 u8，第 4（rgba alpha）0.0-1.0 忽略（对比度按不透明近似）。
fn parse_rgb_components(rest: &str) -> Option<(u8, u8, u8, Option<f32>)> {
    let parts: Vec<&str> = rest.split(',').map(|p| p.trim()).collect();
    if parts.len() < 3 {
        return None;
    }
    let r = parts[0].parse::<u8>().ok()?;
    let g = parts[1].parse::<u8>().ok()?;
    let b = parts[2].parse::<u8>().ok()?;
    let a = if parts.len() >= 4 {
        parts[3].parse::<f32>().ok()
    } else {
        None
    };
    Some((r, g, b, a))
}

// 常见命名色表（WCAG 对比度场景高频）。非完整 CSS 色名，仅覆盖设计稿常用。
fn named_color(name: &str) -> Option<(u8, u8, u8)> {
    let rgb = match name {
        "black" => (0, 0, 0),
        "white" => (255, 255, 255),
        "red" => (255, 0, 0),
        "green" => (0, 128, 0),
        "blue" => (0, 0, 255),
        "yellow" => (255, 255, 0),
        "gray" | "grey" => (128, 128, 128),
        "orange" => (255, 165, 0),
        "purple" => (128, 0, 128),
        "transparent" => (255, 255, 255),
        _ => return None,
    };
    Some(rgb)
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.03928 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub(super) fn contrast_ratio(l1: f64, l2: f64) -> f64 {
    let lighter = l1.max(l2);
    let darker = l1.min(l2);
    (lighter + 0.05) / (darker + 0.05)
}
