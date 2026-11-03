//! 请求路由：静态资产 + /api/* JSON 端点。
//!
//! 手写分发（无 axum）：路径少、形状固定，最小依赖面。
//! 请求体读取带 REQUEST_CAP 上限（8MB，与 IPC 护栏同口径）。

use std::sync::Arc;

use http_body_util::BodyExt;
use hyper::body::Incoming;

use crate::{assets, handlers, AppState};

type Resp = hyper::Response<http_body_util::Full<hyper::body::Bytes>>;

/// 主入口：按 (method, path) 分发。
pub async fn handle(
    req: hyper::Request<Incoming>,
    state: Arc<AppState>,
) -> Result<Resp, std::convert::Infallible> {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or_default().to_string();

    let resp = match (method.as_str(), path.as_str()) {
        ("GET", "/") | ("GET", "/index.html") => assets::index(),
        ("GET", "/workbench.html") => assets::workbench(),
        ("GET", "/landing.js") => assets::landing_js(),
        ("GET", "/shell-sync.js") => assets::shell_sync_js(),
        ("GET", "/api/artifacts") => handlers::artifacts(&query).await,
        ("GET", "/artifact.html") => assets::artifact_page(),
        ("GET", "/artifact.js") => assets::artifact_js(),
        ("GET", p) if p.starts_with("/api/artifact/") => {
            let id = p.trim_start_matches("/api/artifact/").to_string();
            let version = query_param(&query, "version").and_then(|v| v.parse::<u32>().ok());
            handlers::artifact_detail(&id, version).await
        }
        ("GET", "/app.js") => assets::app_js(),
        ("GET", "/app.css") => assets::app_css(),
        ("GET", "/fd_host_web.js") => assets::wasm_js(),
        ("GET", "/fd_host_web_bg.wasm") => assets::wasm_bin(),
        ("GET", "/api/health") => handlers::health(&state, query.contains("deep=1")).await,
        ("GET", "/api/design-systems") => handlers::design_systems(&state),
        ("GET", "/api/sessions") => handlers::sessions(&state),
        ("POST", "/api/generate") => match read_body(req).await {
            Ok(b) => handlers::generate(&state, &b).await,
            Err(r) => r,
        },
        ("POST", "/api/iterate") => match read_body(req).await {
            Ok(b) => handlers::iterate(&state, &b).await,
            Err(r) => r,
        },
        ("POST", "/api/restore") => match read_body(req).await {
            Ok(b) => handlers::restore(&state, &b).await,
            Err(r) => r,
        },
        ("POST", "/api/rollback") => match read_body(req).await {
            Ok(b) => handlers::rollback(&state, &b),
            Err(r) => r,
        },
        ("POST", "/api/export") => match read_body(req).await {
            Ok(b) => handlers::export(&state, &b),
            Err(r) => r,
        },
        ("POST", "/api/send-to-code") => match read_body(req).await {
            Ok(b) => handlers::send_to_code(&state, &b),
            Err(r) => r,
        },
        ("POST", "/api/save-artifact") => match read_body(req).await {
            Ok(b) => handlers::save_artifact(&state, &b).await,
            Err(r) => r,
        },
        ("GET", p) if p.starts_with("/api/tokens") => {
            let ds = query_param(&query, "ds").unwrap_or_else(|| "apple-hig".to_string());
            let theme = query_param(&query, "theme").unwrap_or_else(|| "light".to_string());
            handlers::tokens(&state, &ds, &theme)
        }
        ("GET", p) if p.starts_with("/api/session/") => {
            let id = p.trim_start_matches("/api/session/");
            handlers::session_info(&state, id)
        }
        ("GET", p) if p.starts_with("/api/version/") => {
            // /api/version/<session>/<n>：session id 自身含连字符（s-<hex>-<seq>），
            // 故按段拆（id 可能含 '-'，n 是最后一段）。
            let rest = p.trim_start_matches("/api/version/");
            match rest.rsplit_once('/') {
                Some((id, ver)) => match ver.parse::<u32>() {
                    Ok(v) => handlers::version_doc(&state, id, v),
                    Err(_) => bad_request("版本号非法（须为正整数）"),
                },
                None => bad_request("路径须为 /api/version/<session>/<version>"),
            }
        }
        (m, p) if p.starts_with("/api/") => {
            // issue#36：未知 /api/* 路径必须 404（验收 2026-11-03 误把 405
            // 当成"路由注册方法错误"）。405 只留给"路径存在但方法不对"。
            if known_api_path(p) {
                method_not_allowed(m, p)
            } else {
                not_found()
            }
        }
        _ => not_found(),
    };
    Ok(resp)
}

/// 已知 API 路径判定：区分 404（路径不存在）与 405（路径存在、方法不对）。
/// 新增路由须同步维护本清单 + docs/API.md + 契约测试。
fn known_api_path(path: &str) -> bool {
    const EXACT: [&str; 10] = [
        "/api/health",
        "/api/design-systems",
        "/api/sessions",
        "/api/generate",
        "/api/iterate",
        "/api/restore",
        "/api/rollback",
        "/api/export",
        "/api/send-to-code",
        "/api/save-artifact",
    ];
    const PREFIX: [&str; 5] = [
        "/api/artifacts",
        "/api/artifact/",
        "/api/tokens",
        "/api/session/",
        "/api/version/",
    ];
    EXACT.contains(&path) || PREFIX.iter().any(|p| path.starts_with(p))
}

fn not_found() -> Resp {
    hyper::Response::builder()
        .status(hyper::StatusCode::NOT_FOUND)
        .body(http_body_util::Full::new(hyper::body::Bytes::from(
            serde_json::json!({"error": "not found"}).to_string(),
        )))
        .unwrap_or_default()
}

/// issue#35 契约项：405 带 hint——tokens 等只读端点仅支持 GET，
/// 消费方不必逆向猜方法。响应形状 {"error","method","path","hint"?}。
fn method_not_allowed(method: &str, path: &str) -> Resp {
    let hint = if path.starts_with("/api/tokens") {
        Some("GET /api/tokens?ds=<id>&theme=light|dark".to_string())
    } else if path.starts_with("/api/health")
        || path.starts_with("/api/design-systems")
        || path.starts_with("/api/sessions")
        || path.starts_with("/api/artifacts")
    {
        Some(format!("GET {path}"))
    } else {
        None
    };
    let mut body = serde_json::json!({
        "error": format!("无 {method} {path} 路由"),
        "method": method,
        "path": path,
    });
    if let Some(h) = hint {
        body["hint"] = serde_json::json!(h);
    }
    hyper::Response::builder()
        .status(hyper::StatusCode::METHOD_NOT_ALLOWED)
        .body(http_body_util::Full::new(hyper::body::Bytes::from(
            body.to_string(),
        )))
        .unwrap_or_default()
}

/// 读请求体（上限 REQUEST_CAP）。失败/超限返回 Err(错误响应)。
async fn read_body(req: hyper::Request<Incoming>) -> Result<hyper::body::Bytes, Resp> {
    let cap = handlers::request_cap();
    let body = match req.into_body().collect().await {
        Ok(c) => c.to_bytes(),
        Err(e) => {
            return Err(hyper::Response::builder()
                .status(hyper::StatusCode::BAD_REQUEST)
                .body(http_body_util::Full::new(hyper::body::Bytes::from(
                    serde_json::json!({"error": format!("请求体读取失败: {e}")}).to_string(),
                )))
                .unwrap_or_default());
        }
    };
    if body.len() > cap {
        tracing::warn!(len = body.len(), cap, "请求体超上限，拒绝");
        return Err(hyper::Response::builder()
            .status(hyper::StatusCode::PAYLOAD_TOO_LARGE)
            .body(http_body_util::Full::new(hyper::body::Bytes::from(
                serde_json::json!({"error": format!("请求体超过 {cap} 字节上限")}).to_string(),
            )))
            .unwrap_or_default());
    }
    Ok(body)
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| urldecode(v))
    })
}

/// 极简 percent-decode（query 值仅设计系统 id/theme 等词表值，容错足够）。
fn urldecode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(b) = u8::from_str_radix(&hex, 16) {
                out.push(b as char);
                continue;
            }
            out.push('%');
        } else {
            out.push(c);
        }
    }
    out
}

fn bad_request(msg: &str) -> Resp {
    hyper::Response::builder()
        .status(hyper::StatusCode::BAD_REQUEST)
        .body(http_body_util::Full::new(hyper::body::Bytes::from(
            serde_json::json!({"error": msg}).to_string(),
        )))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_param_extracts_and_decodes() {
        assert_eq!(
            query_param("ds=apple-hig&theme=dark", "ds").as_deref(),
            Some("apple-hig")
        );
        assert_eq!(
            query_param("ds=apple-hig&theme=dark", "theme").as_deref(),
            Some("dark")
        );
        assert_eq!(query_param("a=1", "b"), None);
        assert_eq!(urldecode("%2Fx"), "/x");
    }

    #[tokio::test]
    async fn method_not_allowed_contract() {
        // POST /api/tokens（issue#35 验收报 405 的形状）：405 + hint 指路 GET。
        let resp = method_not_allowed("POST", "/api/tokens");
        assert_eq!(resp.status(), hyper::StatusCode::METHOD_NOT_ALLOWED);
        use http_body_util::BodyExt;
        let b = resp
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(v["method"], "POST");
        assert_eq!(v["path"], "/api/tokens");
        assert!(
            v["hint"]
                .as_str()
                .unwrap_or_default()
                .starts_with("GET /api/tokens"),
            "{v}"
        );
        // 无 hint 的写端点：不带 hint 键。
        let resp = method_not_allowed("GET", "/api/generate");
        assert_eq!(resp.status(), hyper::StatusCode::METHOD_NOT_ALLOWED);
    }

    #[test]
    fn unknown_api_paths_are_404_not_405() {
        // issue#36：/api/v1/* 是 artifacts-engine 风格路径，fd-server 无此面；
        // 未知路径必须 404，不得误报 405（验收据此误判"方法注册错误"）。
        assert!(!known_api_path("/api/v1/health"));
        assert!(!known_api_path("/api/v1/designs"));
        assert!(!known_api_path("/api/v1/share-base"));
        assert!(!known_api_path("/api/share/abc"));
        // 已知路径（含前缀路由）须命中。
        assert!(known_api_path("/api/health"));
        assert!(known_api_path("/api/tokens"));
        assert!(known_api_path("/api/artifacts"));
        assert!(known_api_path("/api/artifact/a-1"));
        assert!(known_api_path("/api/session/s-x"));
        assert!(known_api_path("/api/version/s-x/2"));
        assert!(known_api_path("/api/generate"));
    }
}
