//! fusion-design-server 入口（issue#31 / DS-0）。
//!
//! 端口：FUSION_DESIGN_PORT > DEFAULT_PORT(11493，port-registry 登记属主 fusion-design)。
//! 只绑 127.0.0.1（离线纪律）。日志 stdout（start.sh 重定向落盘）。

use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let port: u16 = match std::env::var("FUSION_DESIGN_PORT") {
        Ok(p) => p
            .trim()
            .parse()
            .map_err(|e| anyhow::anyhow!("FUSION_DESIGN_PORT={p:?} 非法端口: {e}"))?,
        Err(_) => fd_server::DEFAULT_PORT,
    };

    let state = Arc::new(fd_server::AppState::new());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(fd_server::start_server(port, state))
}
