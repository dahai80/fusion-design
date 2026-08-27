//! A-3：Health 子命令 handler（从 main.rs 拆出）。
//!
//! 探测 fusion-mlx 健康状态。R-14：available=false 假绿须非零退出。

pub async fn run(endpoint: String) -> anyhow::Result<()> {
    let client = fd_ai_adapter::FusionMlxClient::with_endpoints(
        fd_ai_adapter::FusionMlxClient::resolve_endpoint(&endpoint)?,
    )?;
    let status = client.health_check().await;
    // R-14：Ok(status) 但 available=false 也是假绿（服务应答但不可用），
    // 须非零退出码。对齐 check-mlx 语义：先打印诊断 JSON（脚本可解析），再 bail。
    let (output, failed) = match status {
        Ok(s) => {
            let not_available = !s.available;
            (serde_json::to_string_pretty(&s)?, not_available)
        }
        Err(e) => (
            serde_json::to_string_pretty(&serde_json::json!({
                "available": false,
                "error": e.to_string()
            }))?,
            true,
        ),
    };
    println!("{output}");
    if failed {
        tracing::error!("MLX 健康检查失败：服务不可用（available=false 或请求错误）");
        anyhow::bail!("MLX 健康检查失败（详见上方 JSON 诊断）");
    }
    Ok(())
}
