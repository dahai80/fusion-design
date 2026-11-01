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
            Ok(b) => handlers::save_artifact(&state, &b),
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
        (m, p) if p.starts_with("/api/") => hyper::Response::builder()
            .status(hyper::StatusCode::METHOD_NOT_ALLOWED)
            .body(http_body_util::Full::new(hyper::body::Bytes::from(
                serde_json::json!({"error": format!("无 {m} {p} 路由")}).to_string(),
            )))
            .unwrap_or_default(),
        _ => hyper::Response::builder()
            .status(hyper::StatusCode::NOT_FOUND)
            .body(http_body_util::Full::new(hyper::body::Bytes::from(
                serde_json::json!({"error": "not found"}).to_string(),
            )))
            .unwrap_or_default(),
    };
    Ok(resp)
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
}
