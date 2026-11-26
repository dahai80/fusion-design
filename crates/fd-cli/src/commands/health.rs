//! A-3: Health subcommand handler (split out of main.rs).
//!
//! Probes fusion-mlx health. R-14: available=false false-greens must exit non-zero.

pub async fn run(endpoint: String) -> anyhow::Result<()> {
    let client = fd_ai_adapter::FusionMlxClient::with_endpoints(
        fd_ai_adapter::FusionMlxClient::resolve_endpoint(&endpoint)?,
    )?;
    let status = client.health_check().await;
    // R-14: Ok(status) but available=false is also a false green (the service responded
    // but is unusable) and must exit non-zero. Aligns with check-mlx semantics: print the
    // diagnostic JSON (script-parseable) first, then bail.
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
        tracing::error!(
            "MLX health check failed: service unavailable (available=false or request error)"
        );
        anyhow::bail!("MLX health check failed (see the JSON diagnostics above)");
    }
    Ok(())
}
