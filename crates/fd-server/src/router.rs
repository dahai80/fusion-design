//! Request routing: static assets + /api/* JSON endpoints.
//!
//! Hand-written dispatch (no axum): few paths, fixed shapes, minimal dependency
//! surface. Request body reads enforce the REQUEST_CAP limit (8MB, same as the
//! IPC guardrail).

use std::sync::Arc;

use http_body_util::BodyExt;
use hyper::body::Incoming;

use crate::{assets, handlers, AppState};

type Resp = hyper::Response<http_body_util::Full<hyper::body::Bytes>>;

/// Main entry: dispatches by (method, path).
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
        ("GET", "/api/design-intel/search") => {
            // issue#44: design-intelligence dataset search (embedded CSV, deterministic scoring).
            let q = query_param(&query, "q").unwrap_or_default();
            let kind = query_param(&query, "kind").unwrap_or_else(|| "all".to_string());
            let limit = query_param(&query, "limit")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(5);
            handlers::design_intel_search(&q, &kind, limit)
        }
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
        ("POST", p) if p.starts_with("/api/session/") && p.ends_with("/mutate") => {
            // issue#42: direct canvas mutation (drag/Token panel persisted), no model round-trip.
            let id = p
                .trim_start_matches("/api/session/")
                .trim_end_matches("/mutate");
            if id.is_empty() {
                bad_request("path must be /api/session/<id>/mutate")
            } else {
                match read_body(req).await {
                    Ok(b) => handlers::node_mutate(&state, id, &b),
                    Err(r) => r,
                }
            }
        }
        ("GET", p) if p.starts_with("/api/session/") => {
            let id = p.trim_start_matches("/api/session/");
            handlers::session_info(&state, id)
        }
        ("GET", p) if p.starts_with("/api/version/") => {
            // /api/version/<session>/<n>: the session id itself contains hyphens
            // (s-<hex>-<seq>), so split from the right (id may contain '-'; n is
            // the last segment).
            let rest = p.trim_start_matches("/api/version/");
            match rest.rsplit_once('/') {
                Some((id, ver)) => match ver.parse::<u32>() {
                    Ok(v) => handlers::version_doc(&state, id, v),
                    Err(_) => bad_request("invalid version number (must be a positive integer)"),
                },
                None => bad_request("path must be /api/version/<session>/<version>"),
            }
        }
        (m, p) if p.starts_with("/api/") => {
            // issue#36: unknown /api/* paths must 404 (acceptance on 2026-11-03
            // misread 405 as "route registered with wrong method"). 405 is only
            // for "path exists but method is wrong".
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

/// Known API path check: distinguishes 404 (path does not exist) from 405 (path
/// exists, method wrong). New routes must keep this list + docs/API.md + contract
/// tests in sync.
fn known_api_path(path: &str) -> bool {
    const EXACT: [&str; 11] = [
        "/api/health",
        "/api/design-systems",
        "/api/sessions",
        "/api/design-intel/search",
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

/// issue#35 contract item: 405 carries a hint — read-only endpoints such as
/// tokens only support GET, so consumers don't have to reverse-engineer the
/// method. Response shape {"error","method","path","hint"?}.
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
        "error": format!("no route for {method} {path}"),
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

/// Reads the request body (capped at REQUEST_CAP). Returns Err(error response)
/// on failure or when over the cap.
async fn read_body(req: hyper::Request<Incoming>) -> Result<hyper::body::Bytes, Resp> {
    let cap = handlers::request_cap();
    let body = match req.into_body().collect().await {
        Ok(c) => c.to_bytes(),
        Err(e) => {
            return Err(hyper::Response::builder()
                .status(hyper::StatusCode::BAD_REQUEST)
                .body(http_body_util::Full::new(hyper::body::Bytes::from(
                    serde_json::json!({"error": format!("failed to read request body: {e}")})
                        .to_string(),
                )))
                .unwrap_or_default());
        }
    };
    if body.len() > cap {
        tracing::warn!(len = body.len(), cap, "request body exceeds cap, rejecting");
        return Err(hyper::Response::builder()
            .status(hyper::StatusCode::PAYLOAD_TOO_LARGE)
            .body(http_body_util::Full::new(hyper::body::Bytes::from(
                serde_json::json!({"error": format!("request body exceeds the {cap} byte limit")})
                    .to_string(),
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

/// Minimal percent-decode (query values are only vocabulary values such as
/// design-system id/theme, so this tolerance is sufficient).
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
        // POST /api/tokens (the 405 shape reported by the issue#35 acceptance): 405 + hint pointing to GET.
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
        // Write endpoints without a hint: no hint key.
        let resp = method_not_allowed("GET", "/api/generate");
        assert_eq!(resp.status(), hyper::StatusCode::METHOD_NOT_ALLOWED);
    }

    #[test]
    fn unknown_api_paths_are_404_not_405() {
        // issue#36: /api/v1/* is artifacts-engine style; fd-server has no such surface.
        // Unknown paths must 404, not misreport 405 (acceptance misjudged this as
        // "method registration error").
        assert!(!known_api_path("/api/v1/health"));
        assert!(!known_api_path("/api/v1/designs"));
        assert!(!known_api_path("/api/v1/share-base"));
        assert!(!known_api_path("/api/share/abc"));
        // Known paths (including prefix routes) must match.
        assert!(known_api_path("/api/health"));
        assert!(known_api_path("/api/tokens"));
        assert!(known_api_path("/api/artifacts"));
        assert!(known_api_path("/api/artifact/a-1"));
        assert!(known_api_path("/api/session/s-x"));
        assert!(known_api_path("/api/version/s-x/2"));
        assert!(known_api_path("/api/generate"));
    }
}
