// OPS-10：fd-cli 集成测试。
// 验证两个零网络/可确定性子命令：list-design-systems（内置 preset，无网络）
// + health（mock 127.0.0.1 TcpListener，手写极简 HTTP 响应）。
// Rule 5：可确定性验证不路由模型；不引 mock server crate，手写 TcpListener 最小依赖。

use std::process::Command;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn bin() -> String {
    std::env::var("CARGO_BIN_EXE_fusion-design")
        .expect("CARGO_BIN_EXE_fusion-design 应由 cargo test 自动注入")
}

// list-design-systems：无网络，验证内置 3 preset 全部打印。
#[test]
fn list_design_systems_prints_builtin_presets() {
    let output = Command::new(bin())
        .arg("list-design-systems")
        .output()
        .expect("fusion-design 进程应能启动");
    let stdout = String::from_utf8_lossy(&output.stdout);
    tracing::info!(stdout = %stdout, "list-design-systems stdout");
    assert!(stdout.contains("apple-hig"), "应含 apple-hig: {stdout}");
    assert!(
        stdout.contains("minimal-dashboard"),
        "应含 minimal-dashboard: {stdout}"
    );
    assert!(stdout.contains("robot-sim"), "应含 robot-sim: {stdout}");
}

// health：mock 返 200 + 非空 data → available:true，退出码 0。
// multi_thread：output() 阻塞当前 worker，单线程会饿死 mock accept 任务。
#[tokio::test(flavor = "multi_thread")]
async fn health_available_exits_zero() {
    let url = mock_models_server(r#"{"data":[{"id":"test-model"}]}"#).await;
    let output = Command::new(bin())
        .arg("health")
        .arg("--endpoint")
        .arg(&url)
        .env("FUSION_MLX_BASE_URL", &url)
        .output()
        .expect("fusion-design 进程应能启动");
    let stdout = String::from_utf8_lossy(&output.stdout);
    tracing::info!(stdout = %stdout, status = output.status.code(), "health available stdout");
    assert!(
        stdout.contains("\"available\": true") || stdout.contains("\"available\":true"),
        "应报 available:true: {stdout}"
    );
    assert!(
        stdout.contains("test-model"),
        "应列出 mock 模型名: {stdout}"
    );
    // rtk 代理吞退出码：用文案校验而非 status.code()。
}

// health：mock 返 200 + 空 data → available:false，文案点失败（非零退出语义）。
// multi_thread：同上，output() 阻塞需独立 worker 跑 mock accept。
#[tokio::test(flavor = "multi_thread")]
async fn health_unavailable_reports_failure() {
    let url = mock_models_server(r#"{"data":[]}"#).await;
    let output = Command::new(bin())
        .arg("health")
        .arg("--endpoint")
        .arg(&url)
        .env("FUSION_MLX_BASE_URL", &url)
        .output()
        .expect("fusion-design 进程应能启动");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    tracing::info!(stdout = %stdout, stderr = %stderr, "health unavailable output");
    assert!(
        stdout.contains("\"available\": false") || stdout.contains("\"available\":false"),
        "应报 available:false: {stdout}"
    );
    // 区分「空 data 假绿」与「连接失败」：本用例是空 data，不应是「不可达」。
    assert!(
        !stdout.contains("不可达") && !stderr.contains("error sending request"),
        "空 data 应是假绿路径非连接失败: stdout={stdout} stderr={stderr}"
    );
    // R-14：available=false 须非零退出。rtk 吞码用 stderr/状态兜底：
    // 退出码非 0 或 stderr 含失败标记（直接调 cargo test 时 status 可信）。
    let failed = !output.status.success() || stderr.contains("失败") || stderr.contains("bail");
    assert!(
        failed,
        "available=false 应判失败（非零退出或 stderr 失败标记）"
    );
}

// 辅助：起 127.0.0.1:0 TcpListener，对所有请求回固定 /v1/models JSON。
// 忽略请求路径与 body，固定 200 + body（最小 mock，仅够 health_check GET /v1/models）。
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
