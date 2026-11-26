//! Fusion-Design CSS safety utilities — color parsing + value sanitization.
//!
//! A-5: sunk from fd-canvas-core into an independent leaf crate (no fd-* deps),
//! shared by fd-canvas-core (re-export)/fd-codegen/fd-export/fd-design-lint/
//! fd-host-web/fd-design-system.
//!
//! Hard offline constraint: the sanitizer intercepts dangerous functions such as url()/expression()/@import/javascript:,
//! and strips escape-capable characters (;{}<>"), preventing CSS injection escape.

/// Parse a hex color into an RGB triple. Supports #RGB/#RGBA/#RRGGBB/#RRGGBBAA (alpha ignored),
/// with or without a # prefix, tolerant of surrounding whitespace. Non-ASCII bytes such as CJK/emoji are rejected by the ASCII gate (returns None).
pub fn parse_hex_color(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    // ASCII hex gate: non-ASCII bytes (CJK/emoji multi-byte) are rejected outright, avoiding byte-slice out-of-bounds.
    if !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let b = s.as_bytes();
    let (r, g, bl) = match b.len() {
        3 => (
            // #RGB: each digit expanded x17 (e.g. f -> ff = 255)
            u8::from_str_radix(&s[0..1], 16).ok()? * 17,
            u8::from_str_radix(&s[1..2], 16).ok()? * 17,
            u8::from_str_radix(&s[2..3], 16).ok()? * 17,
        ),
        4 => (
            // #RGBA: trailing alpha ignored, 3 digits expanded x17
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
            // #RRGGBBAA: trailing 2 alpha digits ignored
            u8::from_str_radix(&s[0..2], 16).ok()?,
            u8::from_str_radix(&s[2..4], 16).ok()?,
            u8::from_str_radix(&s[4..6], 16).ok()?,
        ),
        _ => return None,
    };
    Some([r, g, bl])
}

/// Sanitize a CSS value (color/font etc.). Values containing dangerous functions
/// (url()/expression()/@import/javascript:) return the fallback; otherwise escape-capable
/// characters (;{}<>") are stripped. Pure function; rejections are logged via tracing::warn
/// (fail visibly). Pass fallback="transparent" for colors, "" for fonts.
pub fn sanitize_css_value(raw: &str, fallback: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("url(")
        || lower.contains("expression(")
        || lower.contains("@import")
        || lower.contains("javascript:")
    {
        tracing::warn!(
            raw = raw,
            "CSS value contains a dangerous function, degrading to fallback (offline constraint)"
        );
        return fallback.to_string();
    }
    raw.chars()
        .filter(|&c| !matches!(c, ';' | '{' | '}' | '<' | '>' | '"'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // A-2: CJK/emoji input must never panic (the old impl had a byte-slice out-of-bounds).
    #[test]
    fn parse_hex_color_ascii_gate_rejects_cjk() {
        assert_eq!(parse_hex_color("蓝"), None);
        assert_eq!(parse_hex_color("#中"), None);
        assert_eq!(parse_hex_color("#中文ab"), None);
        assert_eq!(parse_hex_color("😀"), None);
        // Mixed ASCII + CJK: any non-hex byte means rejection
        assert_eq!(parse_hex_color("#a蓝b"), None);
    }

    #[test]
    fn parse_hex_color_valid_forms() {
        assert_eq!(parse_hex_color("#fff"), Some([255, 255, 255]));
        assert_eq!(parse_hex_color("fff"), Some([255, 255, 255]));
        assert_eq!(parse_hex_color("#abcd"), Some([170, 187, 204])); // #RGBA, alpha ignored
        assert_eq!(parse_hex_color("#aabbcc"), Some([170, 187, 204]));
        assert_eq!(parse_hex_color("#aabbccff"), Some([170, 187, 204])); // #RRGGBBAA, alpha ignored
                                                                         // with whitespace
        assert_eq!(parse_hex_color("  #FF8800  "), Some([255, 136, 0]));
    }

    #[test]
    fn parse_hex_color_rejects_invalid_length() {
        assert_eq!(parse_hex_color("#12345"), None); // 5 digits
        assert_eq!(parse_hex_color("#1234567"), None); // 7 digits
        assert_eq!(parse_hex_color(""), None);
    }

    #[test]
    fn parse_hex_color_rejects_non_hex_chars() {
        assert_eq!(parse_hex_color("#gg0000"), None);
        assert_eq!(parse_hex_color("#12gh34"), None);
    }

    // A-3: dangerous functions return the fallback (hard offline constraint).
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
        // case-insensitive
        assert_eq!(
            sanitize_css_value("URL(HTTP://X)", "transparent"),
            "transparent"
        );
    }

    #[test]
    fn sanitize_css_value_strips_quote() {
        // C-6: `"` is used as a property escape point by the Tailwind arbitrary value syntax `bg-[{v}]`, must be stripped.
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
