//! Fusion-Design CSS 安全工具 — 颜色解析 + 值净化。
//!
//! A-5：从 fd-canvas-core 下沉为独立叶子 crate（无 fd-* 依赖），
//! 供 fd-canvas-core（re-export）/fd-codegen/fd-export/fd-design-lint/
//! fd-host-web/fd-design-system 共享。
//!
//! 离线硬约束：净化器拦截 url()/expression()/@import/javascript: 等危险函数，
//! 剥离可逃逸字符（;{}<>"），防 CSS 注入逃逸。

/// 解析 hex 颜色为 RGB 三元组。支持 #RGB/#RGBA/#RRGGBB/#RRGGBBAA（alpha 忽略），
/// 带/不带 # 前缀，前后空白容忍。CJK/emoji 等非 ASCII 字节 ASCII 门控拒（返 None）。
pub fn parse_hex_color(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    // ASCII hex 门控：非 ASCII 字节（CJK/emoji 多字节）直接拒，避免字节切片越界。
    if !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let b = s.as_bytes();
    let (r, g, bl) = match b.len() {
        3 => (
            // #RGB：每位展开 ×17（如 f→ff=255）
            u8::from_str_radix(&s[0..1], 16).ok()? * 17,
            u8::from_str_radix(&s[1..2], 16).ok()? * 17,
            u8::from_str_radix(&s[2..3], 16).ok()? * 17,
        ),
        4 => (
            // #RGBA：忽略末位 alpha，3 位展开 ×17
            u8::from_str_radix(&s[0..1], 16).ok()? * 17,
            u8::from_str_radix(&s[1..2], 16).ok()? * 17,
            u8::from_str_radix(&s[2..3], 16).ok()? * 17,
        ),
        6 => (
            u8::from_str_radix(&s[0..2], 16).ok()?,
            u8::from_str_radix(&s[2..4], 16).ok()?,
            u8::from_str_radix(&s[4..6], 16).ok()?,
        ),
        8 => (
            // #RRGGBBAA：忽略末 2 位 alpha
            u8::from_str_radix(&s[0..2], 16).ok()?,
            u8::from_str_radix(&s[2..4], 16).ok()?,
            u8::from_str_radix(&s[4..6], 16).ok()?,
        ),
        _ => return None,
    };
    Some([r, g, bl])
}

/// 净化 CSS 值（颜色/字体等）。含危险函数（url()/expression()/@import/javascript:）
/// 返 fallback；否则剥可逃逸字符（;{}<>"）。纯函数，rejection 时 tracing::warn
/// 记录（fail visibly）。颜色传 fallback="transparent"，字体传 ""。
pub fn sanitize_css_value(raw: &str, fallback: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("url(")
        || lower.contains("expression(")
        || lower.contains("@import")
        || lower.contains("javascript:")
    {
        tracing::warn!(raw = raw, "CSS 值含危险函数，降级为 fallback（离线约束）");
        return fallback.to_string();
    }
    raw.chars()
        .filter(|&c| !matches!(c, ';' | '{' | '}' | '<' | '>' | '"'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // A-2：CJK/emoji 输入绝不 panic（旧实现字节切片越界）。
    #[test]
    fn parse_hex_color_ascii_gate_rejects_cjk() {
        assert_eq!(parse_hex_color("蓝"), None);
        assert_eq!(parse_hex_color("#中"), None);
        assert_eq!(parse_hex_color("#中文ab"), None);
        assert_eq!(parse_hex_color("😀"), None);
        // 混合 ASCII + CJK：含非 hex 字节即拒
        assert_eq!(parse_hex_color("#a蓝b"), None);
    }

    #[test]
    fn parse_hex_color_valid_forms() {
        assert_eq!(parse_hex_color("#fff"), Some([255, 255, 255]));
        assert_eq!(parse_hex_color("fff"), Some([255, 255, 255]));
        assert_eq!(parse_hex_color("#abcd"), Some([170, 187, 204])); // #RGBA，alpha 忽略
        assert_eq!(parse_hex_color("#aabbcc"), Some([170, 187, 204]));
        assert_eq!(parse_hex_color("#aabbccff"), Some([170, 187, 204])); // #RRGGBBAA，alpha 忽略
                                                                         // 带空白
        assert_eq!(parse_hex_color("  #FF8800  "), Some([255, 136, 0]));
    }

    #[test]
    fn parse_hex_color_rejects_invalid_length() {
        assert_eq!(parse_hex_color("#12345"), None); // 5 位
        assert_eq!(parse_hex_color("#1234567"), None); // 7 位
        assert_eq!(parse_hex_color(""), None);
    }

    #[test]
    fn parse_hex_color_rejects_non_hex_chars() {
        assert_eq!(parse_hex_color("#gg0000"), None);
        assert_eq!(parse_hex_color("#12gh34"), None);
    }

    // A-3：危险函数返 fallback（离线硬约束）。
    #[test]
    fn sanitize_css_value_rejects_url() {
        assert_eq!(
            sanitize_css_value("red;url(http://evil)", "transparent"),
            "transparent"
        );
        assert_eq!(
            sanitize_css_value("url(http://x)", "transparent"),
            "transparent"
        );
        assert_eq!(
            sanitize_css_value("background:expression(alert(1))", "transparent"),
            "transparent"
        );
        assert_eq!(
            sanitize_css_value("@import 'evil.css'", "transparent"),
            "transparent"
        );
        assert_eq!(
            sanitize_css_value("javascript:alert(1)", "transparent"),
            "transparent"
        );
        // 大小写不敏感
        assert_eq!(
            sanitize_css_value("URL(HTTP://X)", "transparent"),
            "transparent"
        );
    }

    #[test]
    fn sanitize_css_value_strips_quote() {
        // C-6：`"` 被 Tailwind 任意值语法 `bg-[{v}]` 用作属性逃逸点，必须剥。
        assert_eq!(sanitize_css_value(r#"a"b"#, "transparent"), "ab");
        assert_eq!(sanitize_css_value(r#"red"{"#, "transparent"), "red");
    }

    #[test]
    fn sanitize_css_value_preserves_safe() {
        assert_eq!(sanitize_css_value("#FF0000", "transparent"), "#FF0000");
        assert_eq!(
            sanitize_css_value("rgb(255,0,0)", "transparent"),
            "rgb(255,0,0)"
        );
        assert_eq!(
            sanitize_css_value("Helvetica, sans-serif", ""),
            "Helvetica, sans-serif"
        );
        assert_eq!(sanitize_css_value("red", "transparent"), "red");
    }

    #[test]
    fn sanitize_css_value_strips_meta_chars() {
        assert_eq!(sanitize_css_value("red;}", "transparent"), "red");
        assert_eq!(sanitize_css_value("a{b}c", "transparent"), "abc");
        assert_eq!(sanitize_css_value("a<b>c", "transparent"), "abc");
    }

    #[test]
    fn sanitize_css_value_font_fallback_empty() {
        assert_eq!(sanitize_css_value("url(x)", ""), "");
    }
}
