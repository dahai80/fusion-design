//! fd-server — Fusion-Design web service surface (issue#31 / DS-0).
//!
//! Server for the three-pane D1 workbench (chat generation/iteration + WASM canvas
//! three-viewport preview + version history + fd-codegen export): hyper inbound
//! listener on 127.0.0.1:11493 (port registered to fusion-design via port-registry),
//! exposing a JSON API + hosting the fd-host-web WASM canvas + embedded workbench
//! static page.
//!
//! Offline hard constraint: this crate **only accepts inbound connections**, zero
//! outbound HTTP — all model calls go through fd-ai-adapter (the only outbound
//! crate, gateway 11432 / MLX 11434). The deny-external-http gate allowlists
//! fd-server's inbound stack (hyper/hyper-util/http-body-util/http); client
//! libraries such as reqwest are forbidden.
//!
//! Version model: in-session version stack (in-memory, v1). The named-version API
//! is not revived (H-A14 lesson — the server-side session stack is a thin
//! standalone layer, not the canvas-core embedded-whole-snapshot approach).

pub mod assets;
pub mod handlers;
pub mod prompt;
pub mod router;
pub mod state;

use std::sync::Arc;

/// Default listening port (port-registry.yaml registered owner: fusion-design).
pub const DEFAULT_PORT: u16 = 11493;

/// Shared service state.
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

/// Starts the HTTP service (blocks the current async task until the listener
/// closes).
///
/// `port` comes from the FUSION_DESIGN_PORT override or DEFAULT_PORT; binds
/// 127.0.0.1 only (offline discipline: no 0.0.0.0 binding, registry rule 4).
pub async fn start_server(port: u16, state: Arc<AppState>) -> anyhow::Result<()> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "fusion-design-server listening");
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "accept failed, skipping connection");
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
                tracing::debug!(peer = %peer, error = %e, "http1 connection ended (non-fatal)");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_port_is_registry_allocated() {
        // port-registry.yaml: fusion-design 11493. Changing the port requires
        // updating this constant + the registry together.
        assert_eq!(DEFAULT_PORT, 11493);
    }

    #[test]
    fn app_state_has_builtin_design_systems() {
        let s = AppState::new();
        let reg = s.registry.lock().unwrap();
        assert_eq!(
            reg.list().len(),
            4,
            "4 built-in presets: apple-hig/minimal-dashboard/robot-sim/desigma-aligned"
        );
    }
}
