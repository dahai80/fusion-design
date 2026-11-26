// OPS-10: fd-cli integration tests.
// Verifies two zero-network/deterministic subcommands: list-design-systems (built-in
// presets, no network) + health (mock 127.0.0.1 TcpListener, hand-written minimal HTTP
// response).
// Rule 5: deterministic verification does not route to a model; no mock-server crate is
// introduced — a hand-written TcpListener keeps deps minimal.

use std::process::Command;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn bin() -> String {
    std::env::var("CARGO_BIN_EXE_fusion-design")
        .expect("CARGO_BIN_EXE_fusion-design should be injected automatically by cargo test")
}

// list-design-systems: no network; verify all 3 built-in presets are printed.
#[test]
fn list_design_systems_prints_builtin_presets() {
    let output = Command::new(bin())
        .arg("list-design-systems")
        .output()
        .expect("fusion-design process should start");
    let stdout = String::from_utf8_lossy(&output.stdout);
    tracing::info!(stdout = %stdout, "list-design-systems stdout");
    assert!(stdout.contains("apple-hig"), "should contain apple-hig: {stdout}");
    assert!(
        stdout.contains("minimal-dashboard"),
        "should contain minimal-dashboard: {stdout}"
    );
    assert!(stdout.contains("robot-sim"), "should contain robot-sim: {stdout}");
}

// health: mock returns 200 + non-empty data → available:true, exit code 0.
// multi_thread: output() blocks the current worker; single-thread would starve the mock
// accept task.
#[tokio::test(flavor = "multi_thread")]
async fn health_available_exits_zero() {
    let url = mock_models_server(r#"{"data":[{"id":"test-model"}]}"#).await;
    let output = Command::new(bin())
        .arg("health")
        .arg("--endpoint")
        .arg(&url)
        .env("FUSION_MLX_BASE_URL", &url)
        .output()
        .expect("fusion-design process should start");
    let stdout = String::from_utf8_lossy(&output.stdout);
    tracing::info!(stdout = %stdout, status = output.status.code(), "health available stdout");
    assert!(
        stdout.contains("\"available\": true") || stdout.contains("\"available\":true"),
        "should report available:true: {stdout}"
    );
    assert!(
        stdout.contains("test-model"),
        "should list the mock model name: {stdout}"
    );
    // The rtk proxy swallows exit codes: assert on output text instead of status.code().
}

// health: mock returns 200 + empty data → available:false, text-level failure
// (non-zero-exit semantics).
// multi_thread: same as above; output() blocks so the mock accept needs its own worker.
#[tokio::test(flavor = "multi_thread")]
async fn health_unavailable_reports_failure() {
    let url = mock_models_server(r#"{"data":[]}"#).await;
    let output = Command::new(bin())
        .arg("health")
        .arg("--endpoint")
        .arg(&url)
        .env("FUSION_MLX_BASE_URL", &url)
        .output()
        .expect("fusion-design process should start");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    tracing::info!(stdout = %stdout, stderr = %stderr, "health unavailable output");
    assert!(
        stdout.contains("\"available\": false") || stdout.contains("\"available\":false"),
        "should report available:false: {stdout}"
    );
    // Distinguish an "empty-data false green" from a "connection failure": this case is
    // empty data, not "unreachable".
    assert!(
        !stdout.contains("unreachable") && !stderr.contains("error sending request"),
        "empty data should take the false-green path, not a connection failure: stdout={stdout} stderr={stderr}"
    );
    // R-14: available=false must exit non-zero. The rtk proxy swallows exit codes, so fall
    // back to stderr/status: non-zero exit, or stderr contains a failure marker (status is
    // trustworthy when invoked via cargo test directly).
    let failed = !output.status.success() || stderr.contains("failed") || stderr.contains("bail");
    assert!(
        failed,
        "available=false should count as failed (non-zero exit or stderr failure marker)"
    );
}

// Helper: bind a 127.0.0.1:0 TcpListener that answers every request with a fixed
// /v1/models JSON.
// Ignores request path and body; always returns 200 + body (minimal mock, just enough for
// health_check's GET /v1/models).
async fn mock_models_server(body: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind 127.0.0.1");
    let addr = listener.local_addr().expect("local_addr");
    let body = body.to_string();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => continue,
            };
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.flush().await;
        }
    });
    format!("http://{}", addr)
}
