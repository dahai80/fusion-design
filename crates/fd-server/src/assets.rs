//! 静态资产：工作台页面编译期内嵌 + WASM 画布运行时查盘。
//!
//! HTML/CSS/JS include_str! 编译期嵌入（CI ubuntu 无 wasm 也能编译，零构建顺序耦合）。
//! fd_host_web_bg.wasm / .js 是 wasm-bindgen 后处理产物（编译期不可得），运行时
//! 按目录链查盘：FUSION_DESIGN_WASM_DIR env > 可执行文件同目录 > cwd >
//! target/wasm32-unknown-unknown/{release,debug}。缺失 404 + 诊断提示（fail visibly）。

use std::sync::OnceLock;

type Resp = hyper::Response<http_body_util::Full<hyper::body::Bytes>>;

fn text_response(body: &'static str, mime: &str) -> Resp {
    hyper::Response::builder()
        .status(hyper::StatusCode::OK)
        .header(hyper::header::CONTENT_TYPE, mime)
        .body(http_body_util::Full::new(hyper::body::Bytes::from(body)))
        .unwrap_or_default()
}

fn bytes_response(body: Vec<u8>, mime: &str) -> Resp {
    hyper::Response::builder()
        .status(hyper::StatusCode::OK)
        .header(hyper::header::CONTENT_TYPE, mime)
        .body(http_body_util::Full::new(hyper::body::Bytes::from(body)))
        .unwrap_or_default()
}

fn not_found_json(msg: &str) -> Resp {
    hyper::Response::builder()
        .status(hyper::StatusCode::NOT_FOUND)
        .header(
            hyper::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )
        .body(http_body_util::Full::new(hyper::body::Bytes::from(
            serde_json::json!({"error": msg}).to_string(),
        )))
        .unwrap_or_default()
}

pub fn index() -> Resp {
    text_response(include_str!("web/index.html"), "text/html; charset=utf-8")
}

pub fn workbench() -> Resp {
    text_response(
        include_str!("web/workbench.html"),
        "text/html; charset=utf-8",
    )
}

pub fn landing_js() -> Resp {
    text_response(
        include_str!("web/landing.js"),
        "application/javascript; charset=utf-8",
    )
}

pub fn app_js() -> Resp {
    text_response(
        include_str!("web/app.js"),
        "application/javascript; charset=utf-8",
    )
}

pub fn app_css() -> Resp {
    text_response(include_str!("web/app.css"), "text/css; charset=utf-8")
}

/// wasm-bindgen JS 胶水（查盘缓存）。
pub fn wasm_js() -> Resp {
    match load_wasm_asset("fd_host_web.js") {
        Some(bytes) => bytes_response(bytes, "application/javascript; charset=utf-8"),
        None => wasm_missing_hint("fd_host_web.js"),
    }
}

/// wasm 二进制（查盘缓存）。
pub fn wasm_bin() -> Resp {
    match load_wasm_asset("fd_host_web_bg.wasm") {
        Some(bytes) => bytes_response(bytes, "application/wasm"),
        None => wasm_missing_hint("fd_host_web_bg.wasm"),
    }
}

fn wasm_missing_hint(name: &str) -> Resp {
    let msg = format!(
        "{name} 未找到。生成方式：cargo build -p fd-host-web --release --target wasm32-unknown-unknown \
&& wasm-bindgen --target web --out-dir target/wasm32-unknown-unknown/release \
target/wasm32-unknown-unknown/release/fd_host_web.wasm；\
或设 FUSION_DESIGN_WASM_DIR 指向含 {name} 的目录"
    );
    tracing::error!(asset = name, "WASM 画布资产缺失，画布不可用");
    not_found_json(&msg)
}

/// wasm 资产查找目录链。
fn wasm_candidate_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(d) = std::env::var("FUSION_DESIGN_WASM_DIR") {
        dirs.push(std::path::PathBuf::from(d));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.to_path_buf());
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.clone());
        dirs.push(cwd.join("target/wasm32-unknown-unknown/release"));
        dirs.push(cwd.join("target/wasm32-unknown-unknown/debug"));
    }
    dirs
}

/// 按目录链查盘，成功后 OnceLock 缓存（进程内不重读）。
fn load_wasm_asset(name: &str) -> Option<Vec<u8>> {
    static JS: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    static BIN: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    let cell = if name == "fd_host_web.js" { &JS } else { &BIN };
    cell.get_or_init(|| {
        for dir in wasm_candidate_dirs() {
            let p = dir.join(name);
            match std::fs::read(&p) {
                Ok(bytes) => {
                    tracing::info!(path = %p.display(), bytes = bytes.len(), "WASM 资产已加载");
                    return Some(bytes);
                }
                Err(_) => continue,
            }
        }
        tracing::warn!(asset = name, "WASM 资产在所有候选目录均未找到");
        None
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workbench_embeds_html_skeleton() {
        // 编译期嵌入冒烟：D1 三栏骨架 + wasm 加载钩子必须在页内。
        let body = include_str!("web/workbench.html");
        assert!(body.contains("id=\"fd-chat\""), "左栏对话容器");
        assert!(body.contains("id=\"fd-canvas-stage\""), "中栏画布舞台");
        assert!(body.contains("id=\"fd-versions\""), "右栏版本列表");
        assert!(body.contains("fd-canvas-mount"), "mount 校验用 canvas 元素");
        // 生态出口按钮（issue#33/DS-2）：发给 Code / 存 Artifacts。
        assert!(body.contains("id=\"send-to-code\""), "发给 Code 按钮");
        assert!(body.contains("id=\"save-artifact\""), "存 Artifacts 按钮");
        // wasm 胶水由 app.js 动态 import，三栏骨架 + 桥容器齐备。
        let js = include_str!("web/app.js");
        assert!(js.contains("fd_host_web.js"), "app.js 须动态加载 wasm 胶水");
        assert!(
            js.contains("fd_host_web_bg.wasm"),
            "app.js 须加载 wasm 二进制"
        );
        assert!(js.contains("page.render-dom"), "须经桥命令驱动 DOM 渲染");
        assert!(
            js.contains("/api/send-to-code"),
            "app.js 须调 send-to-code 接口"
        );
        assert!(
            js.contains("/api/save-artifact"),
            "app.js 须调 save-artifact 接口"
        );
    }

    #[test]
    fn assets_and_api_are_module_relative() {
        // 经 gateway /shell/fusion-design/ 代理时，绝对路径 /app.css 会打到
        // gateway 根（404 → 整页无样式）。资源引用与 API 基路径必须相对模块前缀。
        let html = include_str!("web/index.html");
        assert!(
            html.contains("href=\"app.css\""),
            "样式表须用相对路径，绝对 /app.css 在 shell 模块内 404"
        );
        assert!(!html.contains("href=\"/app.css\""), "禁止绝对 /app.css");
        assert!(html.contains("src=\"landing.js\""), "脚本须用相对路径");
        assert!(
            !html.contains("src=\"/landing.js\""),
            "禁止绝对 /landing.js"
        );
        let js = include_str!("web/app.js");
        assert!(
            js.contains("import.meta.url"),
            "API 基路径须由模块 URL 推导，兼容 shell 前缀"
        );
        assert!(
            !js.contains("fetch(\"/api"),
            "禁止绝对 /api fetch，shell 模块内会打到 gateway 根"
        );
        let workbench = include_str!("web/workbench.html");
        assert!(workbench.contains("href=\"app.css\""), "工作台样式相对路径");
        assert!(workbench.contains("src=\"app.js\""), "工作台脚本相对路径");
        let landing_js = include_str!("web/landing.js");
        assert!(
            landing_js.contains("import.meta.url"),
            "落地页同样按模块基路径"
        );
        assert!(
            !landing_js.contains("fetch(\"/api"),
            "落地页禁止绝对 /api fetch"
        );
    }

    #[test]
    fn landing_page_has_claude_shape() {
        // 落地页结构：三 tab + 搜索 + 排序 + 三个创建卡片 + 列表分页。
        let body = include_str!("web/index.html");
        for id in [
            "tab-row",
            "search-input",
            "sort-select",
            "artifact-grid",
            "pager",
            "create-doc",
            "create-slides",
            "create-design",
        ] {
            assert!(body.contains(&format!("id=\"{id}\"")), "缺元素 {id}");
        }
        assert!(body.contains("data-i18n=\"tabAll\""), "全部 tab");
        assert!(body.contains("data-i18n=\"tabMine\""), "我的 tab");
        assert!(body.contains("data-i18n=\"tabShared\""), "与您共享 tab");
        // 三个创建卡片跳工作台并带 kind。
        for kind in ["document", "slides", "design"] {
            assert!(
                body.contains(&format!("workbench.html?kind={kind}")),
                "创建卡片 {kind} 须跳工作台"
            );
        }
        // 预填提示词：Claude 同款开头。
        let js = include_str!("web/landing.js");
        assert!(js.contains("Make a Design artifact for "), "设计预填");
        assert!(
            js.contains("Make a Claude Document artifact about "),
            "文档预填"
        );
        assert!(
            js.contains("Make a Claude Slides artifact about "),
            "幻灯片预填"
        );
    }

    #[test]
    fn candidate_dirs_cover_env_and_cwd() {
        let dirs = wasm_candidate_dirs();
        assert!(
            dirs.iter()
                .any(|d| d.ends_with("target/wasm32-unknown-unknown/release")),
            "须覆盖 bindgen release 产物目录"
        );
        assert!(
            dirs.iter()
                .any(|d| d.ends_with("target/wasm32-unknown-unknown/debug")),
            "须覆盖 bindgen debug 产物目录"
        );
    }
}
