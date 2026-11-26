//! Fusion-Design host bridge — the communication layer embedded in the Fusion-Desk WKWebView.
//!
//! Corresponds to the PRD presentation layer: "Fusion-Desk embeds a WKWebView rendering an infinite
//! vector canvas".
//! Responsibilities:
//! - Load the compiled frontend static assets (file:// protocol, no network)
//! - Provide message forwarding between the WKWebView ↔ local backend services
//! - Intercept all external network requests (offline hard constraint)

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Host bridge protocol version. Used to negotiate/warn on studio↔wasm version mismatches.
/// Old messages without this field → serde default fills 0 → treated as legacy-compatible.
pub const HOST_PROTOCOL_VERSION: u32 = 1;

/// WKWebView message direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageDirection {
    /// WKWebView → local backend
    WebViewToBackend,
    /// Local backend → WKWebView
    BackendToWebView,
}

/// Host bridge message body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostMessage {
    pub direction: MessageDirection,
    pub kind: String,
    pub payload: serde_json::Value,
    /// Bridge protocol version. Default 0 = legacy (old studio/wasm carried no version).
    /// A2: version numbers were established pre-commercialization so mismatches are locatable
    /// instead of silently dropping features.
    #[serde(default)]
    pub schema_version: u32,
}

impl HostMessage {
    /// Build a current-version message (HOST_PROTOCOL_VERSION filled automatically).
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

    /// Protocol version check. Ok means processable; Err means a version mismatch that requires
    /// sending back an explicit error.
    /// 0 (legacy) passes (backward compatible); above the current version → reject (the peer is newer).
    pub fn check_protocol(&self) -> Result<(), HostConfigError> {
        if self.schema_version == 0 {
            tracing::debug!("HostMessage legacy (no version field), handled with compatibility");
            return Ok(());
        }
        if self.schema_version > HOST_PROTOCOL_VERSION {
            tracing::warn!(
                "HostMessage version {} is above the supported {}, fields may be lost",
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

/// Host bridge configuration.
#[derive(Debug, Clone)]
pub struct HostBridgeConfig {
    /// Frontend static assets directory (file:// load entry).
    pub frontend_dir: PathBuf,
    /// Local backend service endpoint (must be 127.0.0.1).
    pub backend_endpoint: String,
    /// Whether to intercept all external requests (default true, offline hard constraint).
    pub block_external: bool,
}

impl HostBridgeConfig {
    /// Validate the configuration (offline hard constraint).
    pub fn validate(&self) -> Result<(), HostConfigError> {
        if !self.frontend_dir.exists() {
            return Err(HostConfigError::FrontendMissing(self.frontend_dir.clone()));
        }
        if !self.frontend_dir.join("index.html").exists() {
            return Err(HostConfigError::IndexMissing(self.frontend_dir.clone()));
        }
        if !self.block_external {
            tracing::warn!("block_external=false violates the offline hard constraint");
            return Err(HostConfigError::OfflineConstraintViolated);
        }
        validate_localhost(&self.backend_endpoint)?;
        Ok(())
    }

    /// Return the file:// URL of the frontend entry.
    pub fn index_url(&self) -> String {
        let path = self.frontend_dir.join("index.html");
        format!("file://{}", path.display())
    }
}

/// Validate that the endpoint is localhost.
fn validate_localhost(endpoint: &str) -> Result<(), HostConfigError> {
    let url =
        url::Url::parse(endpoint).map_err(|e| HostConfigError::InvalidEndpoint(e.to_string()))?;
    let host = url.host_str().unwrap_or("");
    // url::Url returns IPv6 hosts like "[::1]"; strip the brackets before comparing
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host != "127.0.0.1" && host != "localhost" && host != "::1" {
        return Err(HostConfigError::PublicEndpoint(host.to_string()));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum HostConfigError {
    #[error("frontend directory does not exist: {0}")]
    FrontendMissing(PathBuf),
    #[error("frontend directory is missing index.html: {0}")]
    IndexMissing(PathBuf),
    #[error("invalid endpoint: {0}")]
    InvalidEndpoint(String),
    #[error("offline constraint violated: endpoint host {0} is not localhost")]
    PublicEndpoint(String),
    #[error("offline hard constraint violated: block_external=false, offline mode cannot be disabled")]
    OfflineConstraintViolated,
    /// A2: the bridge protocol version is above what this side supports; an explicit error must be
    /// sent back instead of silently dropping features.
    #[error("bridge protocol version {got} is above the supported {supported}")]
    ProtocolTooNew { got: u32, supported: u32 },
}

/// External request interceptor (for injection into the WKWebView configuration).
///
/// In the real Swift host this corresponds to a `WKURLSchemeHandler` injection;
/// here the Rust-side decision logic is provided for reuse by backend services.
///
/// H-A17 offline division of labor: this function judges the **URL scheme/host** (file:// allowed,
/// javascript:/data: rejected, http(s) only localhost) for WKWebView frontend asset interception;
/// fd-ai-adapter's `validate_localhost` judges the **MLX endpoint host** (loopback+RFC1918+
/// link-local) for backend HTTP egress. The two are not duplicates — they guard the two separate
/// offline boundaries of frontend assets and backend inference. The global constraint on outbound
/// HTTP libraries is enforced by the `Scripts/deny-external-http.sh` CI gate (reqwest etc. allowed
/// only in fd-ai-adapter).
pub fn is_external_url(url: &str) -> bool {
    // F2/offline enforcement: allowlist decision.
    //   Internal = file:// (local assets, the offline frontend entry) or http/https with host on the allowlist.
    //   External = javascript:/data: (dangerous host-less schemes, XSS carriers), and parse failures.
    // The old implementation classified host-less javascript:/data: as "internal/allowed" — an XSS pass-through hole.
    match url::Url::parse(url) {
        Ok(u) => {
            let scheme = u.scheme();
            if scheme == "file" {
                return false; // local file asset, allowed offline
            }
            let host = u.host_str().unwrap_or("");
            let internal_host = host == "127.0.0.1" || host == "localhost" || host == "::1";
            !(scheme == "http" || scheme == "https") || !internal_host
        }
        Err(_) => true, // invalid URLs are always treated as external; reject conservatively
    }
}

/// Compliance-check helper for the frontend static assets directory.
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
        // file:// local asset: allowed offline
        assert!(!is_external_url("file:///path/index.html"));
        // F2 regression: the host-less dangerous schemes javascript:/data: must be judged external (the old implementation allowed them = XSS hole)
        assert!(is_external_url("javascript:alert(1)"));
        assert!(is_external_url("data:text/html,<script>alert(1)</script>"));
        // Invalid URLs are conservatively judged external
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

    // F2 regression: offline is a hard constraint, block_external=false must refuse startup (the old implementation only warned).
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
            "block_external=false must be rejected (offline hard constraint)"
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
            serde_json::json!({"prompt": "login page"}),
        );
        assert_eq!(msg.schema_version, HOST_PROTOCOL_VERSION);
        let s = serde_json::to_string(&msg).unwrap();
        let m2: HostMessage = serde_json::from_str(&s).unwrap();
        assert_eq!(m2.kind, "ai.generate");
        assert_eq!(m2.schema_version, HOST_PROTOCOL_VERSION);
    }

    // A2 regression: old messages without a schema_version field → default 0 = legacy, check_protocol lets them pass.
    #[test]
    fn host_message_legacy_no_version_passes() {
        let s = r#"{"direction":"WebViewToBackend","kind":"ai.generate","payload":{}}"#;
        let m: HostMessage = serde_json::from_str(s).unwrap();
        assert_eq!(m.schema_version, 0);
        assert!(m.check_protocol().is_ok(), "legacy messages should pass for compatibility");
    }

    // A2 regression: a version above ours → check_protocol errors (no silent feature loss).
    #[test]
    fn host_message_protocol_too_new_rejected() {
        let m = HostMessage {
            direction: MessageDirection::WebViewToBackend,
            kind: "unknown.future".into(),
            payload: serde_json::json!({}),
            schema_version: HOST_PROTOCOL_VERSION + 1,
        };
        assert!(m.check_protocol().is_err(), "a version above ours must be rejected");
    }
}
