//! Fusion-Design 宿主桥 — 嵌入 Fusion-Desk WKWebView 的通信层。
//!
//! 对应 PRD 展示层「Fusion-Desk 内置 WKWebView 渲染无限矢量画布」。
//! 负责：
//! - 加载编译后的前端静态资源（file:// 协议，禁止网络）
//! - 提供 WKWebView ↔ 本地后端服务的消息转发
//! - 拦截所有外网请求（离线硬约束）

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 宿主桥协议版本。studio↔wasm 版本错配时据此协商/告警。
/// 旧消息无此字段 → serde default 填 0 → 视为 legacy 兼容。
pub const HOST_PROTOCOL_VERSION: u32 = 1;

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
    /// 桥协议版本。缺省 0 = legacy（旧 studio/wasm 未带版本）。
    /// A2：商用前立版本号，错配可定位而非静默丢功能。
    #[serde(default)]
    pub schema_version: u32,
}

impl HostMessage {
    /// 构造当前版本消息（自动填 HOST_PROTOCOL_VERSION）。
    pub fn new(
        direction: MessageDirection,
        kind: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            direction,
            kind: kind.into(),
            payload,
            schema_version: HOST_PROTOCOL_VERSION,
        }
    }

    /// 协议版本校验。返回 Ok 表示可处理；Err 表示版本错配需显式回传错误。
    /// 0（legacy）放行（向后兼容）；高于当前版本 → 拒绝（对方比本端新）。
    pub fn check_protocol(&self) -> Result<(), HostConfigError> {
        if self.schema_version == 0 {
            tracing::debug!("HostMessage legacy（无版本字段），按兼容处理");
            return Ok(());
        }
        if self.schema_version > HOST_PROTOCOL_VERSION {
            tracing::warn!(
                "HostMessage 版本 {} 高于本端支持 {}，可能丢字段",
                self.schema_version,
                HOST_PROTOCOL_VERSION
            );
            return Err(HostConfigError::ProtocolTooNew {
                got: self.schema_version,
                supported: HOST_PROTOCOL_VERSION,
            });
        }
        Ok(())
    }
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
    /// A2：桥协议版本高于本端支持，需显式回传错误而非静默丢功能。
    #[error("桥协议版本 {got} 高于本端支持 {supported}")]
    ProtocolTooNew { got: u32, supported: u32 },
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
        assert!(
            cfg.validate().is_err(),
            "block_external=false 必须拒绝（离线硬约束）"
        );
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
        let msg = HostMessage::new(
            MessageDirection::WebViewToBackend,
            "ai.generate",
            serde_json::json!({"prompt": "登录页"}),
        );
        assert_eq!(msg.schema_version, HOST_PROTOCOL_VERSION);
        let s = serde_json::to_string(&msg).unwrap();
        let m2: HostMessage = serde_json::from_str(&s).unwrap();
        assert_eq!(m2.kind, "ai.generate");
        assert_eq!(m2.schema_version, HOST_PROTOCOL_VERSION);
    }

    // A2 回归：旧消息无 schema_version 字段 → default 0 = legacy，check_protocol 放行。
    #[test]
    fn host_message_legacy_no_version_passes() {
        let s = r#"{"direction":"WebViewToBackend","kind":"ai.generate","payload":{}}"#;
        let m: HostMessage = serde_json::from_str(s).unwrap();
        assert_eq!(m.schema_version, 0);
        assert!(m.check_protocol().is_ok(), "legacy 消息应兼容放行");
    }

    // A2 回归：版本高于本端 → check_protocol 报错（不静默丢功能）。
    #[test]
    fn host_message_protocol_too_new_rejected() {
        let m = HostMessage {
            direction: MessageDirection::WebViewToBackend,
            kind: "unknown.future".into(),
            payload: serde_json::json!({}),
            schema_version: HOST_PROTOCOL_VERSION + 1,
        };
        assert!(m.check_protocol().is_err(), "高于本端的版本必须拒绝");
    }
}
