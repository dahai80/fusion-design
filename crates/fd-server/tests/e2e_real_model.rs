//! 真模型 E2E（issue#31 验收）：--ignored 手动运行，须 gateway :11432 在线
//! 且 FUSION_CLIENT_KEY 有效、视觉模型已加载。
//!
//! 运行：cargo test -p fd-server --test e2e_real_model -- --ignored --test-threads=1
//! 覆盖 UC-D1 主链路：health(deep) → generate → iterate → restore →
//! export(html/react) → version 查询 → rollback。HTTP 客户端用 std TcpStream
//! 手写（fd-server 禁出站 HTTP 库，测试同口径）。

use std::io::{Read, Write};
use std::net::TcpStream;

/// 手写 HTTP/1.1 请求（Connection: close，读至 EOF）。
fn http_req(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("连接测试服务器");
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

/// 找空闲端口（bind :0 后立刻释放，竞态窗口极小）。
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn json_field(body: &str, key: &str) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_str(body).expect("响应体须为 JSON");
    v.get(key).cloned().unwrap_or(serde_json::Value::Null)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "需真模型：gateway 11432 在线 + FUSION_CLIENT_KEY + 视觉模型已加载"]
async fn uc_d1_full_flow_with_real_model() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let port = free_port();
    let state = std::sync::Arc::new(fd_server::AppState::new());
    tokio::spawn(async move {
        let _ = fd_server::start_server(port, state).await;
    });
    // 等监听就绪。
    for _ in 0..50 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // 1. deep health：gateway + 模型 + 鉴权全通。
    let (st, body) = http_req(port, "GET", "/api/health?deep=1", None);
    assert_eq!(st, 200, "health 状态码: {body}");
    let mlx = json_field(&body, "mlx");
    assert_eq!(
        mlx.get("available").and_then(|v| v.as_bool()),
        Some(true),
        "deep health 须探测可用: {body}"
    );
    assert_eq!(
        mlx.get("auth_ok").and_then(|v| v.as_bool()),
        Some(true),
        "deep health 鉴权须通过: {body}"
    );

    // 2. generate：登录页。
    let (st, body) = http_req(
        port,
        "POST",
        "/api/generate",
        Some(r#"{"prompt":"登录页，邮箱+密码表单，主色 indigo"}"#),
    );
    assert_eq!(st, 200, "generate 状态码: {body}");
    let sid = json_field(&body, "session")
        .as_str()
        .expect("session id")
        .to_string();
    assert!(sid.starts_with("s-"), "会话 id 形状: {sid}");
    assert!(
        json_field(&body, "document")
            .as_str()
            .unwrap_or("")
            .contains("pages"),
        "生成的文档须含 pages"
    );
    assert_eq!(
        json_field(&body, "current_version").as_u64(),
        Some(1),
        "首版须为 v1"
    );

    // 3. iterate：全页重写链路。
    let (st, body) = http_req(
        port,
        "POST",
        "/api/iterate",
        Some(&format!(
            r#"{{"session":"{sid}","instruction":"把主按钮文案改成 Sign In"}}"#
        )),
    );
    assert_eq!(st, 200, "iterate 状态码: {body}");
    assert_eq!(
        json_field(&body, "current_version").as_u64(),
        Some(2),
        "迭代须产 v2: {body}"
    );
    assert_eq!(
        json_field(&body, "session").as_str().unwrap_or(""),
        sid,
        "同会话"
    );

    // 4. restore：截图还原（fixtures/sample-ui.png base64）。
    let png = include_bytes!("fixtures/sample-ui.png");
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    let (st, body) = http_req(
        port,
        "POST",
        "/api/restore",
        Some(&format!(
            r#"{{"image_base64":"{b64}","hint":"导航栏+两张卡片"}}"#
        )),
    );
    assert_eq!(st, 200, "restore 状态码: {body}");
    let rsid = json_field(&body, "session")
        .as_str()
        .expect("restore session")
        .to_string();
    assert_ne!(rsid, sid, "还原须新会话");
    assert!(
        json_field(&body, "document")
            .as_str()
            .unwrap_or("")
            .contains("pages"),
        "还原文档须含 pages"
    );

    // 5. export：html + react（对生成会话）。
    let (st, body) = http_req(
        port,
        "POST",
        "/api/export",
        Some(&format!(r#"{{"session":"{sid}","format":"html"}}"#)),
    );
    assert_eq!(st, 200, "export html 状态码: {body}");
    assert!(
        json_field(&body, "content")
            .as_str()
            .unwrap_or("")
            .contains("<!DOCTYPE html>"),
        "HTML 导出须为完整文档"
    );
    let (st, body) = http_req(
        port,
        "POST",
        "/api/export",
        Some(&format!(r#"{{"session":"{sid}","format":"react"}}"#)),
    );
    assert_eq!(st, 200, "export react 状态码: {body}");
    assert!(
        json_field(&body, "content")
            .as_str()
            .unwrap_or("")
            .contains("export"),
        "React 导出须为组件代码"
    );

    // 6. version 查询 + rollback 到 v1。
    let (st, body) = http_req(port, "GET", &format!("/api/version/{sid}/1"), None);
    assert_eq!(st, 200, "version 查询状态码: {body}");
    assert!(
        json_field(&body, "document")
            .as_str()
            .unwrap_or("")
            .contains("pages"),
        "v1 文档须可查"
    );
    let (st, body) = http_req(
        port,
        "POST",
        "/api/rollback",
        Some(&format!(r#"{{"session":"{sid}","version":1}}"#)),
    );
    assert_eq!(st, 200, "rollback 状态码: {body}");
    assert_eq!(
        json_field(&body, "current_version").as_u64().unwrap_or(0),
        1,
        "回退后 current=1"
    );

    // 7. 会话列表含两会话。
    let (st, body) = http_req(port, "GET", "/api/sessions", None);
    assert_eq!(st, 200, "sessions 状态码: {body}");
    assert!(
        body.contains(&sid) && body.contains(&rsid),
        "会话列表须含两会话: {body}"
    );
}
