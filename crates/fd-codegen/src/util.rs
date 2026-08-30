// ARCH-10 r4：通用工具从 lib.rs 拆出。escape_html/escape_attr/sanitize_css_color
// 被 html/react 路径共用，MAX_CODEGEN_DEPTH 被所有渲染路径共用。零行为变更。

use fd_canvas_core::sanitize_css_value;

// C3 纵深防御：codegen 递归深度守卫。A4 已在反序列化层卡 MAX_NODE_DEPTH，
// 此处独立上限防任何绕过路径（如未来新增加载入口）的栈溢出。
pub(super) const MAX_CODEGEN_DEPTH: usize = 128;

// HTML 实体转义（F1/XSS 修复）：node.text 来自 LLM 输出或 parse-html 解析的
// 不可信文本，原样拼进 HTML/JSX 会执行任意脚本。转义 5 个 HTML 特殊字符。
pub(super) fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

// E-10：CSS 颜色值净化。fill/stroke 来自 LLM 输出或 parse-html 不可信输入，
// 原样拼进 `background:{fill};` / Tailwind `bg-[{fill}]` 可逃逸属性注入任意 CSS
// （如 `red;} * {position:fixed;background:url(http://evil)` 破离线约束）。
// 拒绝 url()（离线硬约束 + 外网探测），剔除可逃逸属性边界的 `;{}`，保留 hex/命名色/rgb()。
pub(super) fn sanitize_css_color(raw: &str) -> String {
    // C-6/A-3：共享 sanitize_css_value，比旧实现多剥 `"`（Tailwind 任意值 bg-[{v}] 逃逸点）。
    sanitize_css_value(raw, "transparent")
}

// E-10：node.id 经 escape_html 转义后安全用于属性值（转 " 防 `data-id="a" onclick=…` 注入）。
pub(super) fn escape_attr(s: &str) -> String {
    escape_html(s)
}
