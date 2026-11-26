// ARCH-10 r4: shared utilities split out of lib.rs. escape_html/escape_attr/sanitize_css_color
// are used by both html/react paths; MAX_CODEGEN_DEPTH is used by all rendering paths. Zero behavior change.

use fd_canvas_core::sanitize_css_value;

// C3 defense in depth: codegen recursion depth guard. A4 already enforces MAX_NODE_DEPTH at the
// deserialization layer; this independent cap prevents stack overflow via any bypass path
// (e.g. a future new load entry point).
pub(super) const MAX_CODEGEN_DEPTH: usize = 128;

// HTML entity escaping (F1/XSS fix): node.text comes from LLM output or text parsed by
// parse-html — untrusted; splicing it verbatim into HTML/JSX would execute arbitrary scripts. Escape the 5 HTML special characters.
pub(super) fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

// E-10: CSS color value sanitization. fill/stroke come from LLM output or untrusted parse-html input;
// splicing them verbatim into `background:{fill};` / Tailwind `bg-[{fill}]` could escape the property and
// inject arbitrary CSS (e.g. `red;} * {position:fixed;background:url(http://evil)` breaking the offline constraint).
// Reject url() (hard offline constraint + external probing), strip `;{}` that could escape the property boundary, keep hex/named colors/rgb().
pub(super) fn sanitize_css_color(raw: &str) -> String {
    // C-6/A-3: shared sanitize_css_value, which additionally strips `"` compared to the old impl (an escape point in Tailwind arbitrary values bg-[{v}]).
    sanitize_css_value(raw, "transparent")
}

// E-10: node.id escaped via escape_html before safe use as an attribute value (escaping " prevents `data-id="a" onclick=...` injection).
pub(super) fn escape_attr(s: &str) -> String {
    escape_html(s)
}
