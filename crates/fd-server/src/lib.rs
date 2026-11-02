//! fd-server — Fusion-Design web 服务面（issue#31 / DS-0）。
//!
//! 三栏 D1 工作台（对话生成/迭代 + WASM 画布三视口预览 + 版本回溯 + fd-codegen 导出）
//! 的服务端：hyper 入站监听 127.0.0.1:11493（端口走 port-registry 登记属主 fusion-design），
//! 暴露 JSON API + 托管 fd-host-web WASM 画布 + 内嵌工作台静态页。
//!
//! 离线硬约束：本 crate **只有入站监听**，零出站 HTTP——所有模型调用经
//! fd-ai-adapter（唯一出站 crate，gateway 11432 / MLX 11434）。deny-external-http
//! 门禁对 fd-server 白名单入站栈（hyper/hyper-util/http-body-util/http），
//! 禁止 reqwest 等客户端库。
//!
//! 版本模型：会话内版本栈（内存态，v1）。命名版本 API 不复活（H-A14 教训——
//! 服务端会话栈是独立薄层，非 canvas-core 内嵌整快照方案）。

pub mod assets;
pub mod handlers;
pub mod prompt;
pub mod router;
pub mod state;

use std::sync::Arc;

/// 默认监听端口（port-registry.yaml 登记属主 fusion-design）。
pub const DEFAULT_PORT: u16 = 11493;

/// 服务共享状态。
pub struct AppState {
    pub sessions: state::SessionStore,
    pub registry: std::sync::Mutex<fd_design_system::DesignSystemRegistry>,
}

impl AppState {
    pub fn new() -> Self {
        let mut reg = fd_design_system::DesignSystemRegistry::new();
        reg.register_builtin();
        Self {
            sessions: state::SessionStore::new(),
            registry: std::sync::Mutex::new(reg),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// 启动 HTTP 服务（阻塞当前 async 任务直至 listener 关闭）。
///
/// `port` 来自 FUSION_DESIGN_PORT 覆盖或 DEFAULT_PORT；只绑 127.0.0.1
/// （离线纪律：无 0.0.0.0 绑定，registry 规则 4）。
pub async fn start_server(port: u16, state: Arc<AppState>) -> anyhow::Result<()> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "fusion-design-server listening");
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "accept 失败，跳过该连接");
                continue;
            }
        };
        let state = state.clone();
        let io = hyper_util::rt::TokioIo::new(stream);
        tokio::spawn(async move {
            let service = hyper::service::service_fn(move |req| {
                let state = state.clone();
                async move { router::handle(req, state).await }
            });
            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, service)
                .await
            {
                tracing::debug!(peer = %peer, error = %e, "http1 连接结束（非致命）");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_port_is_registry_allocated() {
        // port-registry.yaml：fusion-design 11493。改端口须同步改本常量 + registry。
        assert_eq!(DEFAULT_PORT, 11493);
    }

    #[test]
    fn app_state_has_builtin_design_systems() {
        let s = AppState::new();
        let reg = s.registry.lock().unwrap();
        assert_eq!(
            reg.list().len(),
            3,
            "3 内置预设：apple-hig/minimal-dashboard/robot-sim"
        );
    }
}
