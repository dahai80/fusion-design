//! A-3: CheckMlx subcommand handler (split out of main.rs).
//!
//! Validates that the fusion-mlx endpoint is localhost + real inference probe (breaks
//! gateway false-greens).

pub async fn run(endpoint: String, model: String) -> anyhow::Result<()> {
    let resolved = fd_ai_adapter::FusionMlxClient::resolve_endpoint(&endpoint)?;
    let client = fd_ai_adapter::FusionMlxClient::with_endpoints(resolved.clone())?;
    println!("endpoint: {}", resolved.join(","));
    let status = client.health_check().await?;
    let json = serde_json::to_string_pretty(&status)?;
    println!("{json}");
    // Auth failure or unreachable fails immediately — no need to probe real inference.
    if !matches!(status.auth_ok, Some(true)) || !status.available {
        match status.status.as_deref() {
            Some(s) => anyhow::bail!("❌ fusion-mlx unavailable: {s}"),
            None => anyhow::bail!("❌ fusion-mlx unavailable"),
        }
    }
    // Even after the /v1/models listing, a "false green" is still possible: the gateway
    // lists model names but MLX has not loaded them. Use a real inference probe (1 token)
    // as the final verdict. Model resolution: --model > FUSION_MLX_MODEL > first in list.
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
    println!("\n[inference probe] model = {model}");
    let probe = client.check_generate(&model).await?;
    let pjson = serde_json::to_string_pretty(&probe)?;
    println!("{pjson}");
    if probe.available {
        println!("✅ fusion-mlx service available (inference probe passed)");
        Ok(())
    } else {
        match probe.status.as_deref() {
            Some(s) => anyhow::bail!("❌ fusion-mlx unavailable: {s}"),
            None => anyhow::bail!("❌ fusion-mlx inference probe failed"),
        }
    }
}
