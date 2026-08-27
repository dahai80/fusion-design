//! A-3：CheckMlx 子命令 handler（从 main.rs 拆出）。
//!
//! 校验 fusion-mlx endpoint 是否为 localhost + 真推理探针（破 gateway 假绿）。

pub async fn run(endpoint: String, model: String) -> anyhow::Result<()> {
    let resolved = fd_ai_adapter::FusionMlxClient::resolve_endpoint(&endpoint)?;
    let client = fd_ai_adapter::FusionMlxClient::with_endpoints(resolved.clone())?;
    println!("endpoint: {}", resolved.join(","));
    let status = client.health_check().await?;
    let json = serde_json::to_string_pretty(&status)?;
    println!("{json}");
    // 鉴权或不可达直接失败——无须再探真推理。
    if !matches!(status.auth_ok, Some(true)) || !status.available {
        match status.status.as_deref() {
            Some(s) => anyhow::bail!("❌ fusion-mlx 不可用：{s}"),
            None => anyhow::bail!("❌ fusion-mlx 不可用"),
        }
    }
    // 通过 /v1/models 列表后，仍可能「假绿」：gateway 列了模型名但 MLX 未加载。
    // 用真推理探针（1 token）做最终判定。模型解析：--model > FUSION_MLX_MODEL > 列表首个。
    let model = match model.trim() {
        "" => match std::env::var("FUSION_MLX_MODEL") {
            Ok(m) if !m.trim().is_empty() => m.trim().to_string(),
            _ => status
                .model
                .clone()
                .unwrap_or_else(|| "default".to_string()),
        },
        other => other.to_string(),
    };
    println!("\n[推理探针] model = {model}");
    let probe = client.check_generate(&model).await?;
    let pjson = serde_json::to_string_pretty(&probe)?;
    println!("{pjson}");
    if probe.available {
        println!("✅ fusion-mlx 服务可用（推理探针通过）");
        Ok(())
    } else {
        match probe.status.as_deref() {
            Some(s) => anyhow::bail!("❌ fusion-mlx 不可用：{s}"),
            None => anyhow::bail!("❌ fusion-mlx 推理探针失败"),
        }
    }
}
