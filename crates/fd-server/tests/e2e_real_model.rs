//! Real-model E2E (issue#31 acceptance): run manually with --ignored; requires
//! gateway :11432 online, a valid FUSION_CLIENT_KEY, and the vision model loaded.
//!
//! Run: cargo test -p fd-server --test e2e_real_model -- --ignored --test-threads=1
//! Covers the UC-D1 main flow: health(deep) -> generate -> iterate -> restore ->
//! export(html/react) -> version query -> rollback. The HTTP client is hand-written
//! with std TcpStream (fd-server forbids outbound HTTP libraries; the test follows
//! the same discipline).

use std::io::{Read, Write};
use std::net::TcpStream;

/// Hand-written HTTP/1.1 request (Connection: close, read to EOF).
fn http_req(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test server");
    let body_bytes = body.unwrap_or("").as_bytes();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body_bytes.len()
    );
    stream.write_all(req.as_bytes()).unwrap();
    stream.write_all(body_bytes).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let text = String::from_utf8_lossy(&raw).to_string();
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

/// Finds a free port (bind :0 then release immediately; tiny race window).
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn json_field(body: &str, key: &str) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_str(body).expect("response body must be JSON");
    v.get(key).cloned().unwrap_or(serde_json::Value::Null)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a real model: gateway 11432 online + FUSION_CLIENT_KEY + vision model loaded"]
async fn uc_d1_full_flow_with_real_model() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let port = free_port();
    let state = std::sync::Arc::new(fd_server::AppState::new());
    tokio::spawn(async move {
        let _ = fd_server::start_server(port, state).await;
    });
    // Wait for the listener to be ready.
    for _ in 0..50 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // 1. deep health: gateway + model + auth all pass.
    let (st, body) = http_req(port, "GET", "/api/health?deep=1", None);
    assert_eq!(st, 200, "health status: {body}");
    let mlx = json_field(&body, "mlx");
    assert_eq!(
        mlx.get("available").and_then(|v| v.as_bool()),
        Some(true),
        "deep health must probe available: {body}"
    );
    assert_eq!(
        mlx.get("auth_ok").and_then(|v| v.as_bool()),
        Some(true),
        "deep health auth must pass: {body}"
    );

    // 2. generate: login page.
    let (st, body) = http_req(
        port,
        "POST",
        "/api/generate",
        Some(r#"{"prompt":"Login page with an email + password form, indigo primary color"}"#),
    );
    assert_eq!(st, 200, "generate status: {body}");
    let sid = json_field(&body, "session")
        .as_str()
        .expect("session id")
        .to_string();
    assert!(sid.starts_with("s-"), "session id shape: {sid}");
    assert!(
        json_field(&body, "document")
            .as_str()
            .unwrap_or("")
            .contains("pages"),
        "generated document must contain pages"
    );
    assert_eq!(
        json_field(&body, "current_version").as_u64(),
        Some(1),
        "first version must be v1"
    );

    // 3. iterate: full-page rewrite flow.
    let (st, body) = http_req(
        port,
        "POST",
        "/api/iterate",
        Some(&format!(
            r#"{{"session":"{sid}","instruction":"Change the primary button text to Sign In"}}"#
        )),
    );
    assert_eq!(st, 200, "iterate status: {body}");
    assert_eq!(
        json_field(&body, "current_version").as_u64(),
        Some(2),
        "iteration must produce v2: {body}"
    );
    assert_eq!(
        json_field(&body, "session").as_str().unwrap_or(""),
        sid,
        "same session"
    );

    // 4. restore: screenshot restore (fixtures/sample-ui.png base64).
    let png = include_bytes!("fixtures/sample-ui.png");
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    let (st, body) = http_req(
        port,
        "POST",
        "/api/restore",
        Some(&format!(
            r#"{{"image_base64":"{b64}","hint":"A navigation bar plus two cards"}}"#
        )),
    );
    assert_eq!(st, 200, "restore status: {body}");
    let rsid = json_field(&body, "session")
        .as_str()
        .expect("restore session")
        .to_string();
    assert_ne!(rsid, sid, "restore must create a new session");
    assert!(
        json_field(&body, "document")
            .as_str()
            .unwrap_or("")
            .contains("pages"),
        "restored document must contain pages"
    );

    // 5. export: html + react (against the generated session).
    let (st, body) = http_req(
        port,
        "POST",
        "/api/export",
        Some(&format!(r#"{{"session":"{sid}","format":"html"}}"#)),
    );
    assert_eq!(st, 200, "export html status: {body}");
    assert!(
        json_field(&body, "content")
            .as_str()
            .unwrap_or("")
            .contains("<!DOCTYPE html>"),
        "HTML export must be a complete document"
    );
    let (st, body) = http_req(
        port,
        "POST",
        "/api/export",
        Some(&format!(r#"{{"session":"{sid}","format":"react"}}"#)),
    );
    assert_eq!(st, 200, "export react status: {body}");
    assert!(
        json_field(&body, "content")
            .as_str()
            .unwrap_or("")
            .contains("export"),
        "React export must be component code"
    );

    // 6. version query + rollback to v1.
    let (st, body) = http_req(port, "GET", &format!("/api/version/{sid}/1"), None);
    assert_eq!(st, 200, "version query status: {body}");
    assert!(
        json_field(&body, "document")
            .as_str()
            .unwrap_or("")
            .contains("pages"),
        "v1 document must be queryable"
    );
    let (st, body) = http_req(
        port,
        "POST",
        "/api/rollback",
        Some(&format!(r#"{{"session":"{sid}","version":1}}"#)),
    );
    assert_eq!(st, 200, "rollback status: {body}");
    assert_eq!(
        json_field(&body, "current_version").as_u64().unwrap_or(0),
        1,
        "current=1 after rollback"
    );

    // 7. session list contains both sessions.
    let (st, body) = http_req(port, "GET", "/api/sessions", None);
    assert_eq!(st, 200, "sessions status: {body}");
    assert!(
        body.contains(&sid) && body.contains(&rsid),
        "session list must contain both sessions: {body}"
    );
}
