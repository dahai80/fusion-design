//! Static assets: workbench pages embedded at compile time + WASM canvas runtime
//! disk lookup.
//!
//! HTML/CSS/JS embedded at compile time via include_str! (CI ubuntu can compile
//! without wasm, zero build-order coupling). fd_host_web_bg.wasm / .js are
//! wasm-bindgen post-processing artifacts (unavailable at compile time), looked
//! up on disk at runtime along the directory chain: FUSION_DESIGN_WASM_DIR env >
//! exe directory > cwd > target/wasm32-unknown-unknown/{release,debug}. Missing
//! assets 404 + diagnostic hint (fail visibly).

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

pub fn artifact_page() -> Resp {
    text_response(
        include_str!("web/artifact.html"),
        "text/html; charset=utf-8",
    )
}

pub fn artifact_js() -> Resp {
    text_response(
        include_str!("web/artifact.js"),
        "application/javascript; charset=utf-8",
    )
}

pub fn shell_sync_js() -> Resp {
    text_response(
        include_str!("web/shell-sync.js"),
        "application/javascript; charset=utf-8",
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

/// wasm-bindgen JS glue (disk-lookup cache).
pub fn wasm_js() -> Resp {
    match load_wasm_asset("fd_host_web.js") {
        Some(bytes) => bytes_response(bytes, "application/javascript; charset=utf-8"),
        None => wasm_missing_hint("fd_host_web.js"),
    }
}

/// wasm binary (disk-lookup cache).
pub fn wasm_bin() -> Resp {
    match load_wasm_asset("fd_host_web_bg.wasm") {
        Some(bytes) => bytes_response(bytes, "application/wasm"),
        None => wasm_missing_hint("fd_host_web_bg.wasm"),
    }
}

fn wasm_missing_hint(name: &str) -> Resp {
    let msg = format!(
        "{name} not found. Generate it with: cargo build -p fd-host-web --release --target wasm32-unknown-unknown \
&& wasm-bindgen --target web --out-dir target/wasm32-unknown-unknown/release \
target/wasm32-unknown-unknown/release/fd_host_web.wasm; \
or set FUSION_DESIGN_WASM_DIR to a directory containing {name}"
    );
    tracing::error!(
        asset = name,
        "WASM canvas asset missing, canvas unavailable"
    );
    not_found_json(&msg)
}

/// Directory chain for wasm asset lookup.
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

/// Looks up the directory chain; caches via OnceLock on success (no re-reads
/// within the process).
fn load_wasm_asset(name: &str) -> Option<Vec<u8>> {
    static JS: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    static BIN: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    let cell = if name == "fd_host_web.js" { &JS } else { &BIN };
    cell.get_or_init(|| {
        for dir in wasm_candidate_dirs() {
            let p = dir.join(name);
            match std::fs::read(&p) {
                Ok(bytes) => {
                    tracing::info!(path = %p.display(), bytes = bytes.len(), "WASM asset loaded");
                    return Some(bytes);
                }
                Err(_) => continue,
            }
        }
        tracing::warn!(
            asset = name,
            "WASM asset not found in any candidate directory"
        );
        None
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workbench_embeds_html_skeleton() {
        // Compile-time embedding smoke test: D1 three-pane skeleton + wasm load
        // hooks must be present in the page.
        let body = include_str!("web/workbench.html");
        assert!(body.contains("id=\"fd-chat\""), "left pane chat container");
        assert!(
            body.contains("id=\"fd-canvas-stage\""),
            "middle pane canvas stage"
        );
        assert!(
            body.contains("id=\"fd-versions\""),
            "right pane version list"
        );
        assert!(
            body.contains("fd-canvas-mount"),
            "canvas element used for mount verification"
        );
        // Ecosystem exit buttons (issue#33/DS-2): send to Code / save to Artifacts.
        assert!(body.contains("id=\"send-to-code\""), "send-to-code button");
        assert!(
            body.contains("id=\"save-artifact\""),
            "save-artifact button"
        );
        // wasm glue is dynamically imported by app.js; three-pane skeleton +
        // bridge container must be present.
        let js = include_str!("web/app.js");
        assert!(
            js.contains("fd_host_web.js"),
            "app.js must dynamically load the wasm glue"
        );
        assert!(
            js.contains("fd_host_web_bg.wasm"),
            "app.js must load the wasm binary"
        );
        assert!(
            js.contains("page.render-dom"),
            "DOM rendering must be driven via bridge command"
        );
        assert!(
            js.contains("/api/send-to-code"),
            "app.js must call the send-to-code endpoint"
        );
        assert!(
            js.contains("/api/save-artifact"),
            "app.js must call the save-artifact endpoint"
        );
    }

    #[test]
    fn assets_and_api_are_module_relative() {
        // When proxied via gateway /shell/fusion-design/, absolute /app.css would
        // hit the gateway root (404 -> whole page unstyled). Asset references and
        // the API base path must be relative to the module prefix.
        let html = include_str!("web/index.html");
        assert!(
            html.contains("href=\"app.css\""),
            "stylesheet must use a relative path; absolute /app.css 404s inside a shell module"
        );
        assert!(
            !html.contains("href=\"/app.css\""),
            "absolute /app.css is forbidden"
        );
        assert!(
            html.contains("src=\"landing.js\""),
            "scripts must use relative paths"
        );
        assert!(
            !html.contains("src=\"/landing.js\""),
            "absolute /landing.js is forbidden"
        );
        let js = include_str!("web/app.js");
        assert!(
            js.contains("import.meta.url"),
            "API base path must be derived from the module URL, compatible with the shell prefix"
        );
        assert!(
            !js.contains("fetch(\"/api"),
            "absolute /api fetch is forbidden; inside a shell module it hits the gateway root"
        );
        let workbench = include_str!("web/workbench.html");
        assert!(
            workbench.contains("href=\"app.css\""),
            "workbench stylesheet must be relative"
        );
        assert!(
            workbench.contains("src=\"app.js\""),
            "workbench script must be relative"
        );
        let landing_js = include_str!("web/landing.js");
        assert!(
            landing_js.contains("import.meta.url"),
            "landing page must also use the module base path"
        );
        assert!(
            !landing_js.contains("fetch(\"/api"),
            "landing page must not use absolute /api fetch"
        );
    }

    #[test]
    fn landing_page_has_claude_shape() {
        // Landing page structure: three tabs + search + sort + three create
        // cards + list pagination.
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
            assert!(
                body.contains(&format!("id=\"{id}\"")),
                "missing element {id}"
            );
        }
        assert!(body.contains("data-i18n=\"tabAll\""), "all tab");
        assert!(body.contains("data-i18n=\"tabMine\""), "mine tab");
        assert!(
            body.contains("data-i18n=\"tabShared\""),
            "shared-with-you tab"
        );
        // Three create cards navigate to the workbench with a kind.
        for kind in ["document", "slides", "design"] {
            assert!(
                body.contains(&format!("workbench.html?kind={kind}")),
                "create card {kind} must navigate to the workbench"
            );
        }
        // Pre-filled prompts: interaction shape aligned, but must not claim to be
        // Claude (this project is not an Anthropic product).
        let js = include_str!("web/landing.js");
        assert!(js.contains("Make a Design artifact for "), "design prefill");
        assert!(
            js.contains("Make a Document artifact about "),
            "document prefill"
        );
        assert!(
            js.contains("Make a Slides artifact about "),
            "slides prefill"
        );
        assert!(!js.contains("Claude"), "prompts must not mention Claude");
    }

    #[test]
    fn artifact_viewer_embeds_skeleton() {
        // Viewer: view content in place + download + back to workbench to
        // continue; all assets relative paths.
        let body = include_str!("web/artifact.html");
        for id in [
            "artifact-frame",
            "artifact-code",
            "artifact-title",
            "artifact-export",
            "artifact-open-workbench",
        ] {
            assert!(
                body.contains(&format!("id=\"{id}\"")),
                "missing element {id}"
            );
        }
        assert!(
            body.contains("href=\"app.css\""),
            "viewer stylesheet must be relative"
        );
        assert!(
            body.contains("src=\"artifact.js\""),
            "viewer script must be relative"
        );
        let js = include_str!("web/artifact.js");
        assert!(
            js.contains("import.meta.url"),
            "viewer must derive the API base from the module path"
        );
        assert!(
            !js.contains("fetch(\"/api"),
            "viewer must not use absolute /api fetch"
        );
        assert!(
            js.contains("api/artifact/"),
            "viewer must call api/artifact/<id>"
        );
        assert!(
            js.contains("srcdoc"),
            "HTML content must be rendered via iframe"
        );
        let landing = include_str!("web/landing.js");
        assert!(
            landing.contains("artifact.html?id="),
            "card click must open the viewer"
        );
    }

    #[test]
    fn pages_follow_shell_theme_and_lang_standard() {
        // spec §2.1: theme/lang are managed centrally by the shell; modules must
        // not implement or duplicate them — pages must not ship their own
        // lang-select / theme-toggle, nor introduce separate fd-lang/fd-theme keys.
        for html in [
            include_str!("web/index.html"),
            include_str!("web/workbench.html"),
            include_str!("web/artifact.html"),
        ] {
            assert!(
                !html.contains("lang-select"),
                "module-provided language switch is forbidden"
            );
            assert!(
                !html.contains("theme-toggle"),
                "module-provided theme toggle is forbidden"
            );
        }
        for js in [
            include_str!("web/landing.js"),
            include_str!("web/app.js"),
            include_str!("web/artifact.js"),
        ] {
            assert!(
                !js.contains("fd-lang"),
                "separate language key fd-lang is forbidden"
            );
            assert!(
                !js.contains("fd-theme"),
                "separate theme key fd-theme is forbidden"
            );
            assert!(
                js.contains("shell-sync.js"),
                "all three pages must follow the shell via shell-sync"
            );
        }
        let sync = include_str!("web/shell-sync.js");
        assert!(
            sync.contains("fusion.shell.theme"),
            "fallback must use the shell convention key"
        );
        assert!(
            sync.contains("fusion.shell.lang"),
            "fallback must use the shell convention key"
        );
        assert!(
            sync.contains("fusion:theme"),
            "must listen for shell theme events"
        );
        assert!(
            sync.contains("fusion:lang"),
            "must listen for shell lang events"
        );
    }

    #[test]
    fn workbench_has_model_select() {
        // Workbench must allow model switching: the dropdown excludes embedding
        // models (bge etc.), defaulting to chat_model.
        let html = include_str!("web/workbench.html");
        assert!(
            html.contains("id=\"model-select\""),
            "missing model dropdown"
        );
        let js = include_str!("web/app.js");
        assert!(
            js.contains("chat_model"),
            "must default to the server-side chat_model"
        );
        assert!(
            js.contains("model: state.model || undefined"),
            "generate/iterate must carry the selected model"
        );
        assert!(
            js.contains("const fam ="),
            "must filter by chat model family"
        );
        assert!(
            js.contains("const bad ="),
            "must exclude non-chat models (image/video/TTS/embedding)"
        );
    }

    #[test]
    fn workbench_has_shell_module_ux() {
        // issue#42 shell modularization: Token panel + save .fusiondesign +
        // embedded mode + drag persistence.
        let html = include_str!("web/workbench.html");
        assert!(html.contains("id=\"fd-tokens\""), "missing Token panel");
        assert!(
            html.contains("id=\"save-file\""),
            "missing save .fusiondesign button"
        );
        let js = include_str!("web/app.js");
        assert!(js.contains("loadTokenPanel"), "must load the Token panel");
        assert!(
            js.contains("applyToken"),
            "must support applying a Token to the selected node"
        );
        assert!(
            js.contains("onWasmNodeDrag"),
            "must consume node.drag to persist drags"
        );
        assert!(
            js.contains("saveFile"),
            "must be able to save .fusiondesign"
        );
        assert!(
            js.contains("embedded"),
            "must support ?embedded=1 to hide its own navigation"
        );
        assert!(
            js.contains("resp.status === 401") || js.contains("resp.status===401"),
            "401 must show an inline error, no redirect (#28 contract)"
        );
        assert!(
            !js.contains("location.href"),
            "page-level navigation is forbidden inside a module"
        );
        let css = include_str!("web/app.css");
        assert!(css.contains(".token-item"), "missing Token item style");
        assert!(css.contains("body.embedded"), "missing embedded-mode style");
        let manifest = include_str!("../../../web/manifest.json");
        assert!(
            manifest.contains("\"id\": \"fusion-design\""),
            "missing shell manifest"
        );
        assert!(manifest.contains("\"entry\""), "manifest missing entry");
    }

    #[test]
    fn workbench_has_canvas_ux_minimum() {
        // issue#35 minimum canvas UX: layer list + node selection sync + visible
        // undo/redo state.
        let html = include_str!("web/workbench.html");
        assert!(
            html.contains("id=\"fd-layers\""),
            "missing layer list panel"
        );
        assert!(html.contains("id=\"undo-btn\""), "missing undo button");
        assert!(html.contains("id=\"redo-btn\""), "missing redo button");
        let js = include_str!("web/app.js");
        assert!(
            js.contains("node.select"),
            "must drive wasm selection highlight"
        );
        assert!(
            js.contains("pollWasmEvents"),
            "must consume canvas click events to sync layers"
        );
        assert!(
            js.contains("stepVersion"),
            "must have version stack stepping (undo/redo)"
        );
        assert!(
            js.contains("updateUndoRedo"),
            "undo/redo must have a visible disabled state"
        );
        let css = include_str!("web/app.css");
        assert!(css.contains(".layer-item"), "missing layer item style");
    }

    #[test]
    fn candidate_dirs_cover_env_and_cwd() {
        let dirs = wasm_candidate_dirs();
        assert!(
            dirs.iter()
                .any(|d| d.ends_with("target/wasm32-unknown-unknown/release")),
            "must cover the bindgen release output directory"
        );
        assert!(
            dirs.iter()
                .any(|d| d.ends_with("target/wasm32-unknown-unknown/debug")),
            "must cover the bindgen debug output directory"
        );
    }
}
