//! Fusion-Design 宿主桥 — 嵌入 Fusion-Desk WKWebView 的通信层。
//!
//! 对应 PRD 展示层「Fusion-Desk 内置 WKWebView 渲染无限矢量画布」。
//! 负责：
//! - 加载编译后的前端静态资源（file:// 协议，禁止网络）
//! - 提供 WKWebView ↔ 本地后端服务的消息转发
//! - 拦截所有外网请求（离线硬约束）

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// WKWebView 消息方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageDirection {
    /// WKWebView → 本地后端
    WebViewToBackend,
    /// 本地后端 → WKWebView
    BackendToWebView,
}

/// 宿主桥消息体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostMessage {
    pub direction: MessageDirection,
    pub kind: String,
    pub payload: serde_json::Value,
}

/// 宿主桥配置。
#[derive(Debug, Clone)]
pub struct HostBridgeConfig {
    /// 前端静态资源目录（file:// 加载入口）。
    pub frontend_dir: PathBuf,
    /// 本地后端服务 endpoint（必须 127.0.0.1）。
    pub backend_endpoint: String,
    /// 是否拦截所有外网请求（默认 true，离线硬约束）。
    pub block_external: bool,
}

impl HostBridgeConfig {
    /// 校验配置合规性（离线硬约束）。
    pub fn validate(&self) -> Result<(), HostConfigError> {
        if !self.frontend_dir.exists() {
            return Err(HostConfigError::FrontendMissing(self.frontend_dir.clone()));
        }
        if !self.frontend_dir.join("index.html").exists() {
            return Err(HostConfigError::IndexMissing(self.frontend_dir.clone()));
        }
        if !self.block_external {
            tracing::warn!("block_external=false 违反离线硬约束");
            return Err(HostConfigError::OfflineConstraintViolated);
        }
        validate_localhost(&self.backend_endpoint)?;
        Ok(())
    }

    /// 返回前端入口的 file:// URL。
    pub fn index_url(&self) -> String {
        let path = self.frontend_dir.join("index.html");
        format!("file://{}", path.display())
    }
}

/// 校验 endpoint 为 localhost。
fn validate_localhost(endpoint: &str) -> Result<(), HostConfigError> {
    let url = reqwest::Url::parse(endpoint)
        .map_err(|e| HostConfigError::InvalidEndpoint(e.to_string()))?;
    let host = url.host_str().unwrap_or("");
    // reqwest::Url 对 IPv6 返回形如 "[::1]"，需去方括号比对
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host != "127.0.0.1" && host != "localhost" && host != "::1" {
        return Err(HostConfigError::PublicEndpoint(host.to_string()));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum HostConfigError {
    #[error("前端目录不存在: {0}")]
    FrontendMissing(PathBuf),
    #[error("前端目录缺少 index.html: {0}")]
    IndexMissing(PathBuf),
    #[error("无效 endpoint: {0}")]
    InvalidEndpoint(String),
    #[error("违反离线约束：endpoint host {0} 非 localhost")]
    PublicEndpoint(String),
    #[error("违反离线硬约束：block_external=false，离线模式不可关闭")]
    OfflineConstraintViolated,
}

/// 外网请求拦截器（供 WKWebView configuration 注入）。
///
/// 在真实 Swift 宿主中，对应 `WKURLSchemeHandler` 注入；
/// 此处提供 Rust 侧的判定逻辑，供后端服务复用。
pub fn is_external_url(url: &str) -> bool {
    // F2/离线强制：allowlist 判定。
    //   内部 = file://（本地资源，离线前端入口）或 http/https 且 host∈白名单。
    //   外部 = javascript:/data:（无 host 的危险 scheme，XSS 载体）、解析失败。
    // 旧实现判无 host 的 javascript:/data: 为"内部/允许"，是 XSS 放行漏洞。
    match reqwest::Url::parse(url) {
        Ok(u) => {
            let scheme = u.scheme();
            if scheme == "file" {
                return false; // 本地文件资源，离线允许
            }
            let host = u.host_str().unwrap_or("");
            let internal_host = host == "127.0.0.1" || host == "localhost" || host == "::1";
            !(scheme == "http" || scheme == "https") || !internal_host
        }
        Err(_) => true, // 非法 URL 一律视为外网，保守拒绝
    }
}

/// 返回前端静态资源目录的合规校验助手。
pub fn ensure_frontend_dir(dir: &Path) -> anyhow::Result<()> {
    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
    }
    let index = dir.join("index.html");
    if !index.exists() {
        std::fs::write(&index, "<!DOCTYPE html><title>Fusion-Design</title>\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn is_external_url_flags_public() {
        assert!(is_external_url("https://api.openai.com"));
        assert!(is_external_url("http://10.0.0.1:80"));
    }

    #[test]
    fn is_external_url_allows_localhost() {
        assert!(!is_external_url("http://127.0.0.1:8080"));
        assert!(!is_external_url("http://localhost:9000"));
    }

    #[test]
    fn is_external_url_allows_file_rejects_dangerous_schemes() {
        // file:// 本地资源：离线允许
        assert!(!is_external_url("file:///path/index.html"));
        // F2 回归：javascript:/data: 无 host 危险 scheme 必须判外网（旧实现放行 = XSS 漏洞）
        assert!(is_external_url("javascript:alert(1)"));
        assert!(is_external_url("data:text/html,<script>alert(1)</script>"));
        // 非法 URL 保守判外网
        assert!(is_external_url("not-a-url"));
    }

    #[test]
    fn config_validate_rejects_missing_frontend() {
        let cfg = HostBridgeConfig {
            frontend_dir: PathBuf::from("/nonexistent/path"),
            backend_endpoint: "http://127.0.0.1:8080".into(),
            block_external: true,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn config_validate_rejects_missing_index() {
        let tmp = tempdir().unwrap();
        let cfg = HostBridgeConfig {
            frontend_dir: tmp.path().to_path_buf(),
            backend_endpoint: "http://127.0.0.1:8080".into(),
            block_external: true,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn config_validate_rejects_public_endpoint() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("index.html"), "<html></html>").unwrap();
        let cfg = HostBridgeConfig {
            frontend_dir: tmp.path().to_path_buf(),
            backend_endpoint: "http://1.2.3.4:80".into(),
            block_external: true,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn config_validate_passes_valid() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("index.html"), "<html></html>").unwrap();
        let cfg = HostBridgeConfig {
            frontend_dir: tmp.path().to_path_buf(),
            backend_endpoint: "http://127.0.0.1:8080".into(),
            block_external: true,
        };
        cfg.validate().unwrap();
    }

    // F2 回归：离线是硬约束，block_external=false 必须拒绝启动（旧实现仅 warn）。
    #[test]
    fn config_validate_rejects_block_external_false() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("index.html"), "<html></html>").unwrap();
        let cfg = HostBridgeConfig {
            frontend_dir: tmp.path().to_path_buf(),
            backend_endpoint: "http://127.0.0.1:8080".into(),
            block_external: false,
        };
        assert!(cfg.validate().is_err(), "block_external=false 必须拒绝（离线硬约束）");
    }

    #[test]
    fn index_url_uses_file_protocol() {
        let tmp = tempdir().unwrap();
        let cfg = HostBridgeConfig {
            frontend_dir: tmp.path().to_path_buf(),
            backend_endpoint: "http://127.0.0.1:8080".into(),
            block_external: true,
        };
        let url = cfg.index_url();
        assert!(url.starts_with("file://"));
        assert!(url.ends_with("index.html"));
    }

    #[test]
    fn ensure_frontend_dir_creates_index() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("frontend");
        assert!(!dir.exists());
        ensure_frontend_dir(&dir).unwrap();
        assert!(dir.join("index.html").exists());
    }

    #[test]
    fn host_message_serde_roundtrip() {
        let msg = HostMessage {
            direction: MessageDirection::WebViewToBackend,
            kind: "ai.generate".into(),
            payload: serde_json::json!({"prompt": "登录页"}),
        };
        let s = serde_json::to_string(&msg).unwrap();
        let m2: HostMessage = serde_json::from_str(&s).unwrap();
        assert_eq!(m2.kind, "ai.generate");
    }
}
