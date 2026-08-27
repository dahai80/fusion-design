# fd-export

Fusion-Design 导出 crate — 把 `PenDocument` 画布模型批量导出为 PNG/SVG/PDF/HTML。

## 依赖与离线约束

- `resvg 0.45`（default-features=false，仅 `text` feature）— SVG 光栅化到 PNG。纯 Rust，无系统依赖。
- `printpdf 0.12`（default-features=false）— PDF 生成。纯 Rust，无 openssl/系统库。
- 100% 离线：无任何出站 HTTP，仅本地文件 IO + 系统字体探测。

## PDF 字体与 CJK（R-10）

`printpdf 0.12` 的 `BuiltinFont::Helvetica` 是 WinAnsi 编码，CJK 字符（中文/日文/韩文）会丢成 `.notdef` 不可见。fd-export 的 PDF 路径：

- `page_has_cjk_text(page)` 检测页面是否含 CJK 字符。
- 含 CJK 时探测系统字体（macOS 14+ 默认 `/System/Library/Fonts/PingFang.ttc`），用 `printpdf::font::ParsedFont::from_bytes` 内嵌 TTF/TTC，CJK 字符正确出字。
- 找不到系统字体 → `tracing::warn!` 显式告警 + 降级 Helvetica（CJK 不可见但 PDF 仍生成，fail visibly 不静默跳过）。
- 非多平台字体打包（版权 + 体积），运行时探测系统字体。Linux/Windows 需用户预装 CJK 字体到系统字体目录。

## printpdf 上游监控（E-19）

`printpdf` 0.12 是该 crate 当前唯一可用的纯 Rust PDF 生成库，但上游维护偏半停滞状态。已知短板：

- **CJK/Transform**：TTC 字体集合的 face index 选择、字体子集化策略有限，复杂 CJK 排版（竖排/合字）不支持。
- **上游节奏**：tag 发布间隔长，部分 PR 长期未合并。fork 活跃度不稳，无可靠升级目标。

监控策略：本轮不改依赖（无更好替代），记录此文件作为已知债。定期人工检查上游 `git@github.com:fschutt/printpdf.git` tag 与 changelog，若出现稳定新版（修复 CJK/子集化/性能）再评估升级。升级前需回归 `page_has_cjk_text` + 内嵌字体路径（测试见 `tests/` 或 inline `#[test]` PDF CJK 用例）。

## 导出格式

| 格式 | 后端 | 备注 |
|------|------|------|
| PNG | resvg 光栅化 SVG→PNG | 依赖 SVG 路径正确 |
| SVG | 自研 `render_element_svg` | R-5：`<image href>` 经 `sanitize_image_url` 拦截 `javascript:`/`data:text/html`/`http(s):` |
| PDF | printpdf 0.12 ops API | R-10 CJK 字体内嵌；R-11 跳过元素类型显式 warn 不静默 |
| HTML | 自研模板 | Token `var(--*)`/`token:*` 颜色解析为 hex |

## Token 颜色解析

`TokenValue::Color` 引用（`var(--color-bg)` / `token:color.bg`）在光栅化前解析为 hex。未解析的 token 引用留空字符串并 warn，避免渲染出 `var(--*)` 字面量。
